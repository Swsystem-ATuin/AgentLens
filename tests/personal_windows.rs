#![cfg(windows)]

use std::{
    fs,
    io::{Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const CODEX: &[u8] = include_bytes!("fixtures/codex/rollout.jsonl");
const GEMINI: &[u8] = include_bytes!("fixtures/gemini/session.jsonl");

fn personal(home: &Path, appdata: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-lens"));
    command
        .arg("personal")
        .env("USERPROFILE", home)
        .env("LOCALAPPDATA", appdata)
        .env("APPDATA", home.join("AppData/Roaming"));
    for key in [
        "AGENT_LENS_LOCAL_DIR",
        "AGENT_LENS_CODEX_HOME",
        "AGENT_LENS_GEMINI_HOME",
        "AGENT_LENS_HERMES_HOME",
        "AGENT_LENS_PROJECTS_FILE",
        "AGENT_LENS_REDACTION_FILE",
        "CODEX_HOME",
        "HERMES_HOME",
    ] {
        command.env_remove(key);
    }
    command
}

fn fixture(home: &Path, relative: &str, bytes: &[u8]) {
    let path = home.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

#[tokio::test]
async fn packaged_codex_history_is_discovered_and_reads_active_wal_without_duplicates() {
    use sqlx::{Connection, SqliteConnection, sqlite::{SqliteConnectOptions, SqliteJournalMode}};
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("profile");
    let redirected = directory.path().join("redirected local appdata");
    let packages = home.join("AppData/Local/Packages");
    let codex = packages.join("OpenAI.Codex_testpublisher/LocalCache/Roaming/Codex");
    let chat = packages.join("OpenAI.ChatGPT-Desktop_otherpublisher/LocalCache/Roaming/ChatGPT");
    fixture(&chat, "IndexedDB/placeholder.leveldb/marker", b"synthetic");
    fixture(&chat, "Session Storage/marker", b"synthetic");
    let database = codex.join("thread_history_17.sqlite");
    fs::create_dir_all(&codex).unwrap();
    let mut writer = SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(&database).create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal),
    ).await.unwrap();
    sqlx::raw_sql(
        "PRAGMA wal_autocheckpoint=0;
         CREATE TABLE threads(id TEXT PRIMARY KEY, name TEXT, originator TEXT, created_at_ms INTEGER, archived INTEGER, rollout_path TEXT);
         CREATE TABLE thread_items(thread_id TEXT, turn_id TEXT, item_id TEXT, item_type TEXT, item_json TEXT, ordinal INTEGER, updated_at_ordinal INTEGER);
         INSERT INTO threads VALUES('sqlite-only', 'Saved Work thread', 'codex_work_desktop', 1767225600000, 1, NULL);
         INSERT INTO thread_items VALUES('sqlite-only','turn-1','item-1','userMessage','{\"type\":\"userMessage\",\"content\":[{\"type\":\"text\",\"text\":\"First synthetic text\"}]}',1,1);
         INSERT INTO thread_items VALUES('sqlite-only','turn-1','item-2','reasoning','{\"type\":\"reasoning\",\"text\":\"Private reasoning fixture\"}',2,2);",
    ).execute(&mut writer).await.unwrap();

    let line = std::str::from_utf8(CODEX).unwrap().lines().next().unwrap();
    let meta: serde_json::Value = serde_json::from_str(line).unwrap();
    let duplicate = meta["payload"]["id"].as_str().unwrap();
    let compressed = zstd::stream::encode_all(CODEX, 1).unwrap();
    fixture(&home, ".codex/sessions/example.jsonl.zst", &compressed);
    sqlx::query("INSERT INTO threads VALUES(?, 'Catalog title', 'Codex Desktop', NULL, 0, ?)")
        .bind(duplicate).bind(home.join(".codex/sessions/example.jsonl").to_string_lossy().as_ref())
        .execute(&mut writer).await.unwrap();
    sqlx::query("INSERT INTO thread_items VALUES(?,'t','replica','agentMessage','{\"type\":\"agentMessage\",\"text\":\"Replicated body\"}',1,1)")
        .bind(duplicate).execute(&mut writer).await.unwrap();
    let wal = codex.join("thread_history_17.sqlite-wal");
    let before = fs::read(&wal).unwrap();
    assert!(!before.is_empty());

    let output = personal(&home, &redirected).arg("--index-only").output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
    assert!(stdout.contains("storage detected but unsupported"), "{stdout}");
    assert!(stdout.contains("browser session storage"), "{stdout}");
    assert!(stdout.contains("indexed 2 sessions"), "{stdout}");
    assert_eq!(fs::read(&wal).unwrap(), before);

    sqlx::query("UPDATE thread_items SET item_json='{\"type\":\"userMessage\",\"text\":\"Updated synthetic text\"}',updated_at_ordinal=3 WHERE item_id='item-1'")
        .execute(&mut writer).await.unwrap();
    let output = personal(&home, &redirected).args(["--index-only", "--refresh"]).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let mut index = SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(redirected.join("AgentLens/index.sqlite"))
            .read_only(true).immutable(true),
    ).await.unwrap();
    let histories: Vec<String> = sqlx::query_scalar(
        "SELECT m.content FROM messages m JOIN sessions s ON s.id=m.session_id WHERE s.external_id='sqlite-only'",
    ).fetch_all(&mut index).await.unwrap();
    assert_eq!(histories, ["Updated synthetic text"]);
    let origin: String = sqlx::query_scalar(
        "SELECT json_extract(raw_metadata,'$.codex_history.originator') FROM sessions WHERE external_id='sqlite-only'",
    ).fetch_one(&mut index).await.unwrap();
    assert_eq!(origin, "codex_work_desktop");
    let replicas: i64 = sqlx::query_scalar("SELECT count(*) FROM messages WHERE content='Replicated body'")
        .fetch_one(&mut index).await.unwrap();
    assert_eq!(replicas, 0);
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions").fetch_one(&mut index).await.unwrap();
    assert_eq!(sessions, 2);
    index.close().await.unwrap();
    writer.close().await.unwrap();
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn native_launcher_rebuilds_and_serves_usage_and_releases_its_port_and_lock() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("Synthetic user");
    let appdata = dir.path().join("Local AppData");
    let codex = ".codex/sessions/rollout.jsonl.zst";
    let gemini = ".gemini/tmp/test/chats/session.jsonl";
    let hermes = ".hermes/cron/output/synthetic-job/2026-01-01_12-00-00.md";
    let cron = b"# Synthetic run\n\nSynthetic Hermes output\n";
    let compressed = zstd::stream::encode_all(CODEX, 1).unwrap();
    fixture(&home, codex, &compressed);
    fixture(&home, gemini, GEMINI);
    fixture(&home, hermes, cron);

    let reserved = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = reserved.local_addr().unwrap();
    let port = address.port().to_string();
    let occupied = personal(&home, &appdata)
        .args(["--no-browser", "--port", &port])
        .output()
        .unwrap();
    assert!(!occupied.status.success());
    assert!(String::from_utf8_lossy(&occupied.stderr).contains("cannot bind 127.0.0.1"));
    drop(reserved);
    assert!(!appdata.join("AgentLens/index.sqlite").exists());

    let first = personal(&home, &appdata).arg("--index-only").output().unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("indexed 3 sessions"));

    let mut server = Server(
        personal(&home, &appdata)
            .args(["--no-browser", "--port", &port])
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "personal server exited early"
        );
        if let Ok(mut connection) = TcpStream::connect_timeout(&address, Duration::from_millis(100)) {
            connection
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            connection
                .write_all(b"GET /usage HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .unwrap();
            let mut response = String::new();
            connection.read_to_string(&mut response).unwrap();
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
            break;
        }
        assert!(Instant::now() < deadline, "usage server did not start");
        std::thread::sleep(Duration::from_millis(50));
    }

    let concurrent = personal(&home, &appdata)
        .arg("--index-only")
        .output()
        .unwrap();
    assert!(!concurrent.status.success());
    assert!(String::from_utf8_lossy(&concurrent.stderr).contains("cannot lock personal storage"));
    drop(server);
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(500)).is_err());

    let rebuild = personal(&home, &appdata)
        .arg("--index-only")
        .output()
        .unwrap();
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    assert!(String::from_utf8_lossy(&rebuild.stdout).contains("indexed 3 sessions"));
    assert_eq!(fs::read(home.join(codex)).unwrap(), compressed);
    assert_eq!(fs::read(home.join(gemini)).unwrap(), GEMINI);
    assert_eq!(fs::read(home.join(hermes)).unwrap(), cron);
}

#[test]
fn native_launcher_imports_archived_compressed_codex_beside_a_corrupt_rollout() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("Synthetic user");
    let appdata = dir.path().join("Local AppData");
    let archive = ".codex/archived_sessions/rollout.jsonl.zst";
    let corrupt = ".codex/sessions/broken.jsonl.zst";
    let ignored = ".codex/sessions/unrelated.zst";
    let mut frames = zstd::stream::encode_all(&CODEX[..CODEX.len() / 2], 1).unwrap();
    frames.extend(zstd::stream::encode_all(&CODEX[CODEX.len() / 2..], 1).unwrap());
    fixture(&home, archive, &frames);
    fixture(&home, corrupt, b"not a Zstandard frame");
    fixture(&home, ignored, b"unrelated");

    let output = personal(&home, &appdata).arg("--index-only").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary = String::from_utf8_lossy(&output.stdout);
    assert!(summary.contains("indexed 1 sessions"), "{summary}");
    assert!(summary.contains("; 1 errors;"), "{summary}");
    assert_eq!(fs::read(home.join(archive)).unwrap(), frames);
    assert_eq!(fs::read(home.join(corrupt)).unwrap(), b"not a Zstandard frame");
}

#[test]
fn native_launcher_rejects_missing_sources_and_storage_inside_a_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("Synthetic user");
    let appdata = dir.path().join("Local AppData");
    let missing = personal(&home, &appdata)
        .arg("--index-only")
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("No supported agent homes"));
    assert!(!appdata.exists());

    fixture(&home, ".codex/sessions/rollout.jsonl", CODEX);
    let overlap = home.join(".codex/Do not create");
    let output = personal(&home, &appdata)
        .env("AGENT_LENS_LOCAL_DIR", &overlap)
        .arg("--index-only")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must be separate directories"));
    assert!(!overlap.exists());
}

#[test]
fn native_launcher_refuses_hardlinked_storage_files() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("Synthetic user");
    let appdata = dir.path().join("Local AppData");
    let relative = ".codex/sessions/rollout.jsonl";
    fixture(&home, relative, CODEX);
    let storage = appdata.join("AgentLens");
    fs::create_dir_all(&storage).unwrap();
    fs::hard_link(home.join(relative), storage.join("index.sqlite")).unwrap();

    let output = personal(&home, &appdata).arg("--index-only").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("without other links"));
    assert_eq!(fs::read(home.join(relative)).unwrap(), CODEX);
}
