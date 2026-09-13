use super::{
    DiscoveredHome, DiscoveredSources, MessagePresentation, NormalizedRecord, SourceLayout,
    TokenUsage,
};
use crate::fingerprint::{HashingReader, ensure_unchanged};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    fs::{File, Metadata},
    io::{BufRead, BufReader},
    path::Path,
};

pub(crate) const KIND: &str = "codex";
pub(crate) const DISPLAY_NAME: &str = "Codex";
pub(crate) const PARSER_VERSION: i64 = 6;
pub(crate) const SOURCE_AREAS: [&str; 2] = ["sessions", "archived_sessions"];
pub(crate) const HIDDEN_MODEL_PATTERN: &str = "codex-auto-review%";
const HOST_USER_LAYOUT: &str = "host-user";
const DIRECT_HOME_LAYOUT: &str = "direct-codex-home";
const MAX_PARSE_DIAGNOSTICS: usize = 20;

pub(crate) struct RolloutSummary {
    pub(crate) metadata: Metadata,
    pub(crate) content_hash: String,
    pub(crate) external_id: String,
    pub(crate) started_at: Option<String>,
    pub(crate) ended_at: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) status: &'static str,
    pub(crate) raw_metadata: String,
    pub(crate) derived_title: Option<String>,
    pub(crate) malformed_lines: usize,
    pub(crate) parse_error: Option<String>,
}

pub(crate) struct RolloutRecords<'a> {
    path: &'a Path,
    metadata: &'a Metadata,
    reader: BufReader<File>,
    sequence: i64,
    malformed_lines: usize,
    finished: bool,
    line: Vec<u8>,
}

pub(crate) struct AdvisoryTitles {
    reader: BufReader<File>,
    line: Vec<u8>,
    finished: bool,
}

impl AdvisoryTitles {
    pub(crate) fn open(home: &Path) -> Option<Self> {
        let file = File::open(home.join("session_index.jsonl")).ok()?;
        Some(Self {
            reader: BufReader::new(file),
            line: Vec::new(),
            finished: false,
        })
    }

    pub(crate) fn next_title(&mut self) -> Option<(String, String)> {
        while !self.finished {
            self.line.clear();
            match self.reader.read_until(b'\n', &mut self.line) {
                Ok(0) | Err(_) => {
                    self.finished = true;
                    return None;
                }
                Ok(_) => {}
            }
            let Ok(value) = serde_json::from_slice::<Value>(&self.line) else {
                continue;
            };
            let (Some(id), Some(title)) = (value["id"].as_str(), value["thread_name"].as_str())
            else {
                continue;
            };
            if let Some(title) = sanitized_title(title, 160) {
                return Some((id.to_owned(), title));
            }
        }
        None
    }
}

impl<'a> RolloutRecords<'a> {
    pub(crate) fn open(path: &'a Path, metadata: &'a Metadata) -> Result<Self> {
        ensure_unchanged(path, metadata)?;
        let file = File::open(path).context("reopen rollout for record import")?;
        Ok(Self {
            path,
            metadata,
            reader: BufReader::new(file),
            sequence: 0,
            malformed_lines: 0,
            finished: false,
            line: Vec::new(),
        })
    }

    pub(crate) fn next_record(&mut self) -> Result<Option<(i64, NormalizedRecord)>> {
        loop {
            if self.finished {
                return Ok(None);
            }
            self.line.clear();
            if self
                .reader
                .read_until(b'\n', &mut self.line)
                .context("read rollout record for import")?
                == 0
            {
                ensure_unchanged(self.path, self.metadata)?;
                self.finished = true;
                return Ok(None);
            }
            let value = match serde_json::from_slice::<Value>(&self.line) {
                Ok(value) => value,
                Err(_) => {
                    self.malformed_lines = self
                        .malformed_lines
                        .checked_add(1)
                        .context("malformed-line count exceeds supported range")?;
                    continue;
                }
            };
            let sequence = self.sequence;
            self.sequence = self
                .sequence
                .checked_add(1)
                .context("rollout record count exceeds supported range")?;
            if let Some(record) = normalize_record(value) {
                return Ok(Some((sequence, record)));
            }
        }
    }

    pub(crate) fn malformed_lines(&self) -> usize {
        self.malformed_lines
    }
}

#[derive(Default)]
struct ParseDiagnostics {
    malformed_lines: usize,
    examples: Vec<String>,
}

impl ParseDiagnostics {
    fn record(&mut self, line_number: usize) -> Result<()> {
        self.malformed_lines = self
            .malformed_lines
            .checked_add(1)
            .context("malformed-line count exceeds supported range")?;
        if self.examples.len() < MAX_PARSE_DIAGNOSTICS {
            self.examples
                .push(format!("line {line_number}: invalid JSON or UTF-8"));
        }
        Ok(())
    }

    fn message(&self) -> Option<String> {
        if self.malformed_lines == 0 {
            return None;
        }
        let mut message = self.examples.join("; ");
        let omitted = self.malformed_lines - self.examples.len();
        if omitted != 0 {
            message.push_str(&format!("; {omitted} additional malformed lines omitted"));
        }
        Some(message)
    }
}

pub(crate) fn discover(root: &Path) -> Result<DiscoveredSources> {
    ensure!(
        optional_directory(root)?,
        "source root is missing; refresh cancelled"
    );
    if root.file_name() == Some(std::ffi::OsStr::new(".codex")) {
        return Ok(DiscoveredSources {
            layout: SourceLayout::DirectHome,
            database_layout: DIRECT_HOME_LAYOUT,
            homes: vec![DiscoveredHome {
                host: String::new(),
                user: String::new(),
                path: root.to_path_buf(),
            }],
        });
    }
    let mut homes = Vec::new();
    for host in std::fs::read_dir(root).context("read source root")? {
        let host = host.context("read source root entry")?;
        let kind = host.file_type().context("inspect host entry")?;
        if !(kind.is_dir() || kind.is_symlink() && optional_directory(&host.path())?) {
            continue;
        }
        for user in std::fs::read_dir(host.path()).context("read source host directory")? {
            let user = user.context("read source host entry")?;
            let kind = user.file_type().context("inspect user entry")?;
            if !(kind.is_dir() || kind.is_symlink() && optional_directory(&user.path())?) {
                continue;
            }
            let path = user.path().join(".codex");
            if optional_directory(&path)? {
                homes.push(DiscoveredHome {
                    host: host.file_name().to_string_lossy().into(),
                    user: user.file_name().to_string_lossy().into(),
                    path,
                });
            }
        }
    }
    Ok(DiscoveredSources {
        layout: SourceLayout::HostUser,
        database_layout: HOST_USER_LAYOUT,
        homes,
    })
}

pub(crate) fn optional_directory(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            let metadata = if metadata.file_type().is_symlink() {
                std::fs::metadata(path).context("resolve source directory symbolic link")?
            } else {
                metadata
            };
            ensure!(
                metadata.is_dir(),
                "expected a source directory or symbolic link to a directory"
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("inspect source directory"),
    }
}

pub(crate) fn home_belongs_to_root(home: &Path, root: &Path, layout: SourceLayout) -> bool {
    match layout {
        SourceLayout::DirectHome => home == root,
        SourceLayout::HostUser => home.strip_prefix(root).is_ok_and(|relative| {
            relative.components().count() == 3
                && relative.file_name() == Some(std::ffi::OsStr::new(".codex"))
        }),
    }
}

pub(crate) fn scan_rollout(path: &Path) -> Result<RolloutSummary> {
    let file = File::open(path).context("open rollout")?;
    let metadata = file.metadata().context("inspect rollout")?;
    let mut reader = BufReader::new(HashingReader::new(file));
    let mut session_metadata = None;
    let mut model = None;
    let mut saw_turn_context = false;
    let mut ended_at = None;
    let mut status = "unknown";
    let mut derived_title = None;
    let mut conversation_started = false;
    let mut diagnostics = ParseDiagnostics::default();
    let mut line = Vec::new();
    let mut line_number = 0usize;
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .context("read rollout line")?
            == 0
        {
            break;
        }
        line_number = line_number
            .checked_add(1)
            .context("rollout line count exceeds supported range")?;
        match serde_json::from_slice::<Value>(&line) {
            Ok(value) => {
                let capture_session_metadata =
                    session_metadata.is_none() && value["type"] == "session_meta";
                if !saw_turn_context && value["type"] == "turn_context" {
                    saw_turn_context = true;
                    model = value["payload"]["model"].as_str().map(str::to_owned);
                }
                if let Some(timestamp) = value["timestamp"].as_str() {
                    ended_at = Some(timestamp.to_owned());
                }
                status = next_session_status(status, &value);
                let payload = &value["payload"];
                if value["type"] == "response_item"
                    && payload["type"] == "message"
                    && payload["role"] == "user"
                    && derived_title.is_none()
                {
                    let content = message_content(payload);
                    let presentation = classify_message("user", &content, !conversation_started);
                    if presentation
                        .as_ref()
                        .is_none_or(|presentation| presentation.initially_open)
                    {
                        conversation_started = true;
                    }
                    if presentation.is_none() {
                        derived_title = sanitized_title(&content, 80);
                    }
                }
                if capture_session_metadata {
                    session_metadata = Some(value);
                }
            }
            Err(_) => diagnostics.record(line_number)?,
        }
    }
    ensure_unchanged(path, &metadata)?;
    let content_hash = reader.get_ref().fingerprint();
    let meta = session_metadata.context("missing session_meta")?;
    let payload = &meta["payload"];
    let external_id = payload["id"]
        .as_str()
        .context("session_meta missing id")?
        .to_owned();
    let started_at = payload["timestamp"]
        .as_str()
        .or_else(|| meta["timestamp"].as_str())
        .map(str::to_owned);
    Ok(RolloutSummary {
        metadata,
        content_hash,
        external_id,
        started_at,
        ended_at,
        cwd: payload["cwd"].as_str().map(str::to_owned),
        model,
        status,
        raw_metadata: meta.to_string(),
        derived_title,
        malformed_lines: diagnostics.malformed_lines,
        parse_error: diagnostics.message(),
    })
}

fn sanitized_title(value: &str, max_chars: usize) -> Option<String> {
    if value
        .chars()
        .any(|character| character.is_control() && !character.is_whitespace())
    {
        return None;
    }
    let mut normalized = String::new();
    let mut characters = 0usize;
    for word in value.split_whitespace() {
        if characters != 0 {
            normalized.push(' ');
            characters += 1;
        }
        for character in word.chars() {
            if characters == max_chars {
                return Some(normalized);
            }
            normalized.push(character);
            characters += 1;
        }
        if characters == max_chars {
            return Some(normalized);
        }
    }
    if normalized.is_empty() {
        return None;
    }
    Some(normalized)
}

fn normalize_record(value: Value) -> Option<NormalizedRecord> {
    let typ = value["type"].as_str().unwrap_or("");
    let payload = &value["payload"];
    if typ == "response_item" && payload["type"] == "message" {
        let role = payload["role"].as_str().unwrap_or("unknown").to_owned();
        Some(NormalizedRecord::Message {
            timestamp: value["timestamp"].as_str().map(str::to_owned),
            opens_turn: role == "user",
            role,
            content: message_content(payload),
            raw_json: value.to_string(),
        })
    } else if typ == "turn_context" {
        Some(NormalizedRecord::TurnEvent {
            timestamp: value["timestamp"].as_str().map(str::to_owned),
            kind: "requested_model",
            turn_id: string_field(payload, &["turn_id", "id"]),
            requested_model: string_field(payload, &["model", "requested_model"]),
            served_model: None,
            raw_json: value.to_string(),
        })
    } else if typ == "event_msg"
        && matches!(
            payload["type"].as_str(),
            Some(
                "task_started"
                    | "turn_started"
                    | "task_complete"
                    | "turn_complete"
                    | "turn_aborted"
                    | "user_message"
                    | "model_reroute"
            )
        )
    {
        let event_type = payload["type"].as_str().unwrap_or_default();
        Some(NormalizedRecord::TurnEvent {
            timestamp: value["timestamp"].as_str().map(str::to_owned),
            kind: match event_type {
                "task_started" | "turn_started" => "started",
                "task_complete" | "turn_complete" => "completed",
                "turn_aborted" => "interrupted",
                "user_message" => "user_activity",
                _ => "served_model_fallback",
            },
            turn_id: string_field(payload, &["turn_id", "id"]),
            requested_model: (event_type == "model_reroute")
                .then(|| string_field(payload, &["from_model", "requested_model"]))
                .flatten(),
            served_model: (event_type == "model_reroute")
                .then(|| string_field(payload, &["to_model", "model", "served_model"]))
                .flatten(),
            raw_json: value.to_string(),
        })
    } else if typ == "response_item"
        && matches!(
            payload["type"].as_str(),
            Some("function_call" | "custom_tool_call")
        )
    {
        let tool_name = payload["name"].as_str().map(str::to_owned);
        Some(NormalizedRecord::ToolCall {
            timestamp: value["timestamp"].as_str().map(str::to_owned),
            call_id: payload["call_id"]
                .as_str()
                .or_else(|| payload["id"].as_str())
                .map(str::to_owned),
            kind: if tool_name.as_deref() == Some("request_user_input") {
                "user_input"
            } else {
                "tool"
            },
            tool_name,
            arguments: json_text(payload.get("arguments").or_else(|| payload.get("input"))),
            raw_json: value.to_string(),
        })
    } else if typ == "response_item"
        && matches!(
            payload["type"].as_str(),
            Some("function_call_output" | "custom_tool_call_output")
        )
    {
        Some(NormalizedRecord::ToolOutput {
            timestamp: value["timestamp"].as_str().map(str::to_owned),
            call_id: payload["call_id"]
                .as_str()
                .or_else(|| payload["id"].as_str())
                .map(str::to_owned),
            output: json_text(payload.get("output")),
            outcome: if explicit_tool_failure(payload) {
                "failed"
            } else {
                "observed"
            },
            raw_json: value.to_string(),
        })
    } else if typ == "event_msg" && payload["type"] == "token_count" {
        Some(NormalizedRecord::Usage {
            timestamp: value["timestamp"].as_str().map(str::to_owned),
            cumulative: token_usage(&payload["info"]["total_token_usage"]),
            per_turn: token_usage(&payload["info"]["last_token_usage"]),
            raw_json: value.to_string(),
        })
    } else {
        None
    }
}

fn token_usage(value: &Value) -> Option<TokenUsage> {
    value.as_object().map(|_| TokenUsage {
        input_tokens: value["input_tokens"].as_i64(),
        output_tokens: value["output_tokens"].as_i64(),
        cached_input_tokens: value["cached_input_tokens"].as_i64(),
        reasoning_output_tokens: value["reasoning_output_tokens"]
            .as_i64()
            .or_else(|| value["reasoning_tokens"].as_i64()),
        total_tokens: value["total_tokens"].as_i64(),
    })
}

fn string_field(value: &Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value[*name].as_str().map(str::to_owned))
}

fn json_text(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Null) | None => None,
        Some(value) => Some(value.to_string()),
    }
}

fn explicit_tool_failure(payload: &Value) -> bool {
    payload["success"].as_bool() == Some(false)
        || payload["is_error"].as_bool() == Some(true)
        || matches!(payload["status"].as_str(), Some("failed" | "error"))
        || payload["output"]["success"].as_bool() == Some(false)
        || payload["output"]["is_error"].as_bool() == Some(true)
        || matches!(
            payload["output"]["status"].as_str(),
            Some("failed" | "error")
        )
}

fn message_content(payload: &Value) -> String {
    let Some(parts) = payload["content"].as_array() else {
        return String::new();
    };
    let mut content = String::new();
    let mut has_text = false;
    for text in parts.iter().filter_map(|part| part["text"].as_str()) {
        if has_text {
            content.push('\n');
        }
        content.push_str(text);
        has_text = true;
    }
    content
}

fn next_session_status(status: &'static str, value: &Value) -> &'static str {
    let record_type = value["type"].as_str();
    let payload = &value["payload"];
    match (record_type, payload["type"].as_str()) {
        (Some("event_msg"), Some("task_complete" | "turn_complete")) => "complete",
        (Some("event_msg"), Some("turn_aborted")) => "aborted",
        (Some("event_msg"), Some("task_started" | "turn_started" | "user_message"))
        | (Some("turn_context"), _) => "unknown",
        (Some("response_item"), Some("message")) if payload["role"].as_str() == Some("user") => {
            "unknown"
        }
        _ => status,
    }
}

pub(crate) fn classify_message(
    role: &str,
    content: &str,
    before_conversation: bool,
) -> Option<MessagePresentation> {
    let trimmed = content.trim_start();
    let title = if matches!(role, "user" | "developer" | "system") {
        if trimmed.starts_with("# AGENTS.md instructions for ") {
            Some("Repository instructions")
        } else if trimmed.starts_with("<permissions instructions>") {
            Some("Permissions")
        } else if trimmed.starts_with("<collaboration_mode>") {
            Some("Collaboration mode")
        } else if trimmed.starts_with("<model_switch>") {
            Some("Model switch")
        } else if trimmed.starts_with("<skills_instructions>") {
            Some("Available skills")
        } else if before_conversation && trimmed.starts_with("You are ") {
            Some("Initial agent configuration")
        } else if trimmed.starts_with("<multi_agent_mode>") {
            Some("Multi-agent mode")
        } else if trimmed.starts_with("<multi_agent_role>") {
            Some("Multi-agent role")
        } else if trimmed.starts_with("<environment_context>") {
            Some("Environment context")
        } else {
            None
        }
    } else {
        None
    };
    if let Some(title) = title {
        return Some(MessagePresentation {
            title,
            initially_open: false,
        });
    }
    let title = match role {
        "user" | "developer" | "system" if trimmed.starts_with("<turn_aborted>") => {
            "Turn interrupted"
        }
        "user" if trimmed.starts_with("<user_action>") => "User action",
        _ => return None,
    };
    Some(MessagePresentation {
        title,
        initially_open: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_contract_homes() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("host/user/.codex/sessions")).unwrap();
        let homes = discover(tmp.path()).unwrap();
        assert_eq!(homes.layout, SourceLayout::HostUser);
        assert_eq!(homes.homes.len(), 1);
        assert_eq!(homes.homes[0].host, "host");
        assert_eq!(homes.homes[0].user, "user");
    }
}
