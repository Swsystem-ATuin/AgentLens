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
        "SELECT external_id || ':' || raw_metadata FROM sessions ORDER BY external_id",
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
        let rebuilt = crate::db::rebuild(&dir.path().join("rebuilt.sqlite"))
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
    assert_eq!(
        ingest_data_root(&pool, dir.path()).await.unwrap().sessions,
        1
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
