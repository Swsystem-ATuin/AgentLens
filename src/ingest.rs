use anyhow::{Context, Result};
use serde_json::Value;
use sqlx::{SqliteConnection, SqlitePool};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

#[cfg(test)]
mod regression;

#[derive(Default, Debug)]
pub struct IngestReport {
    pub sessions: usize,
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
    let homes = discover_homes(root);
    for (host, user, home) in homes {
        let instance = ensure_instance(&mut transaction, &host, &user, &home).await?;
        for area in ["sessions", "archived_sessions"] {
            let dir = home.join(area);
            if !dir.exists() {
                continue;
            }
            for entry in WalkDir::new(&dir)
                .follow_links(false)
                .into_iter()
                .filter_map(Result::ok)
            {
                if entry.file_type().is_file()
                    && entry.path().extension().is_some_and(|x| x == "jsonl")
                {
                    match ingest_rollout(&mut transaction, run, instance, entry.path(), area).await
                    {
                        Ok((m, t)) => {
                            report.sessions += 1;
                            report.messages += m;
                            report.tool_calls += t;
                        }
                        Err(error) => {
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
    sqlx::query("UPDATE ingest_runs SET finished_at=CURRENT_TIMESTAMP,status='complete',files_seen=?,errors=? WHERE id=?")
        .bind(report.sessions as i64).bind(report.errors as i64).bind(run).execute(&mut *transaction).await?;
    transaction.commit().await?;
    Ok(report)
}

fn discover_homes(root: &Path) -> Vec<(String, String, PathBuf)> {
    let mut out = Vec::new();
    let Ok(hosts) = std::fs::read_dir(root) else {
        return out;
    };
    for host in hosts.flatten().filter(|e| e.path().is_dir()) {
        let Ok(users) = std::fs::read_dir(host.path()) else {
            continue;
        };
        for user in users.flatten().filter(|e| e.path().is_dir()) {
            let home = user.path().join(".codex");
            if home.is_dir() {
                out.push((
                    host.file_name().to_string_lossy().into(),
                    user.file_name().to_string_lossy().into(),
                    home,
                ));
            }
        }
    }
    out
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
) -> Result<(usize, usize)> {
    let metadata = std::fs::metadata(path)?;
    let source_id = sqlx::query("INSERT INTO source_files(instance_id,ingest_run_id,path,kind,size_bytes,parse_status) VALUES(?,?,?,?,?,'parsing')")
        .bind(instance).bind(run).bind(path.to_string_lossy().as_ref()).bind(kind).bind(metadata.len() as i64).execute(&mut *connection).await?.last_insert_rowid();
    let file = File::open(path)?;
    let mut values = Vec::new();
    let mut line_errors = Vec::new();
    for (n, line) in BufReader::new(file).lines().enumerate() {
        match line.and_then(|s| serde_json::from_str::<Value>(&s).map_err(std::io::Error::other)) {
            Ok(v) => values.push(v),
            Err(e) => line_errors.push(format!("line {}: {}", n + 1, e)),
        }
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
    let session_id = sqlx::query("INSERT INTO sessions(instance_id,source_file_id,external_id,started_at,ended_at,cwd,model,status,raw_metadata) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(instance).bind(source_id).bind(external_id).bind(started).bind(ended).bind(p["cwd"].as_str()).bind(model).bind(status).bind(meta.to_string()).execute(&mut *connection).await?.last_insert_rowid();
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
    let parse_status = if line_errors.is_empty() {
        "ok"
    } else {
        "partial"
    };
    sqlx::query("UPDATE source_files SET parse_status=?,parse_error=? WHERE id=?")
        .bind(parse_status)
        .bind((!line_errors.is_empty()).then(|| line_errors.join("; ")))
        .bind(source_id)
        .execute(&mut *connection)
        .await?;
    Ok((messages, tools))
}

async fn record_file_error(
    connection: &mut SqliteConnection,
    run: i64,
    instance: i64,
    path: &Path,
    kind: &str,
    error: &str,
) -> Result<()> {
    sqlx::query("INSERT OR REPLACE INTO source_files(instance_id,ingest_run_id,path,kind,size_bytes,parse_status,parse_error) VALUES(?,?,?,?,?,'error',?)")
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
        let homes = discover_homes(tmp.path());
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
