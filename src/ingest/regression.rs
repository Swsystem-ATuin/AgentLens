use super::*;
use serde_json::json;

fn write_source(root: &Path, area: &str, name: &str, records: &[Value]) -> PathBuf {
    let directory = root
        .join("synthetic-host/synthetic-user/.codex")
        .join(area)
        .join("nested");
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(name);
    std::fs::write(
        &path,
        records
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    path
}

fn meta(id: &str) -> Value {
    json!({"type":"session_meta","timestamp":"2026-01-01T00:00:00Z","payload":{"id":id,"future_field":true}})
}

#[tokio::test]
async fn schema_variants_preserve_messages_tools_usage_and_raw_fields() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(
        dir.path(),
        "archived_sessions",
        "variants.jsonl",
        &[
            meta("synthetic-variants"),
            json!({"type":"future_record","payload":{"unknown":true}}),
            json!({"type":"response_item","payload":{"type":"message","future":42,"content":[{"text":"first"},{"type":"image"},{"text":"second"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":null}}),
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"a","name":"synthetic_function","arguments":"{}"}}),
            json!({"type":"response_item","payload":{"type":"custom_tool_call","id":"b","name":"synthetic_custom","input":"synthetic input"}}),
            json!({"type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"b","output":"custom result"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"a","output":"function result"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"unmatched","output":"ignored"}}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":null}}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":10,"output_tokens":5,"cached_input_tokens":3,"total_tokens":15}}}}),
            json!({"type":"event_msg","payload":{"type":"turn_aborted"}}),
        ],
    );
    let original = std::fs::read(&path).unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    let report = ingest_data_root(&pool, dir.path()).await.unwrap();
    assert_eq!(
        (
            report.sessions,
            report.messages,
            report.tool_calls,
            report.errors
        ),
        (1, 2, 2, 0)
    );
    let session: (String, Option<String>, String, String) =
        sqlx::query_as("SELECT started_at,model,status,raw_metadata FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (session.0.as_str(), session.1, session.2.as_str()),
        ("2026-01-01T00:00:00Z", None, "aborted")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&session.3).unwrap()["payload"]["future_field"],
        true
    );
    let messages: Vec<(String, String, String)> =
        sqlx::query_as("SELECT role,content,raw_json FROM messages ORDER BY sequence")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        (&*messages[0].0, &*messages[0].1),
        ("unknown", "first\nsecond")
    );
    assert_eq!(
        serde_json::from_str::<Value>(&messages[0].2).unwrap()["payload"]["future"],
        42
    );
    assert_eq!((&*messages[1].0, &*messages[1].1), ("assistant", ""));
    let calls: Vec<(String, String, String, String)> =
        sqlx::query_as("SELECT call_id,arguments,output,status FROM tool_calls ORDER BY sequence")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        calls,
        vec![
            (
                "a".into(),
                "{}".into(),
                "function result".into(),
                "complete".into()
            ),
            (
                "b".into(),
                "synthetic input".into(),
                "custom result".into(),
                "complete".into()
            )
        ]
    );
    let usage: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT input_tokens,output_tokens,cached_input_tokens,total_tokens FROM usage_events",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(usage, (10, 5, 3, 15));
    let source: (String, String) = sqlx::query_as("SELECT kind,parse_status FROM source_files")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(source, ("archived_sessions".into(), "ok".into()));
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[tokio::test]
async fn session_status_follows_ordered_lifecycle_and_resume_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let event = |kind: &str, timestamp: &str| json!({"type":"event_msg","timestamp":timestamp,"payload":{"type":kind}});
    let context = json!({"type":"turn_context","timestamp":"2025-01-01T00:00:00Z","payload":{}});
    let user_event = event("user_message", "2025-01-01T00:00:00Z");
    let cases = [
        (
            "complete-then-abort",
            vec![
                event("task_complete", "2026-01-01T00:00:00Z"),
                event("turn_aborted", "2025-01-01T00:00:00Z"),
            ],
            "aborted",
        ),
        (
            "abort-then-complete",
            vec![
                event("turn_aborted", "2026-01-01T00:00:00Z"),
                event("task_complete", "2025-01-01T00:00:00Z"),
            ],
            "complete",
        ),
        (
            "complete-then-started",
            vec![
                event("task_complete", "2026-01-01T00:00:00Z"),
                event("task_started", "2025-01-01T00:00:00Z"),
            ],
            "unknown",
        ),
        (
            "aborted-then-context",
            vec![event("turn_aborted", "2026-01-01T00:00:00Z"), context],
            "unknown",
        ),
        (
            "complete-then-user-message",
            vec![
                event("task_complete", "2026-01-01T00:00:00Z"),
                message("resumed"),
            ],
            "unknown",
        ),
        (
            "aborted-then-user-event",
            vec![event("turn_aborted", "2026-01-01T00:00:00Z"), user_event],
            "unknown",
        ),
        (
            "complete-then-unrelated",
            vec![
                event("task_complete", "2026-01-01T00:00:00Z"),
                event("token_count", "2025-01-01T00:00:00Z"),
            ],
            "complete",
        ),
        ("no-lifecycle", vec![message("initial")], "unknown"),
    ];
    for (id, records, _) in &cases {
        let mut source = vec![meta(id)];
        source.extend(records.iter().cloned());
        write_source(dir.path(), "sessions", &format!("{id}.jsonl"), &source);
    }

    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    let report = ingest_data_root(&pool, dir.path()).await.unwrap();
    assert_eq!((report.sessions, report.errors), (cases.len(), 0));
    let actual: std::collections::HashMap<String, String> =
        sqlx::query_as("SELECT external_id,status FROM sessions")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .collect();
    for (id, _, expected) in cases {
        assert_eq!(
            actual.get(id).map(String::as_str),
            Some(expected),
            "lifecycle status for {id}"
        );
    }
}

#[tokio::test]
async fn malformed_lines_partial_tails_and_bad_files_do_not_discard_good_records() {
    let dir = tempfile::tempdir().unwrap();
    let partial = write_source(
        dir.path(),
        "sessions",
        "partial.jsonl",
        &[meta("synthetic-partial")],
    );
    let content = format!(
        "{}\nnot-json\n{}\n{{",
        std::fs::read_to_string(&partial).unwrap(),
        json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":"surviving message"}]}})
    );
    std::fs::write(&partial, &content).unwrap();
    write_source(
        dir.path(),
        "sessions",
        "missing-meta.jsonl",
        &[json!({"type":"unknown"})],
    );
    write_source(
        dir.path(),
        "sessions",
        "missing-id.jsonl",
        &[json!({"type":"session_meta","payload":{}})],
    );
    write_source(
        dir.path(),
        "archived_sessions",
        "good.jsonl",
        &[meta("synthetic-good")],
    );
    write_source(
        dir.path(),
        "sessions",
        "ignored.txt",
        &[meta("not-a-rollout")],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    let report = ingest_data_root(&pool, dir.path()).await.unwrap();
    assert_eq!((report.sessions, report.messages, report.errors), (2, 1, 2));
    let errors: String =
        sqlx::query_scalar("SELECT parse_error FROM source_files WHERE parse_status='partial'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(errors.contains("line 2:"));
    assert!(errors.contains("line 4:"));
    let failures: i64 =
        sqlx::query_scalar("SELECT count(*) FROM source_files WHERE parse_status='error'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(failures, 2);
    let message: String = sqlx::query_scalar("SELECT content FROM messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(message, "surviving message");
    assert_eq!(std::fs::read_to_string(partial).unwrap(), content);
}

#[tokio::test]
async fn failed_run_finalization_rolls_back_all_index_writes() {
    let dir = tempfile::tempdir().unwrap();
    write_source(
        dir.path(),
        "sessions",
        "good.jsonl",
        &[meta("synthetic-rollback")],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    sqlx::raw_sql("INSERT INTO hosts(name) VALUES('preexisting'); CREATE TRIGGER fail_finalization BEFORE UPDATE ON ingest_runs BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;").execute(&pool).await.unwrap();
    assert!(ingest_data_root(&pool, dir.path()).await.is_err());
    for table in [
        "ingest_runs",
        "agents",
        "users",
        "agent_instances",
        "source_files",
        "sessions",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "failed ingest must roll back {table}");
    }
    let hosts: Vec<String> = sqlx::query_scalar("SELECT name FROM hosts")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(hosts, ["preexisting"]);
    sqlx::query("DROP TRIGGER fail_finalization")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        ingest_data_root(&pool, dir.path()).await.unwrap().sessions,
        1
    );
    let run: (String, i64, i64, bool) =
        sqlx::query_as("SELECT status,files_seen,errors,finished_at IS NOT NULL FROM ingest_runs")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(run, ("complete".into(), 1, 0, true));
}

fn message(text: &str) -> Value {
    json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"text":text}]}})
}

#[tokio::test]
async fn refresh_skips_verified_files_and_reimports_missing_hashes_and_parser_versions() {
    let dir = tempfile::tempdir().unwrap();
    write_source(
        dir.path(),
        "sessions",
        "refresh.jsonl",
        &[meta("refresh"), message("original")],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&pool, dir.path()).await.unwrap();
    let run: i64 = sqlx::query_scalar("SELECT ingest_run_id FROM source_files")
        .fetch_one(&pool)
        .await
        .unwrap();
    let report = ingest_data_root(&pool, dir.path()).await.unwrap();
    assert_eq!(
        (report.sessions, report.errors),
        (0, 0),
        "unchanged imports must be skipped, not fail on duplicate paths"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT ingest_run_id FROM source_files")
            .fetch_one(&pool)
            .await
            .unwrap(),
        run
    );
    for update in [
        "UPDATE source_files SET content_hash=NULL",
        "UPDATE source_files SET parser_version=0",
    ] {
        sqlx::query(update).execute(&pool).await.unwrap();
        let report = ingest_data_root(&pool, dir.path()).await.unwrap();
        assert_eq!((report.sessions, report.errors), (1, 0));
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM messages")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn refresh_detects_same_metadata_edits_and_preserves_session_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(
        dir.path(),
        "sessions",
        "refresh.jsonl",
        &[meta("refresh"), message("before")],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&pool, dir.path()).await.unwrap();
    let id: i64 = sqlx::query_scalar("SELECT id FROM sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    write_source(
        dir.path(),
        "sessions",
        "refresh.jsonl",
        &[meta("refresh"), message("after!")],
    );
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let report = ingest_data_root(&pool, dir.path()).await.unwrap();
    assert_eq!((report.sessions, report.errors), (1, 0));
    let row: (i64, String) = sqlx::query_as(
        "SELECT sessions.id,content FROM sessions JOIN messages ON messages.session_id=sessions.id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (id, "after!".into()));
}

#[tokio::test]
async fn refresh_failed_and_partial_replacements_preserve_previous_records_and_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(
        dir.path(),
        "sessions",
        "refresh.jsonl",
        &[meta("refresh"), message("original")],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&pool, dir.path()).await.unwrap();
    for broken in [
        "not-json".to_owned(),
        format!("{}\n{}\n{{", meta("refresh"), message("partial")),
    ] {
        std::fs::write(&path, broken).unwrap();
        for _ in 0..2 {
            assert_eq!(ingest_data_root(&pool, dir.path()).await.unwrap().errors, 1);
            assert_eq!(
                sqlx::query_scalar::<_, String>("SELECT content FROM messages")
                    .fetch_one(&pool)
                    .await
                    .unwrap(),
                "original"
            );
        }
    }
    write_source(
        dir.path(),
        "sessions",
        "refresh.jsonl",
        &[meta("refresh"), message("repaired")],
    );
    sqlx::raw_sql("CREATE TRIGGER fail_message BEFORE INSERT ON messages BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").execute(&pool).await.unwrap();
    assert_eq!(ingest_data_root(&pool, dir.path()).await.unwrap().errors, 1);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT content FROM messages")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "original"
    );
    sqlx::query("DROP TRIGGER fail_message")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        ingest_data_root(&pool, dir.path()).await.unwrap().sessions,
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT content FROM messages")
            .fetch_one(&pool)
            .await
            .unwrap(),
        "repaired"
    );
}

async fn projection(pool: &SqlitePool) -> Vec<String> {
    let mut out = Vec::new();
    for query in [
        "SELECT external_id || ':' || status || ':' || raw_metadata FROM sessions ORDER BY external_id",
        "SELECT external_id || ':' || sequence || ':' || raw_json FROM messages JOIN sessions ON sessions.id=messages.session_id ORDER BY external_id,sequence",
        "SELECT external_id || ':' || sequence || ':' || raw_json || ':' || coalesce(output,'') FROM tool_calls JOIN sessions ON sessions.id=tool_calls.session_id ORDER BY external_id,sequence",
        "SELECT external_id || ':' || usage_events.raw_json FROM usage_events JOIN sessions ON sessions.id=usage_events.session_id ORDER BY external_id,usage_events.raw_json",
    ] {
        out.extend(
            sqlx::query_scalar::<_, String>(query)
                .fetch_all(pool)
                .await
                .unwrap(),
        );
    }
    out
}

#[tokio::test]
async fn refresh_append_truncate_replace_archive_and_duplicates_match_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let mut records = vec![
        meta("changing"),
        message("initial"),
        json!({"type":"event_msg","payload":{"type":"task_complete"}}),
    ];
    let path = write_source(dir.path(), "sessions", "changing.jsonl", &records);
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&pool, dir.path()).await.unwrap();
    records.extend([
        message("resumed"),
        json!({"type":"response_item","payload":{"type":"function_call","call_id":"call","name":"synthetic","arguments":"{}"}}),
        json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"call","output":"synthetic output"}}),
        json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":12}}}}),
    ]);
    for current in [&records[..], &records[..2]] {
        write_source(dir.path(), "sessions", "changing.jsonl", current);
        assert_eq!(
            ingest_data_root(&pool, dir.path()).await.unwrap().sessions,
            1
        );
        let rebuilt = crate::db::open(&dir.path().join("rebuilt.sqlite"))
            .await
            .unwrap();
        ingest_data_root(&rebuilt, dir.path()).await.unwrap();
        assert_eq!(projection(&pool).await, projection(&rebuilt).await);
        rebuilt.close().await;
    }
    let archived = write_source(dir.path(), "archived_sessions", "changing.jsonl", &records);
    let report = ingest_data_root(&pool, dir.path()).await.unwrap();
    assert_eq!(
        (report.skipped, report.errors),
        (1, 1),
        "duplicate identities must not overwrite the indexed source"
    );
    std::fs::remove_file(&path).unwrap();
    let moved = ingest_data_root(&pool, dir.path()).await.unwrap();
    assert_eq!(
        (moved.sessions, moved.removed_sessions, moved.removed_files),
        (1, 0, 1)
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT kind FROM source_files JOIN sessions ON source_files.id=sessions.source_file_id"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        "archived_sessions"
    );
    std::fs::write(
        &archived,
        format!("{}\n{}", meta("replacement"), message("new identity")),
    )
    .unwrap();
    assert_eq!(
        ingest_data_root(&pool, dir.path()).await.unwrap().sessions,
        1
    );
    let rebuilt = crate::db::open(&dir.path().join("final.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&rebuilt, dir.path()).await.unwrap();
    assert_eq!(projection(&pool).await, projection(&rebuilt).await);
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn refresh_partial_utf8_tail_is_retried_and_completed() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(
        dir.path(),
        "sessions",
        "partial.jsonl",
        &[meta("partial"), message("good")],
    );
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.extend_from_slice(b"\n{\"unfinished\":\"\xc3");
    std::fs::write(&path, bytes).unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    assert_eq!(
        ingest_data_root(&pool, dir.path()).await.unwrap().sessions,
        1
    );
    assert_eq!(ingest_data_root(&pool, dir.path()).await.unwrap().errors, 1);
    write_source(
        dir.path(),
        "sessions",
        "partial.jsonl",
        &[meta("partial"), message("complete")],
    );
    assert_eq!(
        ingest_data_root(&pool, dir.path()).await.unwrap().sessions,
        1
    );
    assert_eq!(
        ingest_data_root(&pool, dir.path()).await.unwrap().skipped,
        1
    );
}

#[tokio::test]
async fn refresh_finalization_failure_rolls_back_replacements_and_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    write_source(
        dir.path(),
        "sessions",
        "refresh.jsonl",
        &[meta("refresh"), message("before")],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&pool, dir.path()).await.unwrap();
    let original = projection(&pool).await;
    let hash: String = sqlx::query_scalar("SELECT content_hash FROM source_files")
        .fetch_one(&pool)
        .await
        .unwrap();
    write_source(
        dir.path(),
        "sessions",
        "refresh.jsonl",
        &[meta("refresh"), message("after")],
    );
    sqlx::raw_sql("CREATE TRIGGER fail_refresh BEFORE UPDATE ON ingest_runs BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").execute(&pool).await.unwrap();
    assert!(ingest_data_root(&pool, dir.path()).await.is_err());
    assert_eq!(projection(&pool).await, original);
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT content_hash FROM source_files")
            .fetch_one(&pool)
            .await
            .unwrap(),
        hash
    );
    sqlx::query("DROP TRIGGER fail_refresh")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        ingest_data_root(&pool, dir.path()).await.unwrap().sessions,
        1
    );
}

#[tokio::test]
async fn refresh_removes_missing_sources_and_all_their_indexed_content() {
    let dir = tempfile::tempdir().unwrap();
    let records = [
        meta("removed"),
        message("synthetic removed content"),
        json!({"type":"response_item","payload":{"type":"function_call","call_id":"a","name":"synthetic","arguments":"{}"}}),
        json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"total_tokens":1}}}}),
    ];
    let path = write_source(dir.path(), "sessions", "removed.jsonl", &records);
    let error_path = write_source(
        dir.path(),
        "sessions",
        "error.jsonl",
        &[json!({"type":"unknown"})],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&pool, dir.path()).await.unwrap();
    for table in ["sessions", "messages", "tool_calls", "usage_events"] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(error_path).unwrap();
    ingest_data_root(&pool, dir.path()).await.unwrap();
    for table in [
        "sessions",
        "messages",
        "tool_calls",
        "usage_events",
        "source_files",
    ] {
        assert_eq!(
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap(),
            0,
            "refresh must remove missing source data from {table}"
        );
    }
    ingest_data_root(&pool, dir.path()).await.unwrap();
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn refresh_prunes_removed_homes_only_within_the_requested_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sources");
    let other = dir.path().join("sources-other");
    write_source(
        &root,
        "sessions",
        "first.jsonl",
        &[meta("first"), message("first")],
    );
    write_source(
        &other,
        "sessions",
        "second.jsonl",
        &[meta("second"), message("second")],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&pool, &root).await.unwrap();
    ingest_data_root(&pool, &other).await.unwrap();
    std::fs::remove_dir_all(root.join("synthetic-host")).unwrap();
    // Even a missing source in another root must not be pruned by this refresh.
    std::fs::remove_dir_all(other.join("synthetic-host")).unwrap();
    ingest_data_root(&pool, &root).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT external_id FROM sessions")
            .fetch_all(&pool)
            .await
            .unwrap(),
        ["second"]
    );
    ingest_data_root(&pool, &other).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn refresh_missing_root_or_invalid_discovery_preserves_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sources");
    let path = write_source(
        &root,
        "sessions",
        "kept.jsonl",
        &[meta("kept"), message("kept")],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&pool, &root).await.unwrap();
    let expected = projection(&pool).await;
    std::fs::rename(&root, dir.path().join("unavailable")).unwrap();
    assert!(
        ingest_data_root(&pool, &root).await.is_err(),
        "missing root must fail, not appear to be an empty snapshot"
    );
    assert_eq!(projection(&pool).await, expected);
    std::fs::rename(dir.path().join("unavailable"), &root).unwrap();
    std::fs::remove_file(&path).unwrap();
    let area = root.join("synthetic-host/synthetic-user/.codex/sessions");
    std::fs::remove_dir_all(&area).unwrap();
    std::fs::write(&area, "synthetic wrong file type").unwrap();
    assert!(
        ingest_data_root(&pool, &root).await.is_err(),
        "invalid source directory must fail discovery and roll back"
    );
    assert_eq!(projection(&pool).await, expected);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ingest_runs")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn refresh_pruning_rolls_back_if_run_finalization_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(
        dir.path(),
        "sessions",
        "kept.jsonl",
        &[meta("kept"), message("kept")],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&pool, dir.path()).await.unwrap();
    std::fs::remove_file(path).unwrap();
    sqlx::raw_sql("CREATE TRIGGER fail_prune BEFORE UPDATE ON ingest_runs BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").execute(&pool).await.unwrap();
    assert!(ingest_data_root(&pool, dir.path()).await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    sqlx::query("DROP TRIGGER fail_prune")
        .execute(&pool)
        .await
        .unwrap();
    ingest_data_root(&pool, dir.path()).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0,
        "retry must finish rolled-back deletions"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn refresh_symlink_loop_during_traversal_cancels_pruning() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(
        dir.path(),
        "sessions",
        "gone.jsonl",
        &[meta("gone"), message("kept on failure")],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&pool, dir.path()).await.unwrap();
    std::fs::remove_file(&path).unwrap();
    let link = path.parent().unwrap().join("synthetic-link");
    std::os::unix::fs::symlink("synthetic-link", &link).unwrap();
    let error = ingest_data_root(&pool, dir.path())
        .await
        .unwrap_err()
        .to_string();
    assert!(
        !error.contains("synthetic-link"),
        "discovery diagnostics should not disclose paths"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        1
    );
    std::fs::remove_file(link).unwrap();
    assert_eq!(
        ingest_data_root(&pool, dir.path())
            .await
            .unwrap()
            .removed_sessions,
        1
    );
}

#[tokio::test]
async fn refresh_file_parse_errors_do_not_prevent_removal_of_other_missing_sources() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(
        dir.path(),
        "sessions",
        "gone.jsonl",
        &[meta("gone"), message("gone")],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&pool, dir.path()).await.unwrap();
    std::fs::remove_file(path).unwrap();
    write_source(
        dir.path(),
        "sessions",
        "bad.jsonl",
        &[json!({"type":"unknown"})],
    );
    let report = ingest_data_root(&pool, dir.path()).await.unwrap();
    assert_eq!(
        (report.removed_sessions, report.removed_files, report.errors),
        (1, 1, 1)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn refresh_removes_a_source_file_replaced_by_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_source(
        dir.path(),
        "sessions",
        "gone.jsonl",
        &[meta("gone"), message("gone")],
    );
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    ingest_data_root(&pool, dir.path()).await.unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let report = ingest_data_root(&pool, dir.path()).await.unwrap();
    assert_eq!((report.removed_sessions, report.removed_files), (1, 1));
    assert!(projection(&pool).await.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn refresh_directory_symlinks_support_backup_rotation_and_broken_target_rollback() {
    use std::os::unix::fs::symlink;
    for level in [
        "",
        "synthetic-host",
        "synthetic-host/synthetic-user",
        "synthetic-host/synthetic-user/.codex",
        "synthetic-host/synthetic-user/.codex/sessions",
        "synthetic-host/synthetic-user/.codex/sessions/nested",
        "synthetic-host/synthetic-user/.codex/archived_sessions",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sources");
        let original = write_source(
            &root,
            if level.ends_with("archived_sessions") {
                "archived_sessions"
            } else {
                "sessions"
            },
            "rollout.jsonl",
            &[meta("backup"), message("first")],
        );
        let bytes = std::fs::read(&original).unwrap();
        let link = if level.is_empty() {
            root.clone()
        } else {
            root.join(level)
        };
        let backup = dir.path().join("nightly");
        std::fs::rename(&link, &backup).unwrap();
        symlink(&backup, &link).unwrap();
        let pool = crate::db::open(&dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let report = ingest_data_root(&pool, &root)
            .await
            .expect("directory symlinks must import at every source-tree level");
        assert_eq!((report.sessions, report.errors), (1, 0), "level {level}");
        assert_eq!(ingest_data_root(&pool, &root).await.unwrap().skipped, 1);
        let indexed: String = sqlx::query_scalar("SELECT path FROM source_files")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            indexed,
            original.to_string_lossy(),
            "retain logical source identity"
        );
        assert_eq!(
            std::fs::read(&original).unwrap(),
            bytes,
            "backup stays immutable"
        );

        let next = dir.path().join("next-nightly");
        std::fs::rename(&backup, &next).unwrap();
        assert!(
            ingest_data_root(&pool, &root).await.is_err(),
            "broken backup links must cancel refresh"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM sessions")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1,
            "broken links must not prune indexed sessions"
        );
        std::fs::remove_file(&link).unwrap();
        symlink(&next, &link).unwrap();
        assert_eq!(
            ingest_data_root(&pool, &root).await.unwrap().skipped,
            1,
            "retargeting preserves checkpoints"
        );
        std::fs::remove_file(&original).unwrap();
        assert_eq!(
            ingest_data_root(&pool, &root)
                .await
                .unwrap()
                .removed_sessions,
            1,
            "prune missing files in valid backup targets"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn refresh_relative_directory_links_reject_cycles_and_file_links() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("sources");
    let source = write_source(&root, "sessions", "rollout.jsonl", &[meta("relative")]);
    let sessions = source.parent().unwrap().parent().unwrap();
    let nested = sessions.join("nested");
    std::fs::rename(&nested, sessions.parent().unwrap().join("backup")).unwrap();
    symlink("../backup", &nested).unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    assert_eq!(ingest_data_root(&pool, &root).await.unwrap().sessions, 1);
    for target in ["..", "missing-backup", "../backup/rollout.jsonl"] {
        let link = sessions.join("invalid");
        symlink(target, &link).unwrap();
        assert!(
            ingest_data_root(&pool, &root).await.is_err(),
            "invalid traversal target {target} must cancel refresh"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ingest_runs")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1,
            "failed traversal must roll back the run"
        );
        std::fs::remove_file(link).unwrap();
    }
    assert_eq!(ingest_data_root(&pool, &root).await.unwrap().skipped, 1);
}
