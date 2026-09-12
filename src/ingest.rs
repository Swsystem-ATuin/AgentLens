use crate::fingerprint::{HashingReader, ensure_unchanged, hash_file};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sqlx::{Acquire, SqliteConnection, SqlitePool};
use std::{
    fs::{File, Metadata},
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

#[cfg(test)]
mod regression;

// Bump when normalized parsing semantics change so refresh replaces old projections.
const PARSER_VERSION: i64 = 2;
const MAX_PARSE_DIAGNOSTICS: usize = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SourceLayout {
    HostUser,
    DirectCodexHome,
}

impl SourceLayout {
    fn database_value(self) -> &'static str {
        match self {
            Self::HostUser => "host-user",
            Self::DirectCodexHome => "direct-codex-home",
        }
    }
}

struct DiscoveredSources {
    layout: SourceLayout,
    homes: Vec<(String, String, PathBuf)>,
}

#[derive(sqlx::FromRow)]
struct SourceCheckpoint {
    content_hash: Option<String>,
    parser_version: Option<i64>,
    parse_status: String,
    has_session: bool,
}

#[derive(Default, Debug)]
pub struct IngestReport {
    pub sessions: usize,
    pub skipped: usize,
    pub files_seen: usize,
    pub removed_sessions: u64,
    pub removed_files: u64,
    pub messages: usize,
    pub tool_calls: usize,
    pub errors: usize,
    pub partial_files: usize,
    pub malformed_lines: usize,
}

struct ImportedRollout {
    messages: usize,
    tool_calls: usize,
    malformed_lines: usize,
}

struct RolloutSummary {
    metadata: Metadata,
    content_hash: String,
    external_id: String,
    started_at: Option<String>,
    ended_at: Option<String>,
    cwd: Option<String>,
    model: Option<String>,
    status: &'static str,
    raw_metadata: String,
    malformed_lines: usize,
    parse_error: Option<String>,
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

pub async fn ingest_data_root(pool: &SqlitePool, root: &Path) -> Result<IngestReport> {
    let discovered = discover_homes(root)?;
    let mut transaction = pool.begin().await?;
    let previous_layout: Option<String> = sqlx::query_scalar(
        "SELECT source_layout FROM ingest_runs WHERE status='complete' ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&mut *transaction)
    .await
    .context("load previous ingest source layout")?;
    ensure!(
        previous_layout
            .as_deref()
            .is_none_or(|previous| previous == discovered.layout.database_value()),
        "source layout does not match the previous rebuild or refresh; run rebuild to change between a direct .codex home and host/user data"
    );
    let run = sqlx::query("INSERT INTO ingest_runs(status,source_layout) VALUES('running',?)")
        .bind(discovered.layout.database_value())
        .execute(&mut *transaction)
        .await?
        .last_insert_rowid();
    let mut report = IngestReport::default();
    for (host, user, home) in discovered.homes {
        let instance = ensure_instance(&mut transaction, &host, &user, &home).await?;
        for area in ["sessions", "archived_sessions"] {
            let dir = home.join(area);
            if !optional_source_directory(&dir)? {
                continue;
            }
            for entry in WalkDir::new(&dir).follow_links(true).into_iter() {
                let entry = entry.map_err(|error| {
                    anyhow::anyhow!(
                        "source traversal failed: {}",
                        error
                            .io_error()
                            .map_or(std::io::ErrorKind::Other, std::io::Error::kind)
                    )
                })?;
                ensure!(
                    !entry.path_is_symlink() || entry.file_type().is_dir(),
                    "source traversal encountered an unsupported file symbolic link"
                );
                if entry.file_type().is_file()
                    && entry.path().extension().is_some_and(|x| x == "jsonl")
                {
                    report.files_seen += 1;
                    let mut file_transaction = transaction.begin().await?;
                    match ingest_rollout(&mut file_transaction, run, instance, entry.path(), area)
                        .await
                    {
                        Ok(None) => {
                            file_transaction.commit().await?;
                            report.skipped += 1;
                        }
                        Ok(Some(imported)) => {
                            file_transaction.commit().await?;
                            report.sessions += 1;
                            report.messages += imported.messages;
                            report.tool_calls += imported.tool_calls;
                            if imported.malformed_lines != 0 {
                                report.partial_files += 1;
                                report.malformed_lines += imported.malformed_lines;
                            }
                        }
                        Err(error) => {
                            file_transaction.rollback().await?;
                            report.errors += 1;
                            record_file_error(
                                &mut transaction,
                                run,
                                instance,
                                entry.path(),
                                area,
                                &format!("{error:#}"),
                            )
                            .await?;
                        }
                    }
                }
            }
        }
    }
    (report.removed_sessions, report.removed_files) =
        prune_missing_sources(&mut transaction, root, discovered.layout).await?;
    sqlx::query("UPDATE ingest_runs SET finished_at=CURRENT_TIMESTAMP,status='complete',files_seen=?,errors=? WHERE id=?")
        .bind(report.files_seen as i64).bind(report.errors as i64).bind(run).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(report)
}

// Missing optional directories indicate removal; inspection failures never do.
// Inspect the link itself first: a dangling backup link is a failure, not removal.
fn optional_source_directory(path: &Path) -> Result<bool> {
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

fn discover_homes(root: &Path) -> Result<DiscoveredSources> {
    ensure!(
        optional_source_directory(root)?,
        "source root is missing; refresh cancelled"
    );
    if root.file_name() == Some(std::ffi::OsStr::new(".codex")) {
        return Ok(DiscoveredSources {
            layout: SourceLayout::DirectCodexHome,
            // The relational schema retains one internal identity for a direct
            // home. Its empty values are never presented by the web UI.
            homes: vec![(String::new(), String::new(), root.to_path_buf())],
        });
    }
    let mut homes = Vec::new();
    for host in std::fs::read_dir(root).context("read source root")? {
        let host = host.context("read source root entry")?;
        let kind = host.file_type().context("inspect host entry")?;
        if !(kind.is_dir() || kind.is_symlink() && optional_source_directory(&host.path())?) {
            continue;
        }
        for user in std::fs::read_dir(host.path()).context("read source host directory")? {
            let user = user.context("read source host entry")?;
            let kind = user.file_type().context("inspect user entry")?;
            if !(kind.is_dir() || kind.is_symlink() && optional_source_directory(&user.path())?) {
                continue;
            }
            let home = user.path().join(".codex");
            if optional_source_directory(&home)? {
                homes.push((
                    host.file_name().to_string_lossy().into(),
                    user.file_name().to_string_lossy().into(),
                    home,
                ));
            }
        }
    }
    Ok(DiscoveredSources {
        layout: SourceLayout::HostUser,
        homes,
    })
}

async fn prune_missing_sources(
    connection: &mut SqliteConnection,
    root: &Path,
    layout: SourceLayout,
) -> Result<(u64, u64)> {
    // Recheck the root before deleting: an unavailable root is not an empty snapshot.
    ensure!(
        optional_source_directory(root)?,
        "source root disappeared; refresh cancelled"
    );
    let mut cursor = 0i64;
    let mut sessions = 0;
    let mut files = 0;
    loop {
        // Bound path buffering even when a database contains many source roots.
        let rows: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT source_files.id,source_files.path,agent_instances.source_home FROM source_files JOIN agent_instances ON agent_instances.id=source_files.instance_id JOIN agents ON agents.id=agent_instances.agent_id WHERE source_files.id>? AND agents.kind='codex' ORDER BY source_files.id LIMIT 256")
            .bind(cursor).fetch_all(&mut *connection).await.context("query source removal candidates")?;
        if rows.is_empty() {
            break;
        }
        for (id, path, home) in rows {
            cursor = id;
            let home = Path::new(&home);
            match layout {
                SourceLayout::DirectCodexHome if home != root => continue,
                SourceLayout::DirectCodexHome => {}
                SourceLayout::HostUser => {
                    let Ok(relative) = home.strip_prefix(root) else {
                        continue;
                    };
                    // Only the exact root/host/user/.codex contract belongs to this run.
                    if relative.components().count() != 3
                        || relative.file_name() != Some(std::ffi::OsStr::new(".codex"))
                    {
                        continue;
                    }
                }
            }
            ensure!(
                Path::new(&path).starts_with(home),
                "indexed source is outside its home; refresh cancelled"
            );
            let source_present = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    ensure!(
                        !metadata.file_type().is_symlink(),
                        "indexed source became a symbolic link; refresh cancelled"
                    );
                    metadata.is_file()
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error).context("check indexed source for removal"),
            };
            if !source_present {
                sessions += sqlx::query("DELETE FROM sessions WHERE source_file_id=?")
                    .bind(id)
                    .execute(&mut *connection)
                    .await
                    .context("remove missing source sessions")?
                    .rows_affected();
                // Session children cascade; deleting the checkpoint also removes parse errors and paths.
                files += sqlx::query("DELETE FROM source_files WHERE id=?")
                    .bind(id)
                    .execute(&mut *connection)
                    .await
                    .context("remove missing source checkpoint")?
                    .rows_affected();
            }
        }
    }
    Ok((sessions, files))
}

async fn ensure_instance(
    connection: &mut SqliteConnection,
    host: &str,
    user: &str,
    home: &Path,
) -> Result<i64> {
    sqlx::query("INSERT OR IGNORE INTO agents(kind,display_name) VALUES('codex','Codex')")
        .execute(&mut *connection)
        .await?;
    sqlx::query("INSERT OR IGNORE INTO hosts(name) VALUES(?)")
        .bind(host)
        .execute(&mut *connection)
        .await?;
    let host_id: i64 = sqlx::query_scalar("SELECT id FROM hosts WHERE name=?")
        .bind(host)
        .fetch_one(&mut *connection)
        .await?;
    sqlx::query("INSERT OR IGNORE INTO users(host_id,os_user) VALUES(?,?)")
        .bind(host_id)
        .bind(user)
        .execute(&mut *connection)
        .await?;
    let user_id: i64 = sqlx::query_scalar("SELECT id FROM users WHERE host_id=? AND os_user=?")
        .bind(host_id)
        .bind(user)
        .fetch_one(&mut *connection)
        .await?;
    let agent_id: i64 = sqlx::query_scalar("SELECT id FROM agents WHERE kind='codex'")
        .fetch_one(&mut *connection)
        .await?;
    let source = home.to_string_lossy();
    sqlx::query(
        "INSERT OR IGNORE INTO agent_instances(agent_id,user_id,source_home) VALUES(?,?,?)",
    )
    .bind(agent_id)
    .bind(user_id)
    .bind(source.as_ref())
    .execute(&mut *connection)
    .await?;
    Ok(
        sqlx::query_scalar("SELECT id FROM agent_instances WHERE source_home=?")
            .bind(source.as_ref())
            .fetch_one(&mut *connection)
            .await?,
    )
}

async fn ingest_rollout(
    connection: &mut SqliteConnection,
    run: i64,
    instance: i64,
    path: &Path,
    kind: &str,
) -> Result<Option<ImportedRollout>> {
    let previous: Option<SourceCheckpoint> = sqlx::query_as(
        "SELECT content_hash,parser_version,parse_status,EXISTS(SELECT 1 FROM sessions WHERE source_file_id=source_files.id) AS has_session FROM source_files WHERE path=?")
        .bind(path.to_string_lossy().as_ref()).fetch_optional(&mut *connection).await?;
    if let Some(checkpoint) = &previous
        && checkpoint.has_session
        && checkpoint.parser_version == Some(PARSER_VERSION)
        && checkpoint.parse_status == "ok"
        && let Some(hash) = &checkpoint.content_hash
        && hash_file(path)? == *hash
    {
        return Ok(None);
    }
    let summary = scan_rollout(path)?;
    if previous.as_ref().is_some_and(|row| row.has_session) {
        ensure!(
            summary.malformed_lines == 0,
            "partial replacement; previous records retained; retry refresh"
        );
    }
    let existing: Option<(i64, String)> = sqlx::query_as("SELECT sessions.id,source_files.path FROM sessions JOIN source_files ON source_files.id=sessions.source_file_id WHERE sessions.instance_id=? AND external_id=?")
        .bind(instance).bind(&summary.external_id).fetch_optional(&mut *connection).await?;
    if existing.is_some() {
        ensure!(
            summary.malformed_lines == 0,
            "partial replacement; previous records retained; retry refresh"
        );
    }
    if let Some((_, old_path)) = &existing {
        ensure!(
            Path::new(old_path) == path
                || !Path::new(old_path)
                    .try_exists()
                    .context("inspect prior source location")?,
            "duplicate session identity at another source; previous records retained"
        );
    }
    let source_id: i64 = sqlx::query_scalar("INSERT INTO source_files(instance_id,ingest_run_id,path,kind,size_bytes,parse_status) VALUES(?,?,?,?,?,'parsing') ON CONFLICT(path) DO UPDATE SET ingest_run_id=excluded.ingest_run_id,kind=excluded.kind,size_bytes=excluded.size_bytes,parse_status='parsing' RETURNING id")
        .bind(instance).bind(run).bind(path.to_string_lossy().as_ref()).bind(kind).bind(summary.metadata.len() as i64).fetch_one(&mut *connection).await?;
    // A replaced path can now identify a different session. Remove only its old projection.
    sqlx::query("DELETE FROM sessions WHERE source_file_id=? AND external_id<>?")
        .bind(source_id)
        .bind(&summary.external_id)
        .execute(&mut *connection)
        .await?;
    let session_id: i64 = sqlx::query_scalar("INSERT INTO sessions(instance_id,source_file_id,external_id,started_at,ended_at,cwd,model,status,raw_metadata) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(instance_id,external_id) DO UPDATE SET source_file_id=excluded.source_file_id,started_at=excluded.started_at,ended_at=excluded.ended_at,cwd=excluded.cwd,model=excluded.model,status=excluded.status,raw_metadata=excluded.raw_metadata RETURNING id")
        .bind(instance).bind(source_id).bind(&summary.external_id).bind(summary.started_at.as_deref()).bind(summary.ended_at.as_deref()).bind(summary.cwd.as_deref()).bind(summary.model.as_deref()).bind(summary.status).bind(&summary.raw_metadata).fetch_one(&mut *connection).await?;
    if existing.is_some() {
        for table in ["messages", "tool_calls", "usage_events"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE session_id=?"))
                .bind(session_id)
                .execute(&mut *connection)
                .await?;
        }
    }
    let (messages, tools, second_pass_malformed) =
        import_rollout_records(connection, path, &summary.metadata, session_id).await?;
    ensure!(
        second_pass_malformed == summary.malformed_lines,
        "source changed between parsing passes; retry refresh"
    );
    let parse_status = if summary.malformed_lines == 0 {
        "ok"
    } else {
        "partial"
    };
    sqlx::query("UPDATE source_files SET parse_status=?,parse_error=?,content_hash=?,parser_version=? WHERE id=?")
        .bind(parse_status)
        .bind(summary.parse_error.as_deref())
        .bind(&summary.content_hash)
        .bind(PARSER_VERSION)
        .bind(source_id)
        .execute(&mut *connection)
        .await?;
    Ok(Some(ImportedRollout {
        messages,
        tool_calls: tools,
        malformed_lines: summary.malformed_lines,
    }))
}

fn scan_rollout(path: &Path) -> Result<RolloutSummary> {
    let file = File::open(path).context("open rollout")?;
    let metadata = file.metadata().context("inspect rollout")?;
    let mut reader = BufReader::new(HashingReader::new(file));
    let mut session_metadata = None;
    let mut model = None;
    let mut saw_turn_context = false;
    let mut ended_at = None;
    let mut status = "unknown";
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
                if capture_session_metadata {
                    session_metadata = Some(value);
                }
            }
            // Never retain malformed source content in diagnostics.
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
        malformed_lines: diagnostics.malformed_lines,
        parse_error: diagnostics.message(),
    })
}

async fn import_rollout_records(
    connection: &mut SqliteConnection,
    path: &Path,
    metadata: &Metadata,
    session_id: i64,
) -> Result<(usize, usize, usize)> {
    ensure_unchanged(path, metadata)?;
    let file = File::open(path).context("reopen rollout for record import")?;
    let mut reader = BufReader::new(file);
    let mut messages = 0usize;
    let mut tools = 0usize;
    let mut malformed_lines = 0usize;
    let mut sequence = 0i64;
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .context("read rollout record for import")?
            == 0
        {
            break;
        }
        let value = match serde_json::from_slice::<Value>(&line) {
            Ok(value) => value,
            Err(_) => {
                malformed_lines = malformed_lines
                    .checked_add(1)
                    .context("malformed-line count exceeds supported range")?;
                continue;
            }
        };
        let typ = value["type"].as_str().unwrap_or("");
        let payload = &value["payload"];
        if typ == "response_item" && payload["type"] == "message" {
            sqlx::query("INSERT INTO messages(session_id,sequence,timestamp,role,content,raw_json) VALUES(?,?,?,?,?,?)")
                .bind(session_id).bind(sequence).bind(value["timestamp"].as_str()).bind(payload["role"].as_str().unwrap_or("unknown")).bind(message_content(payload)).bind(value.to_string()).execute(&mut *connection).await?;
            messages += 1;
        } else if typ == "response_item"
            && matches!(
                payload["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )
        {
            let call_id = payload["call_id"]
                .as_str()
                .or_else(|| payload["id"].as_str())
                .unwrap_or("");
            sqlx::query("INSERT INTO tool_calls(session_id,sequence,timestamp,call_id,tool_name,arguments,status,raw_json) VALUES(?,?,?,?,?,?,'called',?)")
                .bind(session_id).bind(sequence).bind(value["timestamp"].as_str()).bind(call_id).bind(payload["name"].as_str()).bind(payload["arguments"].as_str().or_else(|| payload["input"].as_str())).bind(value.to_string()).execute(&mut *connection).await?;
            tools += 1;
        } else if typ == "response_item"
            && matches!(
                payload["type"].as_str(),
                Some("function_call_output" | "custom_tool_call_output")
            )
        {
            if let Some(call_id) = payload["call_id"].as_str() {
                sqlx::query("UPDATE tool_calls SET output=?,status='complete' WHERE id=(SELECT id FROM tool_calls WHERE session_id=? AND call_id=? ORDER BY sequence DESC LIMIT 1)")
                    .bind(payload["output"].as_str())
                    .bind(session_id)
                    .bind(call_id)
                    .execute(&mut *connection)
                    .await?;
            }
        } else if typ == "event_msg"
            && payload["type"] == "token_count"
            && !payload["info"].is_null()
        {
            let usage = &payload["info"]["total_token_usage"];
            sqlx::query("INSERT INTO usage_events(session_id,timestamp,input_tokens,output_tokens,cached_input_tokens,total_tokens,raw_json) VALUES(?,?,?,?,?,?,?)")
                .bind(session_id).bind(value["timestamp"].as_str()).bind(usage["input_tokens"].as_i64()).bind(usage["output_tokens"].as_i64()).bind(usage["cached_input_tokens"].as_i64()).bind(usage["total_tokens"].as_i64()).bind(value.to_string()).execute(&mut *connection).await?;
        }
        sequence = sequence
            .checked_add(1)
            .context("rollout record count exceeds supported range")?;
    }
    ensure_unchanged(path, metadata)?;
    Ok((messages, tools, malformed_lines))
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

/// Advance the lifecycle outcome in source-record order. `unknown` means that
/// no terminal outcome has been observed for the latest opened turn; it does
/// not claim that a copied session is currently running.
fn next_session_status(status: &'static str, value: &Value) -> &'static str {
    let record_type = value["type"].as_str();
    let payload = &value["payload"];
    match (record_type, payload["type"].as_str()) {
        (Some("event_msg"), Some("task_complete")) => "complete",
        (Some("event_msg"), Some("turn_aborted")) => "aborted",
        (Some("event_msg"), Some("task_started" | "user_message")) | (Some("turn_context"), _) => {
            "unknown"
        }
        (Some("response_item"), Some("message")) if payload["role"].as_str() == Some("user") => {
            "unknown"
        }
        _ => status,
    }
}

async fn record_file_error(
    connection: &mut SqliteConnection,
    run: i64,
    instance: i64,
    path: &Path,
    kind: &str,
    error: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO source_files(instance_id,ingest_run_id,path,kind,size_bytes,parse_status,parse_error) VALUES(?,?,?,?,?,'error',?) ON CONFLICT(path) DO UPDATE SET ingest_run_id=excluded.ingest_run_id,parse_status='error',parse_error=excluded.parse_error")
        .bind(instance).bind(run).bind(path.to_string_lossy().as_ref()).bind(kind).bind(std::fs::metadata(path).ok().map(|m| m.len() as i64)).bind(error).execute(&mut *connection).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovers_contract_homes() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("host/user/.codex/sessions")).unwrap();
        let homes = discover_homes(tmp.path()).unwrap();
        assert_eq!(homes.layout, SourceLayout::HostUser);
        assert_eq!(
            (homes.homes[0].0.as_str(), homes.homes[0].1.as_str()),
            ("host", "user")
        );
    }

    #[tokio::test]
    async fn indexes_redacted_rollout_fixture() {
        let temp = tempfile::tempdir().unwrap();
        let sessions = temp
            .path()
            .join("fixture-host/fixture-user/.codex/sessions/2026/01/02");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("rollout.jsonl"),
            include_str!("../tests/fixtures/codex/rollout.jsonl"),
        )
        .unwrap();
        let pool = crate::db::open(&temp.path().join("index.sqlite"))
            .await
            .unwrap();
        let report = ingest_data_root(&pool, temp.path()).await.unwrap();
        assert_eq!((report.sessions, report.messages, report.errors), (1, 2, 0));
        let row: (String, String, i64) = sqlx::query_as(
            "SELECT external_id, model, (SELECT count(*) FROM messages) FROM sessions",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row,
            (
                "00000000-0000-0000-0000-000000000001".into(),
                "test-model".into(),
                2
            )
        );
    }
}
