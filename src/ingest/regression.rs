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
