#![cfg(unix)]

use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn launcher(home: &Path, storage: &Path) -> Command {
    let mut command = Command::new("bash");
    command
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/local.sh"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", home)
        .env("AGENT_LENS_LOCAL_DIR", storage)
        .env("AGENT_LENS_BIN", env!("CARGO_BIN_EXE_agent-lens"));
    command
}

fn codex_home(home: &Path) {
    let sessions = home.join(".codex/sessions");
    fs::create_dir_all(&sessions).unwrap();
    fs::write(
        sessions.join("rollout.jsonl"),
        include_bytes!("fixtures/codex/rollout.jsonl"),
    )
    .unwrap();
}

fn executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[tokio::test]
async fn personal_launcher_combines_homes_and_rebuilds_without_changing_sources() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("synthetic home");
    let storage = dir.path().join("personal storage");
    codex_home(&home);
    let chats = home.join(".gemini/tmp/project/chats");
    fs::create_dir_all(&chats).unwrap();
    fs::write(
        chats.join("session.jsonl"),
        include_bytes!("fixtures/gemini/session.jsonl"),
    )
    .unwrap();
    fs::create_dir_all(home.join(".hermes/sessions")).unwrap();
    let mut identities = None;
    for _ in 0..2 {
        let output = launcher(&home, &storage)
            .arg("--index-only")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Inspect the completed snapshot without opening live WAL handles
        // before the next launcher process replaces it.
        let pool = sqlx::SqlitePool::connect_with(
            sqlx::sqlite::SqliteConnectOptions::new()
                .filename(storage.join("index.sqlite"))
                .read_only(true)
                .immutable(true),
        )
        .await
        .unwrap();
        let kinds: Vec<String> = sqlx::query_scalar("SELECT agents.kind FROM agent_instances JOIN agents ON agents.id=agent_instances.agent_id ORDER BY agents.kind").fetch_all(&pool).await.unwrap();
        assert_eq!(kinds, ["codex", "gemini", "hermes"]);
        let ids: Vec<String> =
            sqlx::query_scalar("SELECT stable_id FROM sessions ORDER BY stable_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(ids.len(), 2);
        if let Some(previous) = &identities {
            assert_eq!(&ids, previous, "reruns must preserve source identity");
        }
        identities = Some(ids);
        pool.close().await;
        assert!(!storage.join("launcher.lock").exists());
    }
    assert_eq!(
        fs::read(home.join(".codex/sessions/rollout.jsonl")).unwrap(),
        include_bytes!("fixtures/codex/rollout.jsonl")
    );
    assert_eq!(
        fs::read(chats.join("session.jsonl")).unwrap(),
        include_bytes!("fixtures/gemini/session.jsonl")
    );
    assert_eq!(
        fs::read_dir(home.join(".codex")).unwrap().count(),
        1,
        "launcher must not write metadata into sources"
    );
}

#[test]
fn personal_launcher_validates_inputs_before_writing_and_handles_missing_homes() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    fs::create_dir(&home).unwrap();
    let storage = dir.path().join("storage");
    let output = launcher(&home, &storage)
        .arg("--index-only")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("no supported agent homes"));
    assert!(!storage.exists());
    codex_home(&home);
    for port in ["0", "65536", "host:3001", "-1"] {
        let output = launcher(&home, &storage)
            .args(["--port", port])
            .output()
            .unwrap();
        assert!(!output.status.success(), "invalid port {port}");
        assert!(!storage.exists());
    }
    let output = launcher(
        &home,
        &dir.path().join("missing/../home/.codex/do not create"),
    )
    .arg("--index-only")
    .output()
    .unwrap();
    assert!(!output.status.success());
    assert!(!dir.path().join("missing").exists());
    let alias = dir.path().join("source alias");
    symlink(home.join(".codex"), &alias).unwrap();
    let output = launcher(&home, &alias.join("do not create"))
        .arg("--index-only")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!home.join(".codex/do not create").exists());
}

#[test]
fn personal_launcher_supports_custom_homes_and_macos_storage_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    fs::create_dir(&home).unwrap();
    let custom = dir.path().join("custom");
    codex_home(&custom);
    let tools = dir.path().join("tools");
    fs::create_dir(&tools).unwrap();
    executable(&tools.join("uname"), "#!/bin/sh\necho Darwin\n");
    let storage = home.join("Library/Application Support/AgentLens");
    let output = launcher(&home, &storage)
        .env_remove("AGENT_LENS_LOCAL_DIR")
        .env(
            "PATH",
            format!("{}:{}", tools.display(), std::env::var("PATH").unwrap()),
        )
        .env("CODEX_HOME", custom.join(".codex"))
        .arg("--index-only")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(storage.join("index.sqlite").exists());
    assert_eq!(
        fs::read_link(storage.join("sources/local/current/.codex")).unwrap(),
        custom.join(".codex").canonicalize().unwrap()
    );
}

#[test]
fn personal_launcher_does_not_serve_after_rebuild_failure_or_remove_unowned_paths() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    codex_home(&home);
    let storage = dir.path().join("storage");
    let binary = dir.path().join("fail rebuild");
    executable(
        &binary,
        "#!/bin/sh\ncase \"$*\" in *rebuild*) exit 7 ;; *) echo unexpected-serve; exit 0 ;; esac\n",
    );
    let output = launcher(&home, &storage)
        .env("AGENT_LENS_BIN", &binary)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("unexpected-serve"));
    assert!(!storage.join("launcher.lock").exists());
    fs::remove_file(storage.join("sources/local/current/.codex")).unwrap();
    fs::create_dir(storage.join("sources/local/current/.codex")).unwrap();
    let output = launcher(&home, &storage)
        .arg("--index-only")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(storage.join("sources/local/current/.codex").is_dir());
}

struct RunningLauncher(Child);
impl Drop for RunningLauncher {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status();
        let _ = self.0.wait();
    }
}

#[test]
fn personal_launcher_serves_usage_on_loopback_and_stops_child_on_exit() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    codex_home(&home);
    let storage = dir.path().join("storage");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut process = RunningLauncher(
        launcher(&home, &storage)
            .args(["--port", &address.port().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        assert!(
            process.0.try_wait().unwrap().is_none(),
            "launcher exited before serving usage"
        );
        if let Ok(mut stream) = TcpStream::connect(address) {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .write_all(b"GET /usage HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(
                response.starts_with("HTTP/1.1 200"),
                "usage must work without account provisioning"
            );
            break;
        }
        assert!(Instant::now() < deadline, "server did not become ready");
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(process);
    assert!(!storage.join("launcher.lock").exists());
    assert!(
        TcpStream::connect(address).is_err(),
        "launcher must stop its child server"
    );
}

#[test]
fn personal_launcher_build_failure_does_not_start_server_or_change_sources() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    codex_home(&home);
    let storage = dir.path().join("storage");
    let tools = dir.path().join("tools");
    fs::create_dir(&tools).unwrap();
    executable(
        &tools.join("cargo"),
        "#!/bin/sh\ncase \"$*\" in 'build --locked --manifest-path '*) exit 42 ;; *) exit 99 ;; esac\n",
    );
    let output = launcher(&home, &storage)
        .env_remove("AGENT_LENS_BIN")
        .env(
            "PATH",
            format!("{}:{}", tools.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(42),
        "launcher must build before rebuilding or serving"
    );
    assert!(!storage.join("index.sqlite").exists());
    assert!(!storage.join("launcher.lock").exists());
    assert_eq!(fs::read_dir(home.join(".codex")).unwrap().count(), 1);
}
