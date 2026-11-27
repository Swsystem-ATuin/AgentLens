#![cfg(windows)]

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn launcher(home: &Path, appdata: &Path) -> Command {
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-File"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/local.ps1"))
        .env("USERPROFILE", home)
        .env("LOCALAPPDATA", appdata)
        .env("AGENT_LENS_BIN", env!("CARGO_BIN_EXE_agent-lens"));
    for key in [
        "AGENT_LENS_LOCAL_DIR",
        "AGENT_LENS_CODEX_HOME",
        "AGENT_LENS_GEMINI_HOME",
        "AGENT_LENS_HERMES_HOME",
        "AGENT_LENS_REDACTION_FILE",
        "AGENT_LENS_PROJECTS_FILE",
        "AGENT_LENS_PUBLIC_URL",
        "CODEX_HOME",
        "HERMES_HOME",
    ] {
        command.env_remove(key);
    }
    command
}

fn write_fixture(home: &Path, relative: &str, content: &[u8]) {
    let path = home.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

const CODEX: &[u8] = include_bytes!("fixtures/codex/rollout.jsonl");
const GEMINI: &[u8] = include_bytes!("fixtures/gemini/session.jsonl");

#[tokio::test]
async fn windows_launcher_rebuilds_all_homes_in_appdata_and_preserves_sources() {
    let sandbox = tempfile::tempdir().unwrap();
    let home = sandbox.path().join("synthetic home");
    let appdata = sandbox.path().join("local appdata");
    let custom = sandbox.path().join("custom Gemini [home]");
    let codex = ".codex/sessions/rollout.jsonl";
    let gemini = "tmp/project/chats/session.jsonl";
    write_fixture(&home, codex, CODEX);
    write_fixture(&custom, gemini, GEMINI);
    let hermes = home.join(".hermes");
    fs::create_dir(&hermes).unwrap();
    let state_path = hermes.join("state.db");
    let state = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&state_path)
                .create_if_missing(true),
        )
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TABLE sessions(id TEXT PRIMARY KEY,source TEXT NOT NULL,started_at REAL NOT NULL);
         CREATE TABLE messages(id INTEGER PRIMARY KEY,session_id TEXT NOT NULL,role TEXT NOT NULL);
         INSERT INTO sessions VALUES('synthetic-hermes','cli',1767261600);",
    )
    .execute(&state)
    .await
    .unwrap();
    state.close().await;
    let state_before = fs::read(&state_path).unwrap();

    let index = appdata.join("AgentLens/index.sqlite");
    let mut previous = None;
    for _ in 0..2 {
        let output = launcher(&home, &appdata)
            .env("AGENT_LENS_GEMINI_HOME", &custom)
            .arg("-IndexOnly")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let pool = SqlitePoolOptions::new()
            .connect_with(SqliteConnectOptions::new().filename(&index).read_only(true))
            .await
            .unwrap();
        let agents: Vec<String> = sqlx::query_scalar("SELECT kind FROM agents ORDER BY kind")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(agents, ["codex", "gemini", "hermes"]);
        let ids: Vec<String> =
            sqlx::query_scalar("SELECT stable_id FROM sessions ORDER BY stable_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(ids.len(), 3);
        if let Some(previous) = &previous {
            assert_eq!(&ids, previous);
        }
        previous = Some(ids);
        pool.close().await;
    }
    assert_eq!(fs::read(home.join(codex)).unwrap(), CODEX);
    assert_eq!(fs::read(custom.join(gemini)).unwrap(), GEMINI);
    assert_eq!(fs::read(state_path).unwrap(), state_before);
    assert_eq!(fs::read_dir(hermes).unwrap().count(), 1);
    assert!(!appdata.join("AgentLens/sources").exists());
}

#[test]
fn windows_launcher_refuses_missing_sources_overlapping_storage_and_busy_ports() {
    let sandbox = tempfile::tempdir().unwrap();
    let home = sandbox.path().join("home");
    let appdata = sandbox.path().join("appdata");
    fs::create_dir(&home).unwrap();
    let output = launcher(&home, &appdata).arg("-IndexOnly").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("No supported agent homes"));
    assert!(!appdata.exists());
    write_fixture(&home, ".codex/sessions/rollout.jsonl", CODEX);
    let inside = home.join(".codex/storage");
    let output = launcher(&home, &appdata)
        .env("AGENT_LENS_LOCAL_DIR", &inside)
        .arg("-IndexOnly")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("must be separate"));
    assert!(!inside.exists());

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    let output = launcher(&home, &appdata)
        .args(["-Port", &port])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("occupied or reserved"));
    assert!(!appdata.exists());
}

#[test]
fn windows_launcher_refuses_hardlinked_storage_files() {
    let sandbox = tempfile::tempdir().unwrap();
    let home = sandbox.path().join("home");
    let appdata = sandbox.path().join("appdata");
    let relative = ".codex/sessions/rollout.jsonl";
    write_fixture(&home, relative, CODEX);
    fs::create_dir_all(appdata.join("AgentLens")).unwrap();
    fs::hard_link(home.join(relative), appdata.join("AgentLens/index.sqlite")).unwrap();

    let output = launcher(&home, &appdata).arg("-IndexOnly").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Expected a regular launcher file"));
    assert_eq!(fs::read(home.join(relative)).unwrap(), CODEX);
}

const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
const CTRL_BREAK_EVENT: u32 = 1;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GenerateConsoleCtrlEvent(ctrl_event: u32, process_group_id: u32) -> i32;
}

struct RunningLauncher(Child);

impl Drop for RunningLauncher {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = Command::new("taskkill.exe")
                .args(["/T", "/F", "/PID", &self.0.id().to_string()])
                .output();
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[test]
fn windows_launcher_serves_usage_and_releases_server_and_lock_on_console_interrupt() {
    let sandbox = tempfile::tempdir().unwrap();
    let home = sandbox.path().join("home");
    let appdata = sandbox.path().join("appdata");
    write_fixture(&home, ".codex/sessions/rollout.jsonl", CODEX);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut process = RunningLauncher(
        launcher(&home, &appdata)
            .args(["-Port", &address.port().to_string()])
            .creation_flags(CREATE_NEW_PROCESS_GROUP)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            process.0.try_wait().unwrap().is_none(),
            "launcher exited early"
        );
        if let Ok(mut stream) = TcpStream::connect(address) {
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            stream
                .write_all(b"GET /usage HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(response.starts_with("HTTP/1.1 200"));
            break;
        }
        assert!(Instant::now() < deadline, "usage server did not become ready");
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = launcher(&home, &appdata).arg("-IndexOnly").output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Cannot lock personal storage"));

    // Ctrl-Break is the Windows console interrupt that can target just the
    // launcher's group. Ctrl-C would also interrupt the Cargo test runner.
    let delivered = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, process.0.id()) };
    assert_ne!(delivered, 0, "{}", std::io::Error::last_os_error());
    let deadline = Instant::now() + Duration::from_secs(10);
    while process.0.try_wait().unwrap().is_none() || TcpStream::connect(address).is_ok() {
        assert!(Instant::now() < deadline, "launcher left the server running");
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = launcher(&home, &appdata).arg("-IndexOnly").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
