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
        .env("LOCALAPPDATA", appdata);
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
    let codex = ".codex/sessions/rollout.jsonl";
    let gemini = ".gemini/tmp/test/chats/session.jsonl";
    let hermes = ".hermes/cron/output/synthetic-job/2026-01-01_12-00-00.md";
    let cron = b"# Synthetic run\n\nSynthetic Hermes output\n";
    fixture(&home, codex, CODEX);
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
    assert_eq!(fs::read(home.join(codex)).unwrap(), CODEX);
    assert_eq!(fs::read(home.join(gemini)).unwrap(), GEMINI);
    assert_eq!(fs::read(home.join(hermes)).unwrap(), cron);
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
