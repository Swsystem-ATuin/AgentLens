use super::{
    Adapter, DiscoveredHome, DiscoveredSources, MessagePresentation, NormalizedRecord,
    SessionSummary, SourceLayout,
};
use crate::fingerprint::{ensure_unchanged, hash_file};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs::{File, Metadata},
    path::{Path, PathBuf},
};

pub(crate) const KIND: &str = "hermes";
pub(crate) const DISPLAY_NAME: &str = "Hermes Agent";
pub(crate) const PARSER_VERSION: i64 = 1;
pub(crate) const SOURCE_AREAS: [&str; 1] = ["sessions"];
const HOST_USER_LAYOUT: &str = "host-user";
const DIRECT_HOME_LAYOUT: &str = "direct-hermes-home";

pub(crate) struct SessionRecords {
    events: VecDeque<(i64, NormalizedRecord)>,
}

impl SessionRecords {
    pub(crate) fn open(path: &Path, metadata: &Metadata) -> Result<Self> {
        ensure_unchanged(path, metadata)?;
        let snapshot = parse_snapshot(path)?;
        ensure_unchanged(path, metadata)?;
        let events = normalize_snapshot(&snapshot)
            .into_iter()
            .enumerate()
            .map(|(sequence, record)| {
                i64::try_from(sequence)
                    .map(|sequence| (sequence, record))
                    .context("Hermes event count exceeds supported range")
            })
            .collect::<Result<VecDeque<_>>>()?;
        Ok(Self { events })
    }

    pub(crate) fn next_record(&mut self) -> Option<(i64, NormalizedRecord)> {
        self.events.pop_front()
    }
}

pub(crate) fn discover(root: &Path) -> Result<DiscoveredSources> {
    ensure!(
        super::codex::optional_directory(root)?,
        "source root is missing; refresh cancelled"
    );
    if root.file_name() == Some(std::ffi::OsStr::new(".hermes")) {
        return Ok(DiscoveredSources {
            layout: SourceLayout::DirectHome,
            database_layout: DIRECT_HOME_LAYOUT,
            homes: vec![DiscoveredHome {
                adapter: Adapter::Hermes,
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
            let path = user.path().join(".hermes");
            if super::codex::optional_directory(&path)? {
                homes.push(DiscoveredHome {
                    adapter: Adapter::Hermes,
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

pub(crate) fn session_root(home: &Path) -> Result<Option<PathBuf>> {
    let sessions = home.join("sessions");
    Ok(super::codex::optional_directory(&sessions)?.then_some(sessions))
}

pub(crate) fn is_session_file(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("json")
        && path
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.starts_with("request_dump_"))
}

pub(crate) fn scan_session(path: &Path) -> Result<SessionSummary> {
    let file = File::open(path).context("open Hermes request snapshot")?;
    let metadata = file.metadata().context("inspect Hermes request snapshot")?;
    drop(file);
    let snapshot = parse_snapshot(path)?;
    ensure_unchanged(path, &metadata)?;
    let native_id = snapshot
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .context("Hermes request snapshot missing session_id")?;
    let body = request_body(&snapshot)?;
    let model = body.get("model").and_then(Value::as_str).map(str::to_owned);
    let started_at = snapshot
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let derived_title = request_items(body)
        .find(|item| item.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|item| content_text(item.get("content")?))
        .and_then(|content| sanitized_title(&content, 80));
    let filename = path
        .file_name()
        .map(|value| value.to_string_lossy())
        .unwrap_or_default();
    let filename_hash = blake3::hash(filename.as_bytes()).to_hex().to_string();
    // Request dumps are provider-attempt snapshots, not the canonical Hermes
    // session store. Keep retries distinct while retaining their native scope.
    let external_id = format!("{native_id}:request:{}", &filename_hash[..16]);
    let reason = safe_error_reason(snapshot.get("reason").and_then(Value::as_str)).0;
    let raw_metadata = json!({
        "type": "hermes_request_snapshot",
        "timestamp": started_at,
        "model": model,
        "reason": reason,
    })
    .to_string();
    let content_hash = hash_file(path)?;
    ensure_unchanged(path, &metadata)?;
    Ok(SessionSummary {
        metadata,
        content_hash,
        external_id,
        started_at,
        ended_at: None,
        cwd: None,
        model,
        status: "unknown",
        raw_metadata,
        derived_title,
        malformed_lines: 0,
        parse_error: None,
    })
}

fn parse_snapshot(path: &Path) -> Result<Value> {
    let value: Value = serde_json::from_reader(
        File::open(path).context("open Hermes request snapshot for parsing")?,
    )
    .context("parse Hermes request snapshot")?;
    ensure!(
        value.is_object(),
        "Hermes request snapshot is not an object"
    );
    request_body(&value)?;
    Ok(value)
}

fn request_body(snapshot: &Value) -> Result<&serde_json::Map<String, Value>> {
    snapshot
        .get("request")
        .and_then(|request| request.get("body"))
        .and_then(Value::as_object)
        .context("Hermes request snapshot missing request body")
}

fn request_items(body: &serde_json::Map<String, Value>) -> impl Iterator<Item = &Value> {
    body.get("input")
        .or_else(|| body.get("messages"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn normalize_snapshot(snapshot: &Value) -> Vec<NormalizedRecord> {
    let Some(body) = snapshot
        .get("request")
        .and_then(|request| request.get("body"))
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let mut records = Vec::new();
    if let Some(instructions) = body
        .get("instructions")
        .or_else(|| body.get("system"))
        .and_then(content_text)
        .filter(|content| !content.is_empty())
    {
        records.push(NormalizedRecord::Message {
            timestamp: None,
            role: "developer".into(),
            content: instructions.clone(),
            opens_turn: false,
            raw_json: json!({"type":"instructions","content":instructions}).to_string(),
        });
    }
    for item in request_items(body) {
        normalize_item(item, &mut records);
    }
    let timestamp = snapshot
        .get("timestamp")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let (reason, message) = safe_error_reason(snapshot.get("reason").and_then(Value::as_str));
    records.push(NormalizedRecord::Message {
        timestamp,
        role: "error".into(),
        content: message.into(),
        opens_turn: false,
        raw_json: json!({"type":"request_error","reason":reason}).to_string(),
    });
    records
}

fn normalize_item(item: &Value, records: &mut Vec<NormalizedRecord>) {
    match item.get("type").and_then(Value::as_str) {
        Some("function_call") => push_tool_call(item, records),
        Some("function_call_output") => push_tool_output(item, records),
        Some("reasoning") => {}
        _ => {
            if let Some(role) = item.get("role").and_then(Value::as_str)
                && matches!(role, "user" | "assistant" | "system" | "developer")
            {
                if let Some(content) = item
                    .get("content")
                    .and_then(content_text)
                    .filter(|content| !content.is_empty())
                {
                    records.push(NormalizedRecord::Message {
                        timestamp: None,
                        role: if role == "system" {
                            "developer".into()
                        } else {
                            role.into()
                        },
                        content: content.clone(),
                        opens_turn: role == "user",
                        raw_json: json!({
                            "type": item.get("type").and_then(Value::as_str),
                            "role": role,
                            "content": content,
                        })
                        .to_string(),
                    });
                }
                if let Some(parts) = item.get("content").and_then(Value::as_array) {
                    for part in parts {
                        match part.get("type").and_then(Value::as_str) {
                            Some("tool_use") => push_tool_call(part, records),
                            Some("tool_result") => push_tool_output(part, records),
                            _ => {}
                        }
                    }
                }
                if let Some(calls) = item.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        push_tool_call(call, records);
                    }
                }
            }
        }
    }
}

fn push_tool_call(item: &Value, records: &mut Vec<NormalizedRecord>) {
    let function = item.get("function").unwrap_or(item);
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let tool_name = function
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let arguments = function
        .get("arguments")
        .or_else(|| function.get("input"))
        .and_then(value_text);
    records.push(NormalizedRecord::ToolCall {
        timestamp: None,
        call_id: call_id.clone(),
        tool_name: tool_name.clone(),
        arguments: arguments.clone(),
        kind: "tool",
        raw_json: json!({
            "type":"function_call",
            "call_id":call_id,
            "name":tool_name,
            "arguments":arguments,
        })
        .to_string(),
    });
}

fn push_tool_output(item: &Value, records: &mut Vec<NormalizedRecord>) {
    let call_id = item
        .get("call_id")
        .or_else(|| item.get("tool_use_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let output = item
        .get("output")
        .or_else(|| item.get("content"))
        .and_then(value_text);
    let failed = item.get("is_error").and_then(Value::as_bool) == Some(true)
        || matches!(
            item.get("status").and_then(Value::as_str),
            Some("error" | "failed" | "cancelled" | "canceled")
        );
    records.push(NormalizedRecord::ToolOutput {
        timestamp: None,
        call_id: call_id.clone(),
        output: output.clone(),
        outcome: if failed { "failed" } else { "observed" },
        raw_json: json!({
            "type":"function_call_output",
            "call_id":call_id,
            "output":output,
            "outcome":if failed { "failed" } else { "observed" },
        })
        .to_string(),
    });
}

fn content_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Array(parts) => {
            let values = parts
                .iter()
                .filter_map(|part| {
                    part.as_str()
                        .or_else(|| part.get("text").and_then(Value::as_str))
                })
                .collect::<Vec<_>>();
            (!values.is_empty()).then(|| values.join("\n"))
        }
        Value::Object(object) => object
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

fn value_text(value: &Value) -> Option<String> {
    content_text(value).or_else(|| (!value.is_null()).then(|| value.to_string()))
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

fn safe_error_reason(reason: Option<&str>) -> (&'static str, &'static str) {
    match reason {
        Some("max_retries_exhausted") => (
            "max_retries_exhausted",
            "Hermes provider request failed after retry attempts.",
        ),
        Some("non_retryable_client_error") => (
            "non_retryable_client_error",
            "Hermes provider request failed with a non-retryable client error.",
        ),
        Some("usage_limit_reached") => (
            "usage_limit_reached",
            "Hermes provider request failed because an observed usage limit was reached.",
        ),
        _ => ("unknown", "Hermes provider request failed."),
    }
}

pub(crate) fn classify_message(role: &str) -> Option<MessagePresentation> {
    match role {
        "developer" => Some(MessagePresentation {
            title: "Hermes instructions",
            initially_open: false,
        }),
        "error" => Some(MessagePresentation {
            title: "Hermes request failure",
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
        let sessions = dir.path().join("host/user/.hermes/sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let discovered = discover(dir.path()).unwrap();
        assert_eq!(discovered.homes.len(), 1);
        assert_eq!(discovered.homes[0].adapter, Adapter::Hermes);
        assert!(session_root(&discovered.homes[0].path).unwrap().is_some());
        assert_eq!(
            discover(&discovered.homes[0].path).unwrap().database_layout,
            DIRECT_HOME_LAYOUT
        );
        assert!(is_session_file(
            &sessions.join("request_dump_synthetic.json")
        ));
        assert!(!is_session_file(&sessions.join("sessions.json")));
    }

    #[test]
    fn request_snapshots_normalize_supported_variants_and_redact_transport_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("request_dump_synthetic.json");
        std::fs::write(
            &path,
            include_bytes!("../../tests/fixtures/hermes/request_dump.json"),
        )
        .unwrap();
        let summary = scan_session(&path).unwrap();
        assert_eq!(summary.model.as_deref(), Some("synthetic-hermes-model"));
        assert_eq!(summary.status, "unknown");
        assert_eq!(summary.ended_at, None);
        assert_eq!(
            summary.derived_title.as_deref(),
            Some("Synthetic Hermes request")
        );
        assert!(!summary.raw_metadata.contains("PRIVATE_SENTINEL"));

        let mut records = SessionRecords::open(&path, &summary.metadata).unwrap();
        let mut normalized = Vec::new();
        while let Some((_, record)) = records.next_record() {
            normalized.push(record);
        }
        assert_eq!(normalized.len(), 7);
        assert!(matches!(
            &normalized[0],
            NormalizedRecord::Message { role, content, .. }
                if role == "developer" && content == "Synthetic Hermes instructions"
        ));
        assert!(matches!(
            &normalized[1],
            NormalizedRecord::Message { role, content, opens_turn: true, .. }
                if role == "user" && content == "Synthetic Hermes request"
        ));
        assert!(matches!(
            &normalized[3],
            NormalizedRecord::ToolCall { tool_name, .. }
                if tool_name.as_deref() == Some("synthetic_tool")
        ));
        assert!(matches!(
            &normalized[4],
            NormalizedRecord::ToolOutput {
                outcome: "observed",
                ..
            }
        ));
        assert!(matches!(
            &normalized[6],
            NormalizedRecord::Message { role, content, .. }
                if role == "error" && content.contains("retry attempts")
        ));
        let stored = normalized
            .into_iter()
            .map(|record| match record {
                NormalizedRecord::Message { raw_json, .. }
                | NormalizedRecord::ToolCall { raw_json, .. }
                | NormalizedRecord::ToolOutput { raw_json, .. }
                | NormalizedRecord::Usage { raw_json, .. }
                | NormalizedRecord::TurnEvent { raw_json, .. } => raw_json,
            })
            .collect::<String>();
        assert!(!stored.contains("PRIVATE_SENTINEL"));
    }

    #[test]
    fn anthropic_blocks_and_unknown_fields_are_tolerated() {
        let snapshot = json!({
            "session_id":"synthetic-anthropic",
            "timestamp":"2026-01-01T00:00:03Z",
            "reason":"future_reason",
            "request":{"body":{
                "model":"synthetic",
                "system":[{"type":"text","text":"Synthetic system","future":true}],
                "messages":[
                    {"role":"user","content":[{"type":"text","text":"Synthetic question"}]},
                    {"role":"assistant","content":[
                        {"type":"text","text":"Synthetic response"},
                        {"type":"tool_use","id":"call-1","name":"lookup","input":{"key":"value"}}
                    ]},
                    {"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":"Synthetic result","is_error":true}]}
                ],
                "future":true
            }},
            "error":{"message":"PRIVATE_SENTINEL_ERROR"},
            "future":true
        });
        let records = normalize_snapshot(&snapshot);
        assert_eq!(records.len(), 6);
        assert!(records.iter().any(|record| matches!(
            record,
            NormalizedRecord::ToolOutput {
                outcome: "failed",
                ..
            }
        )));
        assert!(matches!(
            records.last(),
            Some(NormalizedRecord::Message { content, .. }) if content == "Hermes provider request failed."
        ));
    }
}
