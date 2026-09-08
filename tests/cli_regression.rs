use std::{
    path::Path,
    process::{Command, Output},
};

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-lens"))
        .current_dir(root)
        .env_remove("AGENT_LENS_DB")
        .env_remove("AGENT_LENS_AUTH_FILE")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn serve_help_documents_loopback_port_and_authentication_options() {
    let dir = tempfile::tempdir().unwrap();
    let result = run(dir.path(), &["serve", "--help"]);
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("127.0.0.1:3001"));
    assert!(output.contains("--auth-file"));
    assert!(output.contains("AGENT_LENS_AUTH_FILE"));
}

#[test]
fn missing_or_invalid_authentication_fails_before_creating_database() {
    let dir = tempfile::tempdir().unwrap();
    for config in [None, Some("{}"), Some(r#"{"username":"","password":""}"#)] {
        if let Some(config) = config {
            std::fs::write(dir.path().join("auth.local.json"), config).unwrap();
        }
        let result = run(dir.path(), &["serve"]);
        assert!(!result.status.success());
        assert!(!dir.path().join("agent-lens.sqlite").exists());
    }
}

#[tokio::test]
async fn repeated_cli_rebuilds_reproduce_index_without_changing_sources_or_needing_auth() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("sources/host/user/.codex/sessions/nested");
    std::fs::create_dir_all(&source).unwrap();
    let fixture = include_bytes!("fixtures/codex/rollout.jsonl");
    let path = source.join("rollout.jsonl");
    std::fs::write(&path, fixture).unwrap();
    for _ in 0..2 {
        let result = run(
            dir.path(),
            &["--database", "index.sqlite", "rebuild", "--data", "sources"],
        );
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            String::from_utf8(result.stdout)
                .unwrap()
                .contains("indexed 1 sessions, 2 messages, 0 tool calls; 0 errors")
        );
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("index.sqlite")),
            )
            .await
            .unwrap();
        let counts: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM sessions),(SELECT count(*) FROM messages),(SELECT count(*) FROM ingest_runs)").fetch_one(&pool).await.unwrap();
        assert_eq!(counts, (1, 2, 1));
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(integrity, "ok");
        assert!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&pool)
                .await
                .unwrap()
                .is_empty()
        );
        pool.close().await;
        assert_eq!(std::fs::read(&path).unwrap(), fixture);
        assert_eq!(std::fs::read_dir(&source).unwrap().count(), 1);
    }
    assert!(!dir.path().join("agent-lens.sqlite").exists());
}
