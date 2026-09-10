use crate::fingerprint::{HashingReader, ensure_unchanged, hash_file};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use sqlx::{Acquire, SqliteConnection, SqlitePool};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

#[cfg(test)]
mod regression;

// Bump when normalized parsing semantics change so refresh replaces old projections.
const PARSER_VERSION: i64 = 1;

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
}

pub async fn ingest_data_root(pool: &SqlitePool, root: &Path) -> Result<IngestReport> {
    let mut transaction = pool.begin().await?;
    let run = sqlx::query("INSERT INTO ingest_runs(status) VALUES('running')")
        .execute(&mut *transaction)
        .await?
        .last_insert_rowid();
    let mut report = IngestReport::default();
    let homes = discover_homes(root)?;
    for (host, user, home) in homes {
        let instance = ensure_instance(&mut transaction, &host, &user, &home).await?;
        for area in ["sessions", "archived_sessions"] {
            let dir = home.join(area);
            if !optional_source_directory(&dir)? {
                continue;
            }
            for entry in WalkDir::new(&dir).follow_links(false).into_iter() {
                let entry = entry.map_err(|error| {
                    anyhow::anyhow!(
                        "source traversal failed: {}",
                        error
                            .io_error()
                            .map_or(std::io::ErrorKind::Other, std::io::Error::kind)
                    )
                })?;
                ensure!(
                    !entry.file_type().is_symlink(),
                    "source traversal encountered an unsupported symbolic link"
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
                        Ok(Some((m, t))) => {
                            file_transaction.commit().await?;
                            report.sessions += 1;
                            report.messages += m;
                            report.tool_calls += t;
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
        prune_missing_sources(&mut transaction, root).await?;
    sqlx::query("UPDATE ingest_runs SET finished_at=CURRENT_TIMESTAMP,status='complete',files_seen=?,errors=? WHERE id=?")
        .bind(report.files_seen as i64).bind(report.errors as i64).bind(run).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(report)
}

// Missing optional directories indicate removal; inspection failures never do.
fn optional_source_directory(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir(),
                "expected a source directory; symbolic links and other file types are unsupported"
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("inspect source directory"),
    }
}

fn discover_homes(root: &Path) -> Result<Vec<(String, String, PathBuf)>> {
    ensure!(
        optional_source_directory(root)?,
        "source root is missing; refresh cancelled"
    );
    let mut out = Vec::new();
    for host in std::fs::read_dir(root).context("read source root")? {
        let host = host.context("read source root entry")?;
        let kind = host.file_type().context("inspect host entry")?;
        ensure!(
            !kind.is_symlink(),
            "symbolic links in source discovery are unsupported"
        );
        if !kind.is_dir() {
            continue;
        }
        for user in std::fs::read_dir(host.path()).context("read source host directory")? {
            let user = user.context("read source host entry")?;
            let kind = user.file_type().context("inspect user entry")?;
            ensure!(
                !kind.is_symlink(),
                "symbolic links in source discovery are unsupported"
            );
            if !kind.is_dir() {
                continue;
            }
            let home = user.path().join(".codex");
            if optional_source_directory(&home)? {
                out.push((
                    host.file_name().to_string_lossy().into(),
                    user.file_name().to_string_lossy().into(),
                    home,
                ));
            }
        }
    }
    Ok(out)
}

async fn prune_missing_sources(
    connection: &mut SqliteConnection,
    root: &Path,
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
            let Ok(relative) = home.strip_prefix(root) else {
                continue;
            };
            // Only the exact root/host/user/.codex contract belongs to this run.
            if relative.components().count() != 3
                || relative.file_name() != Some(std::ffi::OsStr::new(".codex"))
            {
                continue;
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
) -> Result<Option<(usize, usize)>> {
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
    let file = File::open(path).context("open rollout")?;
    let metadata = file.metadata().context("inspect rollout")?;
    let mut reader = BufReader::new(HashingReader::new(file));
    let mut values = Vec::new();
    let mut line_errors = Vec::new();
    let mut line = Vec::new();
    let mut n = 0;
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .context("read rollout line")?
            == 0
        {
            break;
        }
        n += 1;
        match serde_json::from_slice::<Value>(&line) {
            Ok(v) => values.push(v),
            // Do not persist parser text that could quote private source values.
            Err(_) => line_errors.push(format!("line {n}: invalid JSON or UTF-8")),
        }
    }
    ensure_unchanged(path, &metadata)?;
    let content_hash = reader.get_ref().fingerprint();
    if previous.as_ref().is_some_and(|row| row.has_session) {
        ensure!(
            line_errors.is_empty(),
            "partial replacement; previous records retained; retry refresh"
        );
    }
    let meta = values
        .iter()
        .find(|v| v["type"] == "session_meta")
        .context("missing session_meta")?;
    let p = &meta["payload"];
    let external_id = p["id"].as_str().context("session_meta missing id")?;
    let started = p["timestamp"]
        .as_str()
        .or_else(|| meta["timestamp"].as_str());
    let model = values
        .iter()
        .find(|v| v["type"] == "turn_context")
        .and_then(|v| v["payload"]["model"].as_str());
    let ended = values.iter().rev().find_map(|v| v["timestamp"].as_str());
    let status = if values
        .iter()
        .any(|v| v["type"] == "event_msg" && v["payload"]["type"] == "task_complete")
    {
        "complete"
    } else if values
        .iter()
        .any(|v| v["type"] == "event_msg" && v["payload"]["type"] == "turn_aborted")
    {
        "aborted"
    } else {
        "unknown"
    };
    let existing: Option<(i64, String)> = sqlx::query_as("SELECT sessions.id,source_files.path FROM sessions JOIN source_files ON source_files.id=sessions.source_file_id WHERE sessions.instance_id=? AND external_id=?")
        .bind(instance).bind(external_id).fetch_optional(&mut *connection).await?;
    if existing.is_some() {
        ensure!(
            line_errors.is_empty(),
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
        .bind(instance).bind(run).bind(path.to_string_lossy().as_ref()).bind(kind).bind(metadata.len() as i64).fetch_one(&mut *connection).await?;
    // A replaced path can now identify a different session. Remove only its old projection.
    sqlx::query("DELETE FROM sessions WHERE source_file_id=? AND external_id<>?")
        .bind(source_id)
        .bind(external_id)
        .execute(&mut *connection)
        .await?;
    let session_id: i64 = sqlx::query_scalar("INSERT INTO sessions(instance_id,source_file_id,external_id,started_at,ended_at,cwd,model,status,raw_metadata) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(instance_id,external_id) DO UPDATE SET source_file_id=excluded.source_file_id,started_at=excluded.started_at,ended_at=excluded.ended_at,cwd=excluded.cwd,model=excluded.model,status=excluded.status,raw_metadata=excluded.raw_metadata RETURNING id")
        .bind(instance).bind(source_id).bind(external_id).bind(started).bind(ended).bind(p["cwd"].as_str()).bind(model).bind(status).bind(meta.to_string()).fetch_one(&mut *connection).await?;
    if existing.is_some() {
        for table in ["messages", "tool_calls", "usage_events"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE session_id=?"))
                .bind(session_id)
                .execute(&mut *connection)
                .await?;
        }
    }
    let mut messages = 0usize;
    let mut tools = 0usize;
    let mut pending: HashMap<String, i64> = HashMap::new();
    for (sequence, v) in values.iter().enumerate() {
        let typ = v["type"].as_str().unwrap_or("");
        let payload = &v["payload"];
        if typ == "response_item" && payload["type"] == "message" {
            let content = payload["content"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            sqlx::query("INSERT INTO messages(session_id,sequence,timestamp,role,content,raw_json) VALUES(?,?,?,?,?,?)")
                .bind(session_id).bind(sequence as i64).bind(v["timestamp"].as_str()).bind(payload["role"].as_str().unwrap_or("unknown")).bind(content).bind(v.to_string()).execute(&mut *connection).await?;
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
            let id = sqlx::query("INSERT INTO tool_calls(session_id,sequence,timestamp,call_id,tool_name,arguments,status,raw_json) VALUES(?,?,?,?,?,?,'called',?)")
                .bind(session_id).bind(sequence as i64).bind(v["timestamp"].as_str()).bind(call_id).bind(payload["name"].as_str()).bind(payload["arguments"].as_str().or_else(|| payload["input"].as_str())).bind(v.to_string()).execute(&mut *connection).await?.last_insert_rowid();
            pending.insert(call_id.to_owned(), id);
            tools += 1;
        } else if typ == "response_item"
            && matches!(
                payload["type"].as_str(),
                Some("function_call_output" | "custom_tool_call_output")
            )
        {
            if let Some(id) = payload["call_id"].as_str().and_then(|x| pending.get(x)) {
                sqlx::query("UPDATE tool_calls SET output=?,status='complete' WHERE id=?")
                    .bind(payload["output"].as_str())
                    .bind(id)
                    .execute(&mut *connection)
                    .await?;
            }
        } else if typ == "event_msg"
            && payload["type"] == "token_count"
            && !payload["info"].is_null()
        {
            let u = &payload["info"]["total_token_usage"];
            sqlx::query("INSERT INTO usage_events(session_id,timestamp,input_tokens,output_tokens,cached_input_tokens,total_tokens,raw_json) VALUES(?,?,?,?,?,?,?)")
                .bind(session_id).bind(v["timestamp"].as_str()).bind(u["input_tokens"].as_i64()).bind(u["output_tokens"].as_i64()).bind(u["cached_input_tokens"].as_i64()).bind(u["total_tokens"].as_i64()).bind(v.to_string()).execute(&mut *connection).await?;
        }
    }
    ensure_unchanged(path, &metadata)?;
    let parse_status = if line_errors.is_empty() {
        "ok"
    } else {
        "partial"
    };
    sqlx::query("UPDATE source_files SET parse_status=?,parse_error=?,content_hash=?,parser_version=? WHERE id=?")
        .bind(parse_status)
        .bind((!line_errors.is_empty()).then(|| line_errors.join("; ")))
        .bind(&content_hash)
        .bind(PARSER_VERSION)
        .bind(source_id)
        .execute(&mut *connection)
        .await?;
    Ok(Some((messages, tools)))
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
        assert_eq!((homes[0].0.as_str(), homes[0].1.as_str()), ("host", "user"));
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
