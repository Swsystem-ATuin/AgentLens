use super::{IngestReport, SourceRecords, import_records};
use crate::{
    identity,
    redaction::Sanitizer,
    sources::{NormalizedRecord, codex::history::History},
};
use anyhow::Result;
use serde_json::json;
use sqlx::{Acquire, SqliteConnection};
use std::path::{Path, PathBuf};

const KIND: &str = "codex_history";

#[allow(clippy::too_many_arguments)]
pub(super) async fn import(
    index: &mut SqliteConnection,
    run: i64,
    instance: i64,
    source_key: &str,
    homes: &[PathBuf],
    sanitizer: &Sanitizer,
    report: &mut IngestReport,
    diagnostics: bool,
) -> Result<()> {
    let mut history = match History::open(homes).await {
        Ok(history) => history,
        Err(_) => {
            report.errors += 1;
            for home in homes {
                status(home, "access failed", sanitizer, diagnostics);
            }
            return Ok(());
        }
    };
    for (path, state) in &history.diagnostics {
        report.files_seen += 1;
        if state.starts_with("access failed") { report.errors += 1; }
        status(path, state, sanitizer, diagnostics);
    }
    let mut indexed = 0;
    for (external, thread) in std::mem::take(&mut history.threads) {
        let mut tx = index.begin().await?;
        let prior: Option<(i64, String, String, String)> = sqlx::query_as(
            "SELECT s.id,f.kind,f.path,CASE WHEN f.kind='codex_history' THEN coalesce(json_extract(s.raw_metadata,'$.history_hash'),'') ELSE '' END FROM sessions s JOIN source_files f ON f.id=s.source_file_id WHERE s.instance_id=? AND s.external_id=?",
        ).bind(instance).bind(&external).fetch_optional(&mut *tx).await?;
        let safe = |value: Option<String>| sanitizer.optional(value, &[]);
        let metadata = json!({
            "originator": safe(thread.originator),
            "source": safe(thread.source),
            "thread_source": safe(thread.thread_source),
            "archived": thread.archived,
        });
        let title = safe(thread.title).filter(|_| !sanitizer.active());
        if let Some((id, kind, path, _)) = &prior
            && kind != KIND
            && Path::new(path).try_exists().unwrap_or(true)
        {
            // Rollouts carry more detail and win as the transcript source.
            sqlx::query("UPDATE sessions SET raw_metadata=json_set(raw_metadata,'$.codex_history',json(?)),title=CASE WHEN title IS coalesce(derived_title,external_id) THEN coalesce(?,title) ELSE title END WHERE id=? AND (json_extract(raw_metadata,'$.codex_history') IS NOT json(?) OR (title IS coalesce(derived_title,external_id) AND ? IS NOT NULL AND title IS NOT ?))")
                .bind(metadata.to_string()).bind(title.as_deref()).bind(id)
                .bind(metadata.to_string()).bind(title.as_deref()).bind(title.as_deref())
                .execute(&mut *tx).await?;
            tx.commit().await?;
            continue;
        }
        let (records, item_path, malformed) = match history.records(&external).await {
            Ok(result) => result,
            Err(_) => {
                report.errors += 1;
                continue;
            }
        };
        if malformed != 0 {
            report.partial_files += 1;
            report.malformed_lines += malformed;
        }
        if prior.is_some() && (malformed != 0 || records.is_empty()) {
            // Keep a previously materialized transcript when the observed
            // history is incomplete or only a catalog is currently readable.
            continue;
        }
        let path = item_path.as_deref().unwrap_or(&thread.path);
        if sanitizer.check_locator(&external).is_err()
            || sanitizer.check_locator(&path.to_string_lossy()).is_err()
        {
            report.errors += 1;
            continue;
        }
        let started = safe(thread.created);
        let updated = safe(thread.updated);
        let model = safe(thread.model);
        let mut hasher = blake3::Hasher::new();
        hasher.update(json!([metadata, title, started, updated, model, path]).to_string().as_bytes());
        for (_, record) in &records {
            let evidence = match record {
                NormalizedRecord::Message { timestamp, role, content, raw_json, .. } =>
                    json!([timestamp, role, content, raw_json]),
                NormalizedRecord::TurnEvent { timestamp, kind, raw_json, .. } =>
                    json!([timestamp, kind, raw_json]),
                _ => continue,
            };
            hasher.update(evidence.to_string().as_bytes());
        }
        let hash = hasher.finalize().to_hex().to_string();
        if prior.as_ref().is_some_and(|p| p.3 == hash) {
            report.skipped += 1;
            continue;
        }
        let path = path.to_string_lossy();
        let file_id: i64 = sqlx::query_scalar(
            "INSERT INTO source_files(instance_id,ingest_run_id,path,kind,parse_status,observed_at) VALUES(?,?,?,?,'ok',CURRENT_TIMESTAMP) ON CONFLICT(path) DO UPDATE SET ingest_run_id=excluded.ingest_run_id,observed_at=CURRENT_TIMESTAMP RETURNING id",
        ).bind(instance).bind(run).bind(path.as_ref()).bind(KIND).fetch_one(&mut *tx).await?;
        let stable_id = identity::session_id(source_key, &external);
        let raw = json!({"type":"codex_history","codex_history":metadata,"history_hash":hash});
        let session_id: i64 = sqlx::query_scalar(
            "INSERT INTO sessions(instance_id,source_file_id,external_id,stable_id,started_at,ended_at,model,status,title,derived_title,raw_metadata) VALUES(?,?,?,?,?,?,?,'unknown',?,?,?) ON CONFLICT(instance_id,external_id) DO UPDATE SET source_file_id=excluded.source_file_id,started_at=excluded.started_at,ended_at=excluded.ended_at,model=excluded.model,status=excluded.status,title=excluded.title,derived_title=excluded.derived_title,raw_metadata=excluded.raw_metadata RETURNING id",
        ).bind(instance).bind(file_id).bind(&external).bind(stable_id)
            .bind(started).bind(updated).bind(model).bind(title.as_deref().unwrap_or(&external))
            .bind(&title).bind(raw.to_string()).fetch_one(&mut *tx).await?;
        if prior.is_some() {
            for table in ["messages", "tool_calls", "usage_events", "session_usage", "turn_events", "turns"] {
                sqlx::query(&format!("DELETE FROM {table} WHERE session_id=?"))
                    .bind(session_id).execute(&mut *tx).await?;
            }
        }
        let mut records = SourceRecords::Buffered(records);
        match import_records(&mut tx, session_id, &mut records, sanitizer).await {
            Ok((messages, tools, _)) => {
                tx.commit().await?;
                report.messages += messages;
                report.tool_calls += tools;
                report.sessions += 1;
                indexed += 1;
            }
            Err(_) => { report.errors += 1; }
        }
    }
    history.close().await?;
    if indexed != 0 && diagnostics {
        println!("Codex SQLite: history indexed ({indexed} sessions updated).");
    }
    Ok(())
}

fn status(path: &Path, state: &str, sanitizer: &Sanitizer, diagnostics: bool) {
    if diagnostics {
        if sanitizer.active() {
            println!("Codex SQLite: {state}");
        } else {
            println!("Codex SQLite: {state} — {}", path.display());
        }
    }
}
