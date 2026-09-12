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

#[test]
fn refresh_summary_distinguishes_partial_files_and_malformed_lines() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("sources/host/user/.codex/sessions");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("partial.jsonl"),
        concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"synthetic-partial\"}}\n",
            "private malformed content\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"text\":\"survives\"}]}}\n",
            "{"
        ),
    )
    .unwrap();

    let result = run(
        dir.path(),
        &["--database", "index.sqlite", "refresh", "--data", "sources"],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8(result.stdout).unwrap();
    assert!(
        stdout.contains("partial files: 1; malformed lines: 2"),
        "{stdout}"
    );
    assert!(!stdout.contains("private malformed content"), "{stdout}");
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
        let hash: String = sqlx::query_scalar("SELECT content_hash FROM source_files")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            hash,
            format!("blake3:{}", blake3::hash(fixture).to_hex()),
            "full rebuild must store the source checksum"
        );
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
    let args = ["--database", "index.sqlite", "refresh", "--data", "sources"];
    let result = run(dir.path(), &args);
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("skipped 1 unchanged files"));
    std::fs::remove_file(&path).unwrap();
    let result = run(dir.path(), &args);
    assert!(result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("removed 1 sessions and 1 source files")
    );
    assert!(!dir.path().join("agent-lens.sqlite").exists());
}

#[tokio::test]
async fn cli_rebuild_and_refresh_accept_a_direct_codex_home() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("single/.codex/sessions");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("rollout.jsonl"),
        include_bytes!("fixtures/codex/rollout.jsonl"),
    )
    .unwrap();
    let rebuild = run(
        dir.path(),
        &[
            "--database",
            "index.sqlite",
            "rebuild",
            "--data",
            "single/.codex",
        ],
    );
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    assert!(String::from_utf8_lossy(&rebuild.stdout).contains("indexed 1 sessions"));

    let refresh = run(
        dir.path(),
        &[
            "--database",
            "index.sqlite",
            "refresh",
            "--data",
            "single/.codex",
        ],
    );
    assert!(
        refresh.status.success(),
        "{}",
        String::from_utf8_lossy(&refresh.stderr)
    );
    assert!(String::from_utf8_lossy(&refresh.stdout).contains("skipped 1 unchanged files"));

    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("index.sqlite")),
        )
        .await
        .unwrap();
    let layouts: Vec<String> =
        sqlx::query_scalar("SELECT source_layout FROM ingest_runs ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(layouts, ["direct-codex-home", "direct-codex-home"]);
}

#[tokio::test]
async fn failed_cli_rebuild_preserves_the_previous_index_and_removes_staging_files() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("sources/host/user/.codex/sessions");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("rollout.jsonl"),
        include_bytes!("fixtures/codex/rollout.jsonl"),
    )
    .unwrap();
    let result = run(
        dir.path(),
        &["--database", "index.sqlite", "rebuild", "--data", "sources"],
    );
    assert!(result.status.success());

    let result = run(
        dir.path(),
        &[
            "--database",
            "index.sqlite",
            "rebuild",
            "--data",
            "missing-sources",
        ],
    );
    assert!(!result.status.success(), "missing discovery root must fail");

    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .connect_with(
            sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("index.sqlite")),
        )
        .await
        .unwrap();
    let counts: (i64, i64) =
        sqlx::query_as("SELECT (SELECT count(*) FROM sessions),(SELECT count(*) FROM messages)")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(counts, (1, 2), "the last usable index must remain intact");
    pool.close().await;
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|name| !name.contains(".rebuild-")),
        "failed rebuild must clean up its staged database: {names:?}"
    );
}

#[tokio::test]
async fn refresh_upgrades_legacy_database_and_skips_unchanged_files_without_auth() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("sources/host/user/.codex/sessions");
    std::fs::create_dir_all(&source).unwrap();
    let fixture = include_bytes!("fixtures/codex/rollout.jsonl");
    std::fs::write(source.join("rollout.jsonl"), fixture).unwrap();
    let args = ["--database", "index.sqlite", "refresh", "--data", "sources"];
    let result = run(dir.path(), &args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("indexed 1 sessions"));
    let options =
        sqlx::sqlite::SqliteConnectOptions::new().filename(dir.path().join("index.sqlite"));
    let pool = sqlx::SqlitePool::connect_with(options.clone())
        .await
        .unwrap();
    sqlx::raw_sql("DROP INDEX idx_agent_instances_source_key; DROP INDEX idx_sessions_stable_id; ALTER TABLE agent_instances DROP COLUMN source_key; ALTER TABLE sessions DROP COLUMN stable_id; ALTER TABLE sessions DROP COLUMN derived_title; ALTER TABLE source_files DROP COLUMN content_hash; ALTER TABLE source_files DROP COLUMN parser_version; ALTER TABLE ingest_runs DROP COLUMN source_layout;").execute(&pool).await.unwrap();
    pool.close().await;
    let result = run(dir.path(), &args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("indexed 1 sessions"));
    let result = run(dir.path(), &args);
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains(
        "skipped 1 unchanged files; 0 errors; partial files: 0; malformed lines: 0; 1 files seen"
    ));
    let pool = sqlx::SqlitePool::connect_with(options).await.unwrap();
    let row: (i64, i64, String, String, String) = sqlx::query_as("SELECT (SELECT count(*) FROM sessions),(SELECT count(*) FROM messages),content_hash,(SELECT stable_id FROM sessions),(SELECT title FROM sessions) FROM source_files").fetch_one(&pool).await.unwrap();
    assert_eq!((row.0, row.1), (1, 2));
    assert_eq!(row.2, format!("blake3:{}", blake3::hash(fixture).to_hex()));
    assert!(row.3.starts_with("ses_"));
    assert!(!row.4.is_empty());
    assert_eq!(
        std::fs::read(source.join("rollout.jsonl")).unwrap(),
        fixture
    );
}
