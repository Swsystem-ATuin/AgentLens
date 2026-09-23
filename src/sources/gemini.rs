use super::{
    Adapter, DiscoveredHome, DiscoveredSources, MessagePresentation, NormalizedRecord,
    PressureSnapshot, SessionSummary, SourceLayout, TokenUsage,
};
use crate::fingerprint::{ensure_unchanged, hash_file};
use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{File, Metadata},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

pub(crate) const KIND: &str = "gemini";
pub(crate) const DISPLAY_NAME: &str = "Gemini CLI";
pub(crate) const PARSER_VERSION: i64 = 2;
pub(crate) const SOURCE_AREAS: [&str; 1] = ["chats"];
const HOST_USER_LAYOUT: &str = "host-user";
const DIRECT_HOME_LAYOUT: &str = "direct-gemini-home";
const MAX_PARSE_DIAGNOSTICS: usize = 20;

struct ParsedSession {
    metadata: Map<String, Value>,
    messages: Vec<Value>,
    malformed_lines: usize,
    parse_error: Option<String>,
}

#[derive(Default)]
struct MessageSet {
    values: Vec<Value>,
    positions: HashMap<String, usize>,
}

pub(crate) struct SessionRecords {
    events: VecDeque<(i64, NormalizedRecord)>,
    malformed_lines: usize,
}

impl SessionRecords {
    pub(crate) fn open(path: &Path, metadata: &Metadata) -> Result<Self> {
        ensure_unchanged(path, metadata)?;
        let parsed = parse_session(path)?;
        ensure_unchanged(path, metadata)?;
        let mut events = VecDeque::new();
        let mut legacy_results = HashSet::new();
        let mut sequence = 0i64;
        for message in parsed.messages {
            for record in normalize_message(&message, &mut legacy_results) {
                events.push_back((sequence, record));
                sequence = sequence
                    .checked_add(1)
                    .context("Gemini event count exceeds supported range")?;
            }
        }
        Ok(Self {
            events,
            malformed_lines: parsed.malformed_lines,
        })
    }

    pub(crate) fn next_record(&mut self) -> Option<(i64, NormalizedRecord)> {
        self.events.pop_front()
    }

    pub(crate) fn malformed_lines(&self) -> usize {
        self.malformed_lines
    }
}

pub(crate) fn discover(root: &Path) -> Result<DiscoveredSources> {
    ensure!(
        super::codex::optional_directory(root)?,
        "source root is missing; refresh cancelled"
    );
    if root.file_name() == Some(std::ffi::OsStr::new(".gemini")) {
        return Ok(DiscoveredSources {
            layout: SourceLayout::DirectHome,
            database_layout: DIRECT_HOME_LAYOUT,
            homes: vec![DiscoveredHome {
                adapter: Adapter::Gemini,
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
        if !(kind.is_dir() || kind.is_symlink() && super::codex::optional_directory(&host.path())?)
        {
            continue;
        }
        for user in std::fs::read_dir(host.path()).context("read source host directory")? {
            let user = user.context("read source host entry")?;
            let kind = user.file_type().context("inspect user entry")?;
            if !(kind.is_dir()
                || kind.is_symlink() && super::codex::optional_directory(&user.path())?)
            {
                continue;
            }
            let path = user.path().join(".gemini");
            if super::codex::optional_directory(&path)? {
                homes.push(DiscoveredHome {
                    adapter: Adapter::Gemini,
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

pub(crate) fn chat_roots(home: &Path) -> Result<Vec<PathBuf>> {
    let tmp = home.join("tmp");
    if !super::codex::optional_directory(&tmp)? {
        return Ok(Vec::new());
    }
    let mut roots = Vec::new();
    for project in std::fs::read_dir(&tmp).context("read Gemini project storage")? {
        let project = project.context("read Gemini project entry")?;
        let kind = project
            .file_type()
            .context("inspect Gemini project entry")?;
        if !(kind.is_dir()
            || kind.is_symlink() && super::codex::optional_directory(&project.path())?)
        {
            continue;
        }
        let chats = project.path().join("chats");
        if super::codex::optional_directory(&chats)? {
            roots.push(chats);
        }
    }
    roots.sort();
    Ok(roots)
}

pub(crate) fn is_session_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|value| value.to_str()),
        Some("jsonl" | "json")
    )
}

pub(crate) fn scan_session(path: &Path) -> Result<SessionSummary> {
    let file = File::open(path).context("open Gemini session")?;
    let metadata = file.metadata().context("inspect Gemini session")?;
    drop(file);
    let parsed = parse_session(path)?;
    ensure_unchanged(path, &metadata)?;
    let external_id = parsed
        .metadata
        .get("sessionId")
        .and_then(Value::as_str)
        .context("Gemini session metadata missing sessionId")?
        .to_owned();
    ensure!(
        parsed
            .metadata
            .get("projectHash")
            .and_then(Value::as_str)
            .is_some(),
        "Gemini session metadata missing projectHash"
    );
    let started_at = string_field(&parsed.metadata, "startTime");
    let cwd = parsed
        .metadata
        .get("directories")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| workspace_from_session_context(&parsed.messages));
    let model = parsed
        .messages
        .iter()
        .rev()
        .find(|message| message["type"] == "gemini")
        .and_then(|message| message["model"].as_str())
        .map(str::to_owned);
    let derived_title = parsed
        .metadata
        .get("summary")
        .and_then(Value::as_str)
        .and_then(|value| sanitized_title(value, 80))
        .or_else(|| {
            parsed
                .messages
                .iter()
                .filter(|message| message["type"] == "user")
                .filter_map(|message| content_text(&message["content"]))
                .find(|content| !is_internal_user_content(content))
                .and_then(|content| sanitized_title(&content, 80))
        });
    let raw_metadata = Value::Object(parsed.metadata.clone()).to_string();
    let content_hash = hash_file(path)?;
    ensure_unchanged(path, &metadata)?;
    Ok(SessionSummary {
        metadata,
        content_hash,
        external_id,
        started_at,
        // Gemini records when the file was last updated, but does not expose a
        // terminal lifecycle event. Do not turn file activity into completion.
        ended_at: None,
        cwd,
        model,
        status: "unknown",
        raw_metadata,
        derived_title,
        malformed_lines: parsed.malformed_lines,
        parse_error: parsed.parse_error,
    })
}

fn parse_session(path: &Path) -> Result<ParsedSession> {
    let mut metadata = Map::new();
    let mut messages = MessageSet::default();
    let mut malformed_lines = 0usize;
    let mut examples = Vec::new();
    if path.extension().and_then(|value| value.to_str()) == Some("json") {
        let value: Value =
            serde_json::from_reader(File::open(path).context("open legacy Gemini session")?)
                .context("parse legacy Gemini session")?;
        apply_record(value, &mut metadata, &mut messages);
    } else {
        let mut reader = BufReader::new(File::open(path).context("open Gemini session log")?);
        let mut line = Vec::new();
        let mut line_number = 0usize;
        loop {
            line.clear();
            if reader
                .read_until(b'\n', &mut line)
                .context("read Gemini session line")?
                == 0
            {
                break;
            }
            line_number = line_number
                .checked_add(1)
                .context("Gemini line count exceeds supported range")?;
            match serde_json::from_slice(&line) {
                Ok(value) => apply_record(value, &mut metadata, &mut messages),
                Err(_) => {
                    malformed_lines = malformed_lines
                        .checked_add(1)
                        .context("malformed-line count exceeds supported range")?;
                    if examples.len() < MAX_PARSE_DIAGNOSTICS {
                        examples.push(format!("line {line_number}: invalid JSON or UTF-8"));
                    }
                }
            }
        }
    }
    let parse_error = if malformed_lines == 0 {
        None
    } else {
        let omitted = malformed_lines - examples.len();
        let mut message = examples.join("; ");
        if omitted != 0 {
            message.push_str(&format!("; {omitted} additional malformed lines omitted"));
        }
        Some(message)
    };
    Ok(ParsedSession {
        metadata,
        messages: messages.values,
        malformed_lines,
        parse_error,
    })
}

fn apply_record(value: Value, metadata: &mut Map<String, Value>, messages: &mut MessageSet) {
    if let Some(rewind) = value.get("$rewindTo").and_then(Value::as_str) {
        if let Some(index) = messages.positions.get(rewind).copied() {
            messages.values.truncate(index);
            messages.positions.retain(|_, position| *position < index);
        } else {
            messages.values.clear();
            messages.positions.clear();
        }
        return;
    }
    if let Some(update) = value.get("$set").and_then(Value::as_object) {
        if let Some(checkpoint) = update.get("messages").and_then(Value::as_array) {
            messages.values.clear();
            messages.positions.clear();
            for message in checkpoint
                .iter()
                .filter(|message| message["id"].is_string())
            {
                upsert_message(messages, message.clone());
            }
        }
        merge_metadata(metadata, update);
        return;
    }
    if value["id"].is_string() {
        upsert_message(messages, value);
        return;
    }
    if let Some(object) = value.as_object()
        && object.get("sessionId").and_then(Value::as_str).is_some()
        && object.get("projectHash").and_then(Value::as_str).is_some()
    {
        if let Some(initial) = object.get("messages").and_then(Value::as_array) {
            for message in initial.iter().filter(|message| message["id"].is_string()) {
                upsert_message(messages, message.clone());
            }
        }
        merge_metadata(metadata, object);
    }
}

fn merge_metadata(metadata: &mut Map<String, Value>, update: &Map<String, Value>) {
    for (key, value) in update {
        if key != "messages" {
            metadata.insert(key.clone(), value.clone());
        }
    }
}

fn upsert_message(messages: &mut MessageSet, value: Value) {
    let id = value["id"].as_str().unwrap_or_default().to_owned();
    if let Some(position) = messages.positions.get(&id).copied() {
        messages.values[position] = value;
    } else {
        let position = messages.values.len();
        messages.values.push(value);
        messages.positions.insert(id, position);
    }
}

fn normalize_message(
    message: &Value,
    legacy_results: &mut HashSet<String>,
) -> Vec<NormalizedRecord> {
    let mut records = Vec::new();
    let timestamp = message["timestamp"].as_str().map(str::to_owned);
    let raw_json = message.to_string();
    match message["type"].as_str() {
        Some("user") => {
            let content = content_text(&message["displayContent"])
                .or_else(|| content_text(&message["content"]))
                .filter(|content| !content.is_empty());
            if let Some(content) = content {
                records.push(NormalizedRecord::Message {
                    timestamp: timestamp.clone(),
                    role: "user".into(),
                    opens_turn: !is_internal_user_content(&content),
                    content,
                    raw_json: raw_json.clone(),
                });
            }
            for part in content_parts(&message["content"]) {
                let Some(response) = part.get("functionResponse") else {
                    continue;
                };
                let call_id = response["id"].as_str().map(str::to_owned);
                if call_id
                    .as_ref()
                    .is_some_and(|call_id| legacy_results.contains(call_id))
                {
                    continue;
                }
                records.push(NormalizedRecord::ToolOutput {
                    timestamp: timestamp.clone(),
                    call_id,
                    output: value_text(
                        response
                            .get("response")
                            .and_then(|response| response.get("output"))
                            .or_else(|| response.get("response")),
                    ),
                    outcome: "observed",
                    raw_json: part.to_string(),
                });
            }
        }
        Some("info" | "warning" | "error") => records.push(NormalizedRecord::Message {
            timestamp,
            role: message["type"].as_str().unwrap_or("info").to_owned(),
            content: content_text(&message["displayContent"])
                .or_else(|| content_text(&message["content"]))
                .unwrap_or_default(),
            opens_turn: false,
            raw_json,
        }),
        Some("gemini") => {
            if let Some(content) = content_text(&message["displayContent"])
                .or_else(|| content_text(&message["content"]))
                .filter(|content| !content.is_empty())
            {
                records.push(NormalizedRecord::Message {
                    timestamp: timestamp.clone(),
                    role: "assistant".into(),
                    content,
                    opens_turn: false,
                    raw_json: raw_json.clone(),
                });
            }
            let modern_calls: Vec<&Value> = content_parts(&message["content"])
                .filter_map(|part| part.get("functionCall"))
                .collect();
            if modern_calls.is_empty() {
                normalize_legacy_tools(message, &timestamp, &mut records, legacy_results, true);
            } else {
                for call in modern_calls {
                    records.push(NormalizedRecord::ToolCall {
                        timestamp: timestamp.clone(),
                        call_id: call["id"].as_str().map(str::to_owned),
                        tool_name: call["name"].as_str().map(str::to_owned),
                        arguments: value_text(call.get("args")),
                        kind: "tool",
                        raw_json: call.to_string(),
                    });
                }
                normalize_legacy_tools(message, &timestamp, &mut records, legacy_results, false);
            }
            if message["tokens"].is_object() {
                let tokens = &message["tokens"];
                records.push(NormalizedRecord::Usage {
                    timestamp,
                    cumulative: None,
                    per_turn: Some(TokenUsage {
                        input_tokens: tokens["input"].as_i64(),
                        output_tokens: tokens["output"].as_i64(),
                        cached_input_tokens: tokens["cached"].as_i64(),
                        reasoning_output_tokens: tokens["thoughts"].as_i64(),
                        total_tokens: tokens["total"].as_i64(),
                    }),
                    pressure: Box::<PressureSnapshot>::default(),
                    raw_json,
                });
            }
        }
        _ => {}
    }
    records
}

fn normalize_legacy_tools(
    message: &Value,
    timestamp: &Option<String>,
    records: &mut Vec<NormalizedRecord>,
    legacy_results: &mut HashSet<String>,
    emit_calls: bool,
) {
    if let Some(tools) = message["toolCalls"].as_array() {
        for tool in tools {
            let call_id = tool["id"].as_str().map(str::to_owned);
            if emit_calls {
                records.push(NormalizedRecord::ToolCall {
                    timestamp: tool["timestamp"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| timestamp.clone()),
                    call_id: call_id.clone(),
                    tool_name: tool["name"].as_str().map(str::to_owned),
                    arguments: value_text(tool.get("args")),
                    kind: "tool",
                    raw_json: tool.to_string(),
                });
            }
            if let Some(result) = tool.get("result").filter(|result| !result.is_null()) {
                if let Some(call_id) = &call_id {
                    legacy_results.insert(call_id.clone());
                }
                records.push(NormalizedRecord::ToolOutput {
                    timestamp: tool["timestamp"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| timestamp.clone()),
                    call_id,
                    output: value_text(Some(result)),
                    outcome: if matches!(
                        tool["status"].as_str(),
                        Some("error" | "failed" | "cancelled" | "canceled")
                    ) {
                        "failed"
                    } else {
                        "observed"
                    },
                    raw_json: tool.to_string(),
                });
            }
        }
    }
}

fn content_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Array(parts) => {
            let values: Vec<&str> = parts
                .iter()
                .filter_map(|part| {
                    part.as_str().or_else(|| {
                        (part.get("thought").and_then(Value::as_bool) != Some(true))
                            .then(|| part.get("text").and_then(Value::as_str))
                            .flatten()
                    })
                })
                .collect();
            (!values.is_empty()).then(|| values.join("\n"))
        }
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

fn content_parts(value: &Value) -> impl Iterator<Item = &Value> {
    let parts: &[Value] = match value {
        Value::Array(parts) => parts,
        Value::Object(_) => std::slice::from_ref(value),
        _ => &[],
    };
    parts.iter()
}

fn is_internal_user_content(content: &str) -> bool {
    let content = content.trim_start();
    content.starts_with("<session_context>") || content.starts_with("<hook_context>")
}

fn workspace_from_session_context(messages: &[Value]) -> Option<String> {
    messages
        .iter()
        .filter(|message| message["type"] == "user")
        .filter_map(|message| content_text(&message["content"]))
        .find_map(|content| {
            if !content.trim_start().starts_with("<session_context>") {
                return None;
            }
            let mut lines = content.lines();
            lines.find(|line| line.trim() == "- **Workspace Directories:**")?;
            lines
                .map(str::trim)
                .take_while(|line| !line.starts_with("- **"))
                .find_map(|line| {
                    line.strip_prefix("- ")
                        .map(str::trim)
                        .filter(|path| !path.is_empty())
                        .map(str::to_owned)
                })
        })
}

fn value_text(value: Option<&Value>) -> Option<String> {
    let value = value?;
    content_text(value).or_else(|| (!value.is_null()).then(|| value.to_string()))
}

fn string_field(metadata: &Map<String, Value>, key: &str) -> Option<String> {
    metadata.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn sanitized_title(value: &str, max_chars: usize) -> Option<String> {
    if value
        .chars()
        .any(|character| character.is_control() && !character.is_whitespace())
    {
        return None;
    }
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let title: String = normalized.chars().take(max_chars).collect();
    (!title.is_empty()).then_some(title)
}

pub(crate) fn classify_message(role: &str, content: &str) -> Option<MessagePresentation> {
    if role == "user" {
        let content = content.trim_start();
        if content.starts_with("<session_context>") {
            return Some(MessagePresentation {
                title: "Gemini session context",
                initially_open: false,
            });
        }
        if content.starts_with("<hook_context>") {
            return Some(MessagePresentation {
                title: "Gemini hook context",
                initially_open: false,
            });
        }
    }
    match role {
        "info" => Some(MessagePresentation {
            title: "Gemini information",
            initially_open: false,
        }),
        "warning" => Some(MessagePresentation {
            title: "Gemini warning",
            initially_open: true,
        }),
        "error" => Some(MessagePresentation {
            title: "Gemini error",
            initially_open: true,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_direct_and_host_user_homes() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("host/user/.gemini/tmp/project/chats");
        std::fs::create_dir_all(&home).unwrap();
        let discovered = discover(dir.path()).unwrap();
        assert_eq!(discovered.homes.len(), 1);
        assert_eq!(discovered.homes[0].adapter, Adapter::Gemini);
        assert_eq!(chat_roots(&discovered.homes[0].path).unwrap().len(), 1);
        assert_eq!(
            discover(&discovered.homes[0].path).unwrap().database_layout,
            DIRECT_HOME_LAYOUT
        );
    }

    #[test]
    fn checkpoints_unknowns_and_malformed_lines_are_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"sessionId\":\"synthetic\",\"projectHash\":\"project\",\"startTime\":\"2026-01-01T00:00:00Z\"}\n",
                "{\"id\":\"old\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"type\":\"user\",\"content\":\"old\"}\n",
                "private malformed bytes\n",
                "{\"$set\":{\"messages\":[{\"id\":\"kept\",\"timestamp\":\"2026-01-01T00:00:02Z\",\"type\":\"gemini\",\"content\":\"kept\",\"tokens\":{\"input\":1}}],\"future\":true}}\n",
                "{\"futureRecord\":true}\n"
            ),
        )
        .unwrap();
        let summary = scan_session(&path).unwrap();
        assert_eq!(summary.malformed_lines, 1);
        assert_eq!(
            summary.parse_error.as_deref(),
            Some("line 3: invalid JSON or UTF-8")
        );
        assert!(!summary.parse_error.unwrap().contains("private"));
        let mut records = SessionRecords::open(&path, &summary.metadata).unwrap();
        let (_, NormalizedRecord::Message { content, .. }) = records.next_record().unwrap() else {
            panic!("checkpointed Gemini message was not retained");
        };
        assert_eq!(content, "kept");
        assert!(matches!(
            records.next_record(),
            Some((_, NormalizedRecord::Usage { .. }))
        ));
        assert!(records.next_record().is_none());
        assert_eq!(records.malformed_lines(), 1);
    }

    #[test]
    fn legacy_json_sessions_are_supported_without_inventing_completion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        std::fs::write(
            &path,
            r#"{"sessionId":"legacy-synthetic","projectHash":"project","startTime":"2026-01-01T00:00:00Z","lastUpdated":"2026-01-01T00:01:00Z","messages":[{"id":"one","timestamp":"2026-01-01T00:00:30Z","type":"user","content":"synthetic legacy message"}]}"#,
        )
        .unwrap();

        let summary = scan_session(&path).unwrap();
        assert_eq!(summary.external_id, "legacy-synthetic");
        assert_eq!(summary.status, "unknown");
        assert_eq!(summary.ended_at, None);
        let mut records = SessionRecords::open(&path, &summary.metadata).unwrap();
        let (_, NormalizedRecord::Message { content, .. }) = records.next_record().unwrap() else {
            panic!("legacy Gemini message was not retained");
        };
        assert_eq!(content, "synthetic legacy message");
    }

    #[test]
    fn content_variants_preserve_string_and_text_parts() {
        assert_eq!(
            content_text(&serde_json::json!([
                "synthetic string part",
                {"text":"synthetic object part"},
                {"unknown":true}
            ]))
            .as_deref(),
            Some("synthetic string part\nsynthetic object part")
        );
        assert_eq!(
            content_parts(&serde_json::json!({"functionCall":{"name":"synthetic"}})).count(),
            1
        );
        assert_eq!(
            classify_message("user", "  <hook_context>synthetic</hook_context>")
                .unwrap()
                .title,
            "Gemini hook context"
        );
    }

    #[test]
    fn current_parts_preserve_tools_without_creating_internal_user_turns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"sessionId\":\"modern-synthetic\",\"projectHash\":\"project\",\"startTime\":\"2026-01-01T00:00:00Z\"}\n",
                "{\"$set\":{\"messages\":[",
                "{\"id\":\"context\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"user\",\"content\":[{\"text\":\"<session_context>\\n- **Workspace Directories:**\\n  - /synthetic/modern\\n</session_context>\"}]},",
                "{\"id\":\"prompt\",\"timestamp\":\"2026-01-01T00:00:01Z\",\"type\":\"user\",\"content\":[{\"text\":\"Synthetic modern request\"}]},",
                "{\"id\":\"call\",\"timestamp\":\"2026-01-01T00:00:02Z\",\"type\":\"gemini\",\"content\":[{\"text\":\"private thought\",\"thought\":true},{\"functionCall\":{\"id\":\"call-modern\",\"name\":\"modern_tool\",\"args\":{\"value\":1}}}]},",
                "{\"id\":\"response\",\"timestamp\":\"2026-01-01T00:00:03Z\",\"type\":\"user\",\"content\":[{\"functionResponse\":{\"id\":\"call-modern\",\"name\":\"modern_tool\",\"response\":{\"output\":\"Synthetic modern output\"}}}]},",
                "{\"id\":\"answer\",\"timestamp\":\"2026-01-01T00:00:04Z\",\"type\":\"gemini\",\"model\":\"gemini-modern\",\"content\":[{\"text\":\"Synthetic modern answer\"}]}",
                "]}}\n"
            ),
        )
        .unwrap();

        let summary = scan_session(&path).unwrap();
        assert_eq!(summary.cwd.as_deref(), Some("/synthetic/modern"));
        assert_eq!(summary.model.as_deref(), Some("gemini-modern"));
        assert_eq!(
            summary.derived_title.as_deref(),
            Some("Synthetic modern request")
        );

        let mut records = SessionRecords::open(&path, &summary.metadata).unwrap();
        let mut messages = Vec::new();
        let mut calls = Vec::new();
        let mut outputs = Vec::new();
        while let Some((_, record)) = records.next_record() {
            match record {
                NormalizedRecord::Message {
                    content,
                    opens_turn,
                    ..
                } => messages.push((content, opens_turn)),
                NormalizedRecord::ToolCall {
                    call_id, tool_name, ..
                } => calls.push((call_id, tool_name)),
                NormalizedRecord::ToolOutput {
                    call_id, output, ..
                } => outputs.push((call_id, output)),
                _ => {}
            }
        }
        assert_eq!(
            messages,
            [
                (
                    "<session_context>\n- **Workspace Directories:**\n  - /synthetic/modern\n</session_context>".into(),
                    false
                ),
                ("Synthetic modern request".into(), true),
                ("Synthetic modern answer".into(), false),
            ]
        );
        assert_eq!(
            calls,
            [(Some("call-modern".into()), Some("modern_tool".into()))]
        );
        assert_eq!(
            outputs,
            [(
                Some("call-modern".into()),
                Some("Synthetic modern output".into())
            )]
        );
    }

    #[test]
    fn legacy_embedded_result_wins_over_repeated_function_response() {
        let message = serde_json::json!({
            "id": "legacy",
            "timestamp": "2026-01-01T00:00:00Z",
            "type": "gemini",
            "content": "",
            "toolCalls": [{
                "id": "legacy-call",
                "name": "legacy_tool",
                "args": {},
                "result": [{"text": "Synthetic legacy output"}],
                "status": "success"
            }]
        });
        let response = serde_json::json!({
            "id": "response",
            "timestamp": "2026-01-01T00:00:01Z",
            "type": "user",
            "content": [{"functionResponse": {
                "id": "legacy-call",
                "name": "legacy_tool",
                "response": {"output": "Synthetic legacy output"}
            }}]
        });
        let mut emitted = HashSet::new();
        let mut records = normalize_message(&message, &mut emitted);
        records.extend(normalize_message(&response, &mut emitted));
        assert_eq!(
            records
                .iter()
                .filter(|record| matches!(record, NormalizedRecord::ToolOutput { .. }))
                .count(),
            1
        );
        assert!(
            !records
                .iter()
                .any(|record| matches!(record, NormalizedRecord::Message { .. }))
        );
    }
}
