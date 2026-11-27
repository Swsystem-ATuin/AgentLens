use super::{
    Adapter, DiscoveredHome, DiscoveredSources, MessagePresentation, NormalizedRecord,
    PressureSnapshot, SessionSummary, SourceLayout, TokenUsage,
};
use crate::fingerprint::{ensure_unchanged, hash_file};
use anyhow::{Context, Result, ensure};
use futures_util::TryStreamExt;
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection, sqlite::SqliteConnectOptions};
use std::{
    collections::{HashSet, VecDeque},
    fs::{File, Metadata},
    path::{Path, PathBuf},
};

pub(crate) const KIND: &str = "hermes";
pub(crate) const DISPLAY_NAME: &str = "Hermes Agent";
pub(crate) const PARSER_VERSION: i64 = 5;
pub(crate) const SOURCE_AREAS: [&str; 3] = ["state", "cron", "sessions"];
const HOST_USER_LAYOUT: &str = "host-user";
const DIRECT_HOME_LAYOUT: &str = "direct-hermes-home";

pub(crate) struct StateReader {
    connection: SqliteConnection,
    metadata: Metadata,
    session_columns: HashSet<String>,
    message_columns: HashSet<String>,
    scope: String,
    #[cfg(windows)]
    _snapshot_guard: File,
}

pub(crate) struct StateScan {
    pub(crate) content_hash: String,
    pub(crate) metadata: Metadata,
    pub(crate) sessions: Vec<StateSession>,
}

pub(crate) struct StateSession {
    pub(crate) native_id: String,
    pub(crate) external_id: String,
    pub(crate) started_at: Option<String>,
    pub(crate) ended_at: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) status: &'static str,
    pub(crate) title: Option<String>,
    pub(crate) raw_metadata: String,
    pub(crate) usage: Option<TokenUsage>,
}

pub(crate) struct SessionRecords {
    events: VecDeque<(i64, NormalizedRecord)>,
}

impl SessionRecords {
    pub(crate) fn open(path: &Path, metadata: &Metadata) -> Result<Self> {
        ensure_unchanged(path, metadata)?;
        if is_cron_output(path) {
            let content = std::fs::read_to_string(path).context("read Hermes cron output")?;
            ensure_unchanged(path, metadata)?;
            let timestamp = cron_output_timestamp(path);
            let mut events = VecDeque::new();
            events.push_back((
                0,
                NormalizedRecord::Message {
                    timestamp,
                    role: "assistant".into(),
                    content: content.clone(),
                    opens_turn: false,
                    raw_json: json!({"type":"cron_output","content":content}).to_string(),
                },
            ));
            return Ok(Self { events });
        }
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

impl StateReader {
    pub(crate) async fn open(path: &Path, home: &Path) -> Result<Self> {
        #[cfg(windows)]
        let snapshot_guard = open_windows_snapshot(path)?;
        let metadata = std::fs::metadata(path).context("inspect Hermes state database")?;
        ensure!(metadata.is_file(), "Hermes state database is not a file");
        let options = SqliteConnectOptions::new()
            .filename(path)
            .read_only(true)
            .create_if_missing(false);
        #[cfg(windows)]
        let options = options.immutable(true);
        let mut connection = SqliteConnection::connect_with(&options)
            .await
            .context("open Hermes state database")?;
        sqlx::query("BEGIN DEFERRED")
            .execute(&mut connection)
            .await
            .context("start Hermes state snapshot")?;
        let session_columns = table_columns(&mut connection, "sessions").await?;
        let message_columns = table_columns(&mut connection, "messages").await?;
        for (table, columns, required) in [
            (
                "sessions",
                &session_columns,
                &["id", "source", "started_at"][..],
            ),
            (
                "messages",
                &message_columns,
                &["id", "session_id", "role"][..],
            ),
        ] {
            for column in required {
                ensure!(
                    columns.contains(*column),
                    "Hermes {table} table missing required {column} column"
                );
            }
        }
        let relative = path.strip_prefix(home).unwrap_or(path);
        let scope = if relative == Path::new("state.db") {
            String::new()
        } else {
            relative
                .parent()
                .and_then(Path::file_name)
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| "profile".into())
        };
        Ok(Self {
            connection,
            metadata,
            session_columns,
            message_columns,
            scope,
            #[cfg(windows)]
            _snapshot_guard: snapshot_guard,
        })
    }

    pub(crate) async fn scan(&mut self) -> Result<StateScan> {
        let query = format!(
            "SELECT id,source,{started_at},{ended_at},{cwd},{model},{end_reason},{title},{parent_session_id},{archived},{pinned},{input_tokens},{output_tokens},{cache_read_tokens},{cache_write_tokens},{reasoning_tokens} FROM sessions ORDER BY id",
            started_at = timestamp_expression(&self.session_columns, "started_at"),
            ended_at = timestamp_expression(&self.session_columns, "ended_at"),
            cwd = optional_column(&self.session_columns, "cwd", "NULL"),
            model = optional_column(&self.session_columns, "model", "NULL"),
            end_reason = optional_column(&self.session_columns, "end_reason", "NULL"),
            title = optional_column(&self.session_columns, "title", "NULL"),
            parent_session_id = optional_column(&self.session_columns, "parent_session_id", "NULL"),
            archived = optional_column(&self.session_columns, "archived", "0"),
            pinned = optional_column(&self.session_columns, "pinned", "0"),
            input_tokens = optional_column(&self.session_columns, "input_tokens", "NULL"),
            output_tokens = optional_column(&self.session_columns, "output_tokens", "NULL"),
            cache_read_tokens = optional_column(&self.session_columns, "cache_read_tokens", "NULL"),
            cache_write_tokens =
                optional_column(&self.session_columns, "cache_write_tokens", "NULL"),
            reasoning_tokens = optional_column(&self.session_columns, "reasoning_tokens", "NULL"),
        );
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"agent-lens-hermes-state-v2\0");
        let mut sessions = Vec::new();
        let rows = sqlx::query(&query)
            .fetch_all(&mut self.connection)
            .await
            .context("read Hermes state sessions")?;
        for row in rows {
            let native_id: String = row.try_get(0).context("read Hermes session id")?;
            let source: String = row.try_get(1).context("read Hermes session source")?;
            let started_at: Option<String> = row.try_get(2)?;
            let ended_at: Option<String> = row.try_get(3)?;
            let cwd: Option<String> = row.try_get(4)?;
            let model: Option<String> = row.try_get(5)?;
            let end_reason: Option<String> = row.try_get(6)?;
            let title: Option<String> = row.try_get(7)?;
            let parent_session_id: Option<String> = row.try_get(8)?;
            let archived = row.try_get::<i64, _>(9)? != 0;
            let pinned = row.try_get::<i64, _>(10)? != 0;
            let input_tokens: Option<i64> = row.try_get(11)?;
            let output_tokens: Option<i64> = row.try_get(12)?;
            let cache_read_tokens: Option<i64> = row.try_get(13)?;
            let cache_write_tokens: Option<i64> = row.try_get(14)?;
            let reasoning_tokens: Option<i64> = row.try_get(15)?;
            let status = state_status(ended_at.as_deref(), end_reason.as_deref());
            let external_id = if self.scope.is_empty() {
                native_id.clone()
            } else {
                format!("profile:{}:{native_id}", self.scope)
            };
            let raw_metadata = json!({
                "type": "hermes_state_session",
                "source": source,
                "end_reason": end_reason,
                "parent_session_id": parent_session_id,
                "profile": if self.scope.is_empty() { None } else { Some(self.scope.as_str()) },
                "archived": archived,
                "pinned": pinned,
            })
            .to_string();
            let usage = state_usage(
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                reasoning_tokens,
            );
            hash_value(
                &mut hasher,
                &json!([
                    native_id,
                    started_at,
                    ended_at,
                    cwd,
                    model,
                    title,
                    raw_metadata,
                    usage
                ]),
            );
            sessions.push(StateSession {
                native_id,
                external_id,
                started_at,
                ended_at,
                cwd,
                model,
                status,
                title: title.and_then(|value| sanitized_title(&value, 160)),
                raw_metadata,
                usage,
            });
        }

        let message_query = self.message_query(None, true);
        let mut rows = sqlx::query(&message_query).fetch(&mut self.connection);
        while let Some(row) = rows
            .try_next()
            .await
            .context("fingerprint Hermes state messages")?
        {
            let values = state_message_values(&row)?;
            hash_value(&mut hasher, &values);
        }
        drop(rows);
        Ok(StateScan {
            content_hash: format!("blake3:{}", hasher.finalize().to_hex()),
            metadata: self.metadata.clone(),
            sessions,
        })
    }

    pub(crate) async fn records(
        &mut self,
        session: &StateSession,
    ) -> Result<VecDeque<(i64, NormalizedRecord)>> {
        let query = self.message_query(Some("?"), false);
        let rows = sqlx::query(&query)
            .bind(&session.native_id)
            .fetch_all(&mut self.connection)
            .await
            .context("read Hermes state session messages")?;
        let mut records = VecDeque::new();
        let mut sequence = 0i64;
        for row in rows {
            let values = state_message_values(&row)?;
            normalize_state_message(&values, &mut sequence, &mut records)?;
        }
        if let Some(usage) = &session.usage {
            records.push_back((
                sequence,
                NormalizedRecord::Usage {
                    timestamp: session
                        .ended_at
                        .clone()
                        .or_else(|| session.started_at.clone()),
                    cumulative: Some(usage.clone()),
                    per_turn: None,
                    pressure: Box::<PressureSnapshot>::default(),
                    raw_json: json!({"type":"session_usage","usage":usage}).to_string(),
                },
            ));
        }
        Ok(records)
    }

    fn message_query(&self, session_filter: Option<&str>, all_sessions: bool) -> String {
        let active = optional_column(&self.message_columns, "active", "1");
        let compacted = optional_column(&self.message_columns, "compacted", "0");
        let where_clause = if all_sessions {
            format!("WHERE ({active}=1 OR {compacted}=1)")
        } else {
            format!(
                "WHERE session_id={} AND ({active}=1 OR {compacted}=1)",
                session_filter.unwrap_or("?")
            )
        };
        let timestamp_order = if self.message_columns.contains("timestamp") {
            "timestamp"
        } else {
            "id"
        };
        format!(
            "SELECT id,session_id,role,{content},{tool_call_id},{tool_calls},{tool_name},{timestamp},{observed},{effect_disposition},{active},{compacted},{display_kind} FROM messages {where_clause} ORDER BY session_id,{timestamp_order},id",
            content = optional_column(&self.message_columns, "content", "NULL"),
            tool_call_id = optional_column(&self.message_columns, "tool_call_id", "NULL"),
            tool_calls = optional_column(&self.message_columns, "tool_calls", "NULL"),
            tool_name = optional_column(&self.message_columns, "tool_name", "NULL"),
            timestamp = timestamp_expression(&self.message_columns, "timestamp"),
            observed = optional_column(&self.message_columns, "observed", "0"),
            effect_disposition =
                optional_column(&self.message_columns, "effect_disposition", "NULL"),
            display_kind = optional_column(&self.message_columns, "display_kind", "NULL"),
        )
    }
}

#[cfg(windows)]
fn open_windows_snapshot(path: &Path) -> Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    // Windows SQLite can create or modify a WAL shared-memory file even in
    // read-only mode. Prevent writes to the database while using immutable mode,
    // and refuse a nonempty WAL because immutable mode can ignore its changes.
    const FILE_SHARE_READ: u32 = 1;
    let guard = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
        .context("lock Hermes state for a read-only snapshot; close Hermes and retry")?;
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    match std::fs::metadata(wal) {
        Ok(metadata) => ensure!(
            metadata.is_file() && metadata.len() == 0,
            "Hermes state has an uncheckpointed WAL; close Hermes cleanly and retry"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect Hermes state WAL"),
    }
    Ok(guard)
}

pub(crate) fn state_databases(home: &Path) -> Result<Vec<PathBuf>> {
    let mut databases = Vec::new();
    let primary = home.join("state.db");
    if optional_regular_file(&primary)? {
        databases.push(primary);
    }
    let profiles = home.join("profiles");
    if super::codex::optional_directory(&profiles)? {
        for entry in std::fs::read_dir(&profiles).context("read Hermes profiles directory")? {
            let entry = entry.context("read Hermes profile entry")?;
            if !entry
                .file_type()
                .context("inspect Hermes profile entry")?
                .is_dir()
            {
                continue;
            }
            let database = entry.path().join("state.db");
            if optional_regular_file(&database)? {
                databases.push(database);
            }
        }
    }
    databases.sort();
    Ok(databases)
}

async fn table_columns(connection: &mut SqliteConnection, table: &str) -> Result<HashSet<String>> {
    let columns = sqlx::query_scalar::<_, String>("SELECT name FROM pragma_table_info(?)")
        .bind(table)
        .fetch_all(connection)
        .await
        .with_context(|| format!("inspect Hermes {table} schema"))?;
    ensure!(
        !columns.is_empty(),
        "Hermes state database missing {table} table"
    );
    Ok(columns.into_iter().collect())
}

fn optional_column(columns: &HashSet<String>, column: &str, fallback: &str) -> String {
    if columns.contains(column) {
        format!("\"{column}\"")
    } else {
        fallback.to_owned()
    }
}

fn timestamp_expression(columns: &HashSet<String>, column: &str) -> String {
    if columns.contains(column) {
        format!(
            "CASE WHEN \"{column}\" IS NULL THEN NULL ELSE strftime('%Y-%m-%dT%H:%M:%fZ',\"{column}\",'unixepoch') END AS \"{column}\""
        )
    } else {
        "NULL".into()
    }
}

fn optional_regular_file(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                !metadata.file_type().is_symlink() && metadata.is_file(),
                "expected a regular Hermes state database"
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("inspect Hermes state database"),
    }
}

fn state_usage(
    input: Option<i64>,
    output: Option<i64>,
    cache_read: Option<i64>,
    cache_write: Option<i64>,
    reasoning: Option<i64>,
) -> Option<TokenUsage> {
    if [input, output, cache_read, cache_write, reasoning]
        .into_iter()
        .all(|value| value.is_none_or(|value| value == 0))
    {
        return None;
    }
    let input_tokens = input
        .unwrap_or_default()
        .checked_add(cache_read.unwrap_or_default())?
        .checked_add(cache_write.unwrap_or_default())?;
    let output_tokens = output.unwrap_or_default();
    Some(TokenUsage {
        input_tokens: Some(input_tokens),
        output_tokens: Some(output_tokens),
        cached_input_tokens: Some(cache_read.unwrap_or_default()),
        reasoning_output_tokens: reasoning,
        total_tokens: input_tokens.checked_add(output_tokens),
    })
}

fn state_status(ended_at: Option<&str>, end_reason: Option<&str>) -> &'static str {
    if ended_at.is_none() {
        return "unknown";
    }
    if end_reason.is_some_and(|reason| {
        let reason = reason.to_ascii_lowercase();
        ["fail", "error", "cancel", "abort", "interrupt"]
            .iter()
            .any(|value| reason.contains(value))
    }) {
        "aborted"
    } else {
        "complete"
    }
}

fn hash_value(hasher: &mut blake3::Hasher, value: &Value) {
    let bytes = value.to_string();
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes.as_bytes());
}

fn state_message_values(row: &sqlx::sqlite::SqliteRow) -> Result<Value> {
    Ok(json!([
        row.try_get::<i64, _>(0)?,
        row.try_get::<String, _>(1)?,
        row.try_get::<String, _>(2)?,
        row.try_get::<Option<String>, _>(3)?,
        row.try_get::<Option<String>, _>(4)?,
        row.try_get::<Option<String>, _>(5)?,
        row.try_get::<Option<String>, _>(6)?,
        row.try_get::<Option<String>, _>(7)?,
        row.try_get::<i64, _>(8)?,
        row.try_get::<Option<String>, _>(9)?,
        row.try_get::<i64, _>(10)?,
        row.try_get::<i64, _>(11)?,
        row.try_get::<Option<String>, _>(12)?,
    ]))
}

fn normalize_state_message(
    values: &Value,
    sequence: &mut i64,
    records: &mut VecDeque<(i64, NormalizedRecord)>,
) -> Result<()> {
    let role = values[2].as_str().unwrap_or_default();
    let content = values[3].as_str().unwrap_or_default();
    let call_id = values[4].as_str().map(str::to_owned);
    let tool_calls = values[5].as_str();
    let tool_name = values[6].as_str().map(str::to_owned);
    let timestamp = values[7].as_str().map(str::to_owned);
    let observed = values[8].as_i64().unwrap_or_default() != 0;
    let disposition = values[9].as_str();
    let active = values[10].as_i64().unwrap_or(1) != 0;
    let compacted = values[11].as_i64().unwrap_or_default() != 0;
    let display_kind = values[12].as_str();
    if display_kind == Some("hidden") {
        return Ok(());
    }
    let message_raw = || {
        json!({
            "type":"state_message",
            "role":role,
            "content":content,
            "active":active,
            "compacted":compacted,
            "observed":observed,
        })
        .to_string()
    };
    let mut push = |record| -> Result<()> {
        records.push_back((*sequence, record));
        *sequence = sequence
            .checked_add(1)
            .context("Hermes state event count exceeds supported range")?;
        Ok(())
    };
    match role {
        "user" | "assistant" | "system" | "developer" if !content.is_empty() => {
            push(NormalizedRecord::Message {
                timestamp: timestamp.clone(),
                role: if role == "system" { "developer" } else { role }.into(),
                content: content.into(),
                opens_turn: role == "user",
                raw_json: message_raw(),
            })?;
        }
        "tool" => {
            let failed = matches!(
                disposition,
                Some("failed" | "error" | "cancelled" | "canceled")
            );
            push(NormalizedRecord::ToolOutput {
                timestamp: timestamp.clone(),
                call_id: call_id.clone(),
                output: (!content.is_empty()).then(|| content.to_owned()),
                outcome: if failed { "failed" } else { "observed" },
                raw_json: json!({
                    "type":"state_tool_result",
                    "call_id":call_id,
                    "tool_name":tool_name,
                    "output":content,
                    "outcome":if failed { "failed" } else { "observed" },
                    "active":active,
                    "compacted":compacted,
                    "observed":observed,
                })
                .to_string(),
            })?;
        }
        _ => {}
    }
    if let Some(tool_calls) = tool_calls
        && let Ok(Value::Array(calls)) = serde_json::from_str::<Value>(tool_calls)
    {
        for call in calls {
            let function = call.get("function").unwrap_or(&call);
            let call_id = call
                .get("call_id")
                .or_else(|| call.get("id"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let tool_name = function
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let arguments = function.get("arguments").and_then(value_text);
            push(NormalizedRecord::ToolCall {
                timestamp: timestamp.clone(),
                call_id: call_id.clone(),
                tool_name: tool_name.clone(),
                arguments: arguments.clone(),
                kind: "tool",
                raw_json: json!({
                    "type":"state_tool_call",
                    "call_id":call_id,
                    "name":tool_name,
                    "arguments":arguments,
                    "active":active,
                    "compacted":compacted,
                    "observed":observed,
                })
                .to_string(),
            })?;
        }
    }
    Ok(())
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

pub(crate) fn cron_output_root(home: &Path) -> Result<Option<PathBuf>> {
    let output = home.join("cron/output");
    Ok(super::codex::optional_directory(&output)?.then_some(output))
}

pub(crate) fn is_session_file(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("json")
        && path
            .file_stem()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.starts_with("request_dump_"))
}

pub(crate) fn is_cron_output(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("md")
}

pub(crate) fn scan_session(path: &Path) -> Result<SessionSummary> {
    if is_cron_output(path) {
        return scan_cron_output(path);
    }
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

fn scan_cron_output(path: &Path) -> Result<SessionSummary> {
    let file = File::open(path).context("open Hermes cron output")?;
    let metadata = file.metadata().context("inspect Hermes cron output")?;
    drop(file);
    let content = std::fs::read_to_string(path).context("read Hermes cron output")?;
    ensure_unchanged(path, &metadata)?;
    let job_id = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .context("Hermes cron output missing job directory")?;
    let filename = path
        .file_stem()
        .and_then(|value| value.to_str())
        .context("Hermes cron output missing timestamp filename")?;
    let timestamp = cron_output_timestamp(path);
    let has_output = cron_output_has_output(&content);
    let derived_title = content
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .and_then(|value| sanitized_title(value, 160))
        .or_else(|| Some(format!("Hermes cron {filename}")));
    Ok(SessionSummary {
        metadata,
        content_hash: hash_file(path)?,
        external_id: format!("cron-output:{job_id}:{filename}"),
        started_at: timestamp.clone(),
        ended_at: timestamp,
        cwd: None,
        model: None,
        status: "complete",
        raw_metadata: json!({
            "type":"hermes_cron_output",
            "job_id":job_id,
            "timestamp":cron_output_timestamp(path),
            "has_output":has_output,
        })
        .to_string(),
        derived_title,
        malformed_lines: 0,
        parse_error: None,
    })
}

fn cron_output_has_output(content: &str) -> bool {
    let mut lines = content.lines();
    let Some(title) = lines.next() else {
        return false;
    };
    if !title.trim_start().starts_with("# ") {
        return content.lines().any(cron_line_has_output);
    }
    let mut saw_metadata = false;
    let mut body_started = false;
    for line in lines {
        let line = line.trim();
        if body_started {
            if cron_line_has_output(line) {
                return true;
            }
            continue;
        }
        if line.is_empty() {
            if saw_metadata {
                body_started = true;
            }
            continue;
        }
        if line.starts_with("**") && line.contains(":**") {
            saw_metadata = true;
            continue;
        }
        return true;
    }
    false
}

fn cron_line_has_output(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty()
        && !matches!(
            line,
            "Script gate returned wakeAgent=false — agent skipped."
                | "Script gate returned `wakeAgent=false` — agent skipped."
        )
}

fn cron_output_timestamp(path: &Path) -> Option<String> {
    let value = path.file_stem()?.to_str()?;
    let bytes = value.as_bytes();
    if bytes.len() != 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'_'
        || bytes[13] != b'-'
        || bytes[16] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 7 | 10 | 13 | 16) && !byte.is_ascii_digit())
    {
        return None;
    }
    Some(format!(
        "{}T{}:{}:{}",
        &value[..10],
        &value[11..13],
        &value[14..16],
        &value[17..19]
    ))
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

    #[cfg(windows)]
    #[test]
    fn windows_snapshot_refuses_open_writers_and_nonempty_wal_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let writer = File::create(&path).unwrap();
        assert!(open_windows_snapshot(&path).is_err());
        drop(writer);

        let wal = dir.path().join("state.db-wal");
        std::fs::write(&wal, b"synthetic WAL").unwrap();
        let error = open_windows_snapshot(&path).unwrap_err();
        assert!(error.to_string().contains("uncheckpointed WAL"));
        assert_eq!(std::fs::read(&wal).unwrap(), b"synthetic WAL");
        assert!(!dir.path().join("state.db-shm").exists());
    }

    #[tokio::test]
    async fn state_reader_tolerates_minimal_profile_schema() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".hermes");
        let profile = home.join("profiles/synthetic-profile");
        std::fs::create_dir_all(&profile).unwrap();
        let path = profile.join("state.db");
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true);
        let mut connection = SqliteConnection::connect_with(&options).await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE sessions(id TEXT PRIMARY KEY,source TEXT NOT NULL,started_at REAL NOT NULL); \
             CREATE TABLE messages(id INTEGER PRIMARY KEY,session_id TEXT NOT NULL,role TEXT NOT NULL,content TEXT); \
             ALTER TABLE sessions ADD COLUMN future_session_field TEXT; \
             ALTER TABLE messages ADD COLUMN future_message_field TEXT; \
             INSERT INTO sessions(id,source,started_at) VALUES('minimal','cron',1767261600); \
             INSERT INTO messages(id,session_id,role,content) VALUES(1,'minimal','user','Synthetic minimal profile request');",
        )
        .execute(&mut connection)
        .await
        .unwrap();
        connection.close().await.unwrap();

        assert_eq!(
            state_databases(&home).unwrap().as_slice(),
            std::slice::from_ref(&path)
        );
        let mut reader = StateReader::open(&path, &home).await.unwrap();
        let scan = reader.scan().await.unwrap();
        assert_eq!(scan.sessions.len(), 1);
        assert_eq!(
            scan.sessions[0].external_id,
            "profile:synthetic-profile:minimal"
        );
        assert_eq!(scan.sessions[0].status, "unknown");
        let records = reader.records(&scan.sessions[0]).await.unwrap();
        assert!(matches!(
            records.front(),
            Some((0, NormalizedRecord::Message { content, .. }))
                if content == "Synthetic minimal profile request"
        ));
    }

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
    fn cron_output_distinguishes_body_content_from_metadata_only_audits() {
        assert!(!cron_output_has_output(""));
        assert!(!cron_output_has_output(
            "# Cron Job: synthetic\n\n**Job ID:** synthetic-id\n**Run Time:** 2026-01-01 00:00:00\n**Mode:** no_agent (script)\n**Status:** silent (empty output)\n"
        ));
        assert!(!cron_output_has_output(
            "# Cron Job: synthetic\n\n**Job ID:** synthetic-id\n**Run Time:** 2026-01-01 00:00:00\n**Mode:** monitor\n**Status:** no_change (agent run suppressed)\n"
        ));
        assert!(cron_output_has_output(
            "# Cron Job: synthetic\n\n**Job ID:** synthetic-id\n**Run Time:** 2026-01-01 00:00:00\n**Mode:** no_agent (script)\n\nSynthetic script output"
        ));
        assert!(cron_output_has_output(
            "# Synthetic legacy output\n\nSynthetic agent output"
        ));
    }

    #[test]
    fn cron_script_gate_skip_is_empty_only_when_it_is_the_entire_body() {
        let notice = "Script gate returned wakeAgent=false — agent skipped.";
        let header =
            "# Cron Job: synthetic\n\n**Job ID:** synthetic\n**Unknown field:** tolerated\n\n";
        for body in [
            notice.to_owned(),
            format!("  {notice}\n\n{notice}  "),
            "Script gate returned `wakeAgent=false` — agent skipped.".into(),
        ] {
            assert!(
                !cron_output_has_output(&format!("{header}{body}")),
                "skip-only cron must be empty"
            );
            assert!(
                !cron_output_has_output(&body),
                "plain skip-only cron must be empty"
            );
        }
        for body in [
            format!("{notice}\nSynthetic result"),
            format!("Synthetic result\n{notice}"),
            "Script gate returned wakeAgent=true — agent skipped.".into(),
            "Script gate returned wakeAgent=false".into(),
        ] {
            assert!(
                cron_output_has_output(&format!("{header}{body}")),
                "other output must remain visible"
            );
        }
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
