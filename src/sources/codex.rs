use super::{
    Adapter, DiscoveredHome, DiscoveredSources, LimitWindow, MessagePresentation, NormalizedRecord,
    PressureSnapshot, SessionSummary, SourceLayout, TokenUsage,
};
use crate::fingerprint::{HashingReader, ensure_unchanged};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    collections::HashMap,
    fs::{File, Metadata},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

pub(crate) const GENERATED_IMAGE_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GeneratedImage {
    pub(crate) filename: String,
    pub(crate) size_bytes: u64,
}

pub(crate) fn generated_image_aliases(
    home: &Path,
    session_id: &str,
    tool_arguments: &[String],
) -> Result<HashMap<String, String>> {
    let mut aliases = HashMap::new();
    for arguments in tool_arguments {
        let expanded = arguments.replace("\\n", "\n");
        for line in expanded.lines() {
            let tokens = command_tokens(line);
            let Some((source, destination)) = copy_operands(&tokens) else {
                continue;
            };
            let Some(alias) = relative_image_alias(destination) else {
                continue;
            };
            let normalized_source = source.replace('\\', "/");
            let marker = format!("/generated_images/{session_id}/");
            let Some(filename) = normalized_source
                .split_once(&marker)
                .map(|(_, filename)| filename)
                .filter(|filename| !filename.contains('/'))
            else {
                continue;
            };
            if generated_image_path(home, session_id, filename)?.is_some() {
                aliases.insert(alias, filename.to_owned());
            }
        }
    }
    Ok(aliases)
}

fn command_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match quote {
            Some(active) if character == active => quote = None,
            Some(_) => current.push(character),
            None if matches!(character, '\'' | '"') => quote = Some(character),
            None if character.is_whitespace() => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            None if character == '\\'
                && characters.peek().is_some_and(|next| next.is_whitespace()) =>
            {
                current.push(characters.next().expect("checked next character"));
            }
            None => current.push(character),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn copy_operands(tokens: &[String]) -> Option<(&str, &str)> {
    if let Some(command) = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case("Copy-Item"))
    {
        let source = flag_value(tokens, command + 1, &["-LiteralPath", "-Path"])?;
        let destination = flag_value(tokens, command + 1, &["-Destination"])?;
        return Some((source, destination));
    }
    let command = tokens
        .iter()
        .position(|token| token.rsplit('/').next() == Some("cp"))?;
    let operands: Vec<_> = tokens[command + 1..]
        .iter()
        .filter(|token| !token.starts_with('-'))
        .collect();
    (operands.len() >= 2).then(|| {
        (
            operands[operands.len() - 2].as_str(),
            operands[operands.len() - 1].as_str(),
        )
    })
}

fn flag_value<'a>(tokens: &'a [String], start: usize, names: &[&str]) -> Option<&'a str> {
    let index = tokens[start..]
        .iter()
        .position(|token| names.iter().any(|name| token.eq_ignore_ascii_case(name)))?
        + start;
    tokens.get(index + 1).map(String::as_str)
}

fn relative_image_alias(value: &str) -> Option<String> {
    let value = value
        .trim_end_matches(['"', '}', ')', ';'])
        .replace('\\', "/");
    let value = value.strip_prefix("./").unwrap_or(&value);
    if value.starts_with('/') || value.contains(':') {
        return None;
    }
    let components: Vec<_> = value.split('/').collect();
    if components.is_empty()
        || components
            .iter()
            .any(|component| component.is_empty() || matches!(*component, "." | ".."))
        || !valid_generated_image_name(components.last().copied().unwrap_or_default())
    {
        return None;
    }
    Some(components.join("/"))
}

pub(crate) fn generated_images(home: &Path, session_id: &str) -> Result<Vec<GeneratedImage>> {
    let Some(directory) = generated_image_directory(home, session_id)? else {
        return Ok(Vec::new());
    };
    let mut images = Vec::new();
    for entry in std::fs::read_dir(directory).context("read generated image directory")? {
        let entry = entry.context("read generated image entry")?;
        let file_type = entry.file_type().context("inspect generated image entry")?;
        let Some(filename) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !file_type.is_file() || !valid_generated_image_name(&filename) {
            continue;
        }
        let size_bytes = entry.metadata().context("inspect generated image")?.len();
        if size_bytes <= GENERATED_IMAGE_LIMIT {
            images.push(GeneratedImage {
                filename,
                size_bytes,
            });
        }
    }
    images.sort_by(|left, right| left.filename.cmp(&right.filename));
    images.truncate(100);
    Ok(images)
}

pub(crate) fn generated_image_path(
    home: &Path,
    session_id: &str,
    filename: &str,
) -> Result<Option<PathBuf>> {
    if !valid_generated_image_name(filename) {
        return Ok(None);
    }
    let Some(directory) = generated_image_directory(home, session_id)? else {
        return Ok(None);
    };
    let candidate = directory.join(filename);
    match std::fs::symlink_metadata(&candidate) {
        Ok(metadata)
            if metadata.file_type().is_file() && metadata.len() <= GENERATED_IMAGE_LIMIT => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspect generated image"),
    }
    let canonical = candidate
        .canonicalize()
        .context("resolve generated image path")?;
    ensure!(
        canonical.starts_with(&directory),
        "generated image escaped its source directory"
    );
    Ok(Some(canonical))
}

fn generated_image_directory(home: &Path, session_id: &str) -> Result<Option<PathBuf>> {
    if !valid_path_component(session_id) {
        return Ok(None);
    }
    let root = home.join("generated_images");
    let directory = root.join(session_id);
    match std::fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspect generated image directory"),
    }
    let canonical_root = root
        .canonicalize()
        .context("resolve generated image root")?;
    let canonical_directory = directory
        .canonicalize()
        .context("resolve generated image directory")?;
    ensure!(
        canonical_directory.starts_with(&canonical_root),
        "generated image directory escaped its source home"
    );
    Ok(Some(canonical_directory))
}

fn valid_path_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_generated_image_name(value: &str) -> bool {
    value.len() <= 200
        && value
            .strip_suffix(".png")
            .or_else(|| value.strip_suffix(".jpg"))
            .or_else(|| value.strip_suffix(".jpeg"))
            .or_else(|| value.strip_suffix(".webp"))
            .is_some_and(valid_path_component)
}

pub(crate) const KIND: &str = "codex";
pub(crate) const DISPLAY_NAME: &str = "Codex";
pub(crate) const PARSER_VERSION: i64 = 7;
pub(crate) const SOURCE_AREAS: [&str; 2] = ["sessions", "archived_sessions"];
pub(crate) const HIDDEN_MODEL_PATTERN: &str = "codex-auto-review%";
const HOST_USER_LAYOUT: &str = "host-user";
const DIRECT_HOME_LAYOUT: &str = "direct-codex-home";
const MAX_PARSE_DIAGNOSTICS: usize = 20;

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
                adapter: Adapter::Codex,
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
                    adapter: Adapter::Codex,
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

pub(crate) fn scan_rollout(path: &Path) -> Result<SessionSummary> {
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
    Ok(SessionSummary {
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
        let per_turn = token_usage(&payload["info"]["last_token_usage"]);
        Some(NormalizedRecord::Usage {
            timestamp: value["timestamp"].as_str().map(str::to_owned),
            cumulative: token_usage(&payload["info"]["total_token_usage"]),
            pressure: Box::new(pressure_snapshot(payload, per_turn.as_ref())),
            per_turn,
            raw_json: value.to_string(),
        })
    } else {
        None
    }
}

fn pressure_snapshot(payload: &Value, per_turn: Option<&TokenUsage>) -> PressureSnapshot {
    let rate_limits = &payload["rate_limits"];
    PressureSnapshot {
        context_window: payload["info"]["model_context_window"].as_i64(),
        context_tokens: per_turn.and_then(|usage| usage.total_tokens),
        rate_limit_id: string_field(rate_limits, &["limit_id"]),
        rate_limit_name: string_field(rate_limits, &["limit_name"]),
        primary: limit_window(&rate_limits["primary"]),
        secondary: limit_window(&rate_limits["secondary"]),
        plan_type: string_field(rate_limits, &["plan_type"]),
        rate_limit_reached_type: string_field(rate_limits, &["rate_limit_reached_type"]),
    }
}

fn limit_window(value: &Value) -> LimitWindow {
    LimitWindow {
        used_percent: value["used_percent"].as_f64(),
        window_minutes: value["window_minutes"].as_i64(),
        resets_at: value["resets_at"].as_i64(),
        resets_in_seconds: value["resets_in_seconds"].as_i64(),
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

    #[test]
    fn generated_images_are_session_scoped_and_reject_unsafe_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".codex");
        let directory = home.join("generated_images/synthetic-session");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("exec-safe.png"), b"synthetic image").unwrap();
        std::fs::write(directory.join("not-an-image.txt"), b"private").unwrap();

        let images = generated_images(&home, "synthetic-session").unwrap();
        assert_eq!(
            images,
            vec![GeneratedImage {
                filename: "exec-safe.png".into(),
                size_bytes: 15,
            }]
        );
        assert_eq!(
            generated_image_path(&home, "synthetic-session", "exec-safe.png")
                .unwrap()
                .unwrap(),
            directory.canonicalize().unwrap().join("exec-safe.png")
        );
        assert!(
            generated_image_path(&home, "../synthetic-session", "exec-safe.png")
                .unwrap()
                .is_none()
        );
        assert!(
            generated_image_path(&home, "synthetic-session", "../private.png")
                .unwrap()
                .is_none()
        );
        assert!(
            generated_image_path(&home, "synthetic-session", "not-an-image.txt")
                .unwrap()
                .is_none()
        );

        let aliases = generated_image_aliases(
            &home,
            "synthetic-session",
            &[
                "text(await tools.exec_command({cmd:\"New-Item outputs\\nCopy-Item -LiteralPath 'C:/Users/test/.codex/generated_images/synthetic-session/exec-safe.png' -Destination 'outputs/windows-preview.png'\"}));".into(),
                "cp '$HOME/.codex/generated_images/synthetic-session/exec-safe.png' ./outputs/unix-preview.png".into(),
                "cp '$HOME/.codex/generated_images/other-session/exec-safe.png' outputs/wrong.png".into(),
                "cp '$HOME/.codex/generated_images/synthetic-session/exec-safe.png' ../escaped.png".into(),
            ],
        )
        .unwrap();
        assert_eq!(
            aliases,
            HashMap::from([
                ("outputs/windows-preview.png".into(), "exec-safe.png".into()),
                ("outputs/unix-preview.png".into(), "exec-safe.png".into()),
            ])
        );
    }
}
