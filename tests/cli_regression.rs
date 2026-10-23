use std::{
    path::Path,
    process::{Command, Output},
};

#[test]
fn quarter_hour_timer_override_replaces_inherited_triggers_and_schedules_real_quarter_hours() {
    let config = include_str!("../packaging/systemd/agent-lens-refresh-quarter-hour.conf");
    let lines: Vec<_> = config.lines().map(str::trim).collect();
    for reset in [
        "OnActiveSec=",
        "OnBootSec=",
        "OnStartupSec=",
        "OnUnitActiveSec=",
        "OnUnitInactiveSec=",
        "OnCalendar=",
    ] {
        assert!(
            lines.contains(&reset),
            "quarter-hour override must clear inherited trigger {reset}"
        );
    }
    assert!(
        lines.contains(&"Persistent=true"),
        "calendar timer must catch up after downtime"
    );
    let schedules: Vec<_> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("OnCalendar="))
        .filter(|value| !value.is_empty())
        .collect();
    assert_eq!(
        schedules,
        vec!["*-*-* *:00/15:00"],
        "use one calendar schedule, not a boot-relative or service-start-relative timer"
    );
    let output = match Command::new("systemd-analyze")
        .env("TZ", "UTC")
        .env("LC_ALL", "C")
        .args([
            "calendar",
            "--iterations=3",
            "--base-time=2026-01-01 00:01:00 UTC",
            schedules[0],
        ])
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => panic!("could not validate timer calendar: {error}"),
    };
    assert!(
        output.status.success(),
        "systemd rejected the calendar: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let schedule = String::from_utf8(output.stdout).expect("systemd calendar output is UTF-8");
    for expected in [
        "2026-01-01 00:15:00 UTC",
        "2026-01-01 00:30:00 UTC",
        "2026-01-01 00:45:00 UTC",
    ] {
        assert!(
            schedule.contains(expected),
            "actual systemd calendar must include {expected}: {schedule}"
        );
    }
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agent-lens"))
        .current_dir(root)
        .env_remove("AGENT_LENS_DB")
        .env_remove("AGENT_LENS_STATE_DB")
        .env_remove("AGENT_LENS_MFA_KEY_FILE")
        .env_remove("AGENT_LENS_SECURE_COOKIES")
        .env_remove("AGENT_LENS_PROJECTS_FILE")
        .env_remove("AGENT_LENS_DATA")
        .env_remove("AGENT_LENS_BIND")
        .env_remove("AGENT_LENS_LOCAL")
        .env_remove("AGENT_LENS_PUBLIC_URL")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn oidc_preflight_is_offline_does_not_open_databases_and_rejects_incomplete_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oidc.json");
    std::fs::write(&path, r#"{
        "issuer":"https://identity.example.test/tenant",
        "public_url":"https://lens.example.test",
        "client_id":"synthetic-client",
        "client_authentication":{"method":"client_secret_basic","secret_file":"/nonexistent/secret"},
        "account_linking":"administrator_provisioned_issuer_subject",
        "permissions":"local_only",
        "logout":"rp_initiated_and_back_channel_required",
        "local_break_glass":true
    }"#).unwrap();
    let result = run(dir.path(), &["oidc-check", "--config", "oidc.json"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("sign-in remains unavailable"));
    let deployment = run(
        dir.path(),
        &[
            "oidc-check",
            "--config",
            "oidc.json",
            "--prepare",
            "--deployment",
            "--public-url",
            "https://lens.example.test",
            "--secure-cookies",
        ],
    );
    assert!(!deployment.status.success());
    assert!(
        String::from_utf8_lossy(&deployment.stderr)
            .contains("open existing OIDC deployment state read-only"),
        "deployment prerequisites must fail before secret loading or network requests"
    );
    for extra in [
        vec!["--deployment"],
        vec!["--public-url", "https://lens.example.test"],
        vec![
            "--prepare",
            "--deployment",
            "--public-url",
            "https://lens.example.test",
        ],
    ] {
        let mut args = vec!["oidc-check", "--config", "oidc.json"];
        args.extend(extra);
        assert!(
            !run(dir.path(), &args).status.success(),
            "deployment declarations must be explicit and complete"
        );
    }
    let prepared = run(
        dir.path(),
        &["oidc-check", "--config", "oidc.json", "--prepare"],
    );
    assert!(!prepared.status.success());
    assert!(
        String::from_utf8_lossy(&prepared.stderr).contains("OIDC client secret requires"),
        "preparation must reject missing credentials before making a network request"
    );
    assert!(!String::from_utf8_lossy(&prepared.stderr).contains("/nonexistent/secret"));
    assert!(
        !run(
            dir.path(),
            &[
                "oidc-check",
                "--config",
                "oidc.json",
                "--prepare",
                "--discover"
            ]
        )
        .status
        .success()
    );
    std::fs::write(&path, "{}").unwrap();
    assert!(
        !run(dir.path(), &["oidc-check", "--config", "oidc.json"])
            .status
            .success()
    );
    let invalid_online = run(
        dir.path(),
        &["oidc-check", "--config", "oidc.json", "--discover"],
    );
    assert!(!invalid_online.status.success());
    assert!(
        String::from_utf8_lossy(&invalid_online.stderr)
            .contains("invalid OIDC configuration structure"),
        "discovery must validate local configuration before contacting a provider"
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "preflight must not create state, keys, or an index"
    );
}

#[tokio::test]
async fn oidc_cli_links_exact_subjects_without_disclosure_or_touching_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let password = dir.path().join("password");
    let subject = dir.path().join("subject");
    let marker = "Synthetic-Subject-'PrivateMarker";
    std::fs::write(&password, "Synthetic1-password").unwrap();
    std::fs::write(&subject, format!("{marker}\r\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in [&password, &subject] {
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    std::fs::write(
        dir.path().join("oidc.json"),
        r#"{
        "issuer":"https://identity.example.test/tenant",
        "public_url":"https://lens.example.test",
        "client_id":"synthetic-client",
        "client_authentication":{"method":"none"},
        "account_linking":"administrator_provisioned_issuer_subject",
        "permissions":"local_only",
        "logout":"rp_initiated_and_back_channel_required",
        "local_break_glass":true
    }"#,
    )
    .unwrap();
    let index_path = dir.path().join("agent-lens.sqlite");
    std::fs::write(&index_path, b"Synthetic index must remain untouched").unwrap();
    for username in ["admin", "member"] {
        assert!(
            run(
                dir.path(),
                &[
                    "account",
                    "create",
                    "--username",
                    username,
                    "--password-file",
                    "password"
                ]
            )
            .status
            .success()
        );
    }
    let linked = run(
        dir.path(),
        &[
            "account",
            "link-oidc",
            "--username",
            "member",
            "--config",
            "oidc.json",
            "--subject-file",
            "subject",
        ],
    );
    assert!(
        linked.status.success(),
        "{}",
        String::from_utf8_lossy(&linked.stderr)
    );
    assert!(!String::from_utf8_lossy(&linked.stdout).contains(marker));
    assert!(String::from_utf8_lossy(&linked.stdout).contains("sign-in remains unavailable"));
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("agent-lens-state.sqlite").display()
    ))
    .await
    .unwrap();
    let (id, stored): (String, String) =
        sqlx::query_as("SELECT id,subject FROM account_oidc_links")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored, marker,
        "subject must not be normalized or interpreted as SQL"
    );
    let listed = run(
        dir.path(),
        &["account", "list-oidc", "--username", "member"],
    );
    assert!(listed.status.success());
    let output = String::from_utf8_lossy(&listed.stdout);
    assert!(output.contains(&id));
    assert!(output.contains("https://identity.example.test/tenant"));
    assert!(!output.contains(marker));
    let duplicate = run(
        dir.path(),
        &[
            "account",
            "link-oidc",
            "--username",
            "admin",
            "--config",
            "oidc.json",
            "--subject-file",
            "subject",
        ],
    );
    assert!(!duplicate.status.success());
    assert!(!String::from_utf8_lossy(&duplicate.stderr).contains(marker));
    assert!(
        !run(
            dir.path(),
            &[
                "account",
                "unlink-oidc",
                "--username",
                "admin",
                "--link-id",
                &id
            ]
        )
        .status
        .success()
    );
    assert!(
        run(
            dir.path(),
            &[
                "account",
                "unlink-oidc",
                "--username",
                "member",
                "--link-id",
                &id
            ]
        )
        .status
        .success()
    );
    let generation: i64 =
        sqlx::query_scalar("SELECT session_generation FROM accounts WHERE username='member'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        generation, 2,
        "each successful identity change revokes account credentials"
    );
    let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM account_oidc_links")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(retained, 0);
    assert_eq!(
        std::fs::read(&index_path).unwrap(),
        b"Synthetic index must remain untouched"
    );
}

#[tokio::test]
async fn mfa_cli_rotates_custom_keys_and_recovers_missing_keys_without_touching_index() {
    let dir = tempfile::tempdir().unwrap();
    let password = dir.path().join("password");
    std::fs::write(&password, "Synthetic1-password").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(
        run(
            dir.path(),
            &[
                "account",
                "create",
                "--username",
                "admin",
                "--password-file",
                "password"
            ]
        )
        .status
        .success()
    );
    let rotated = run(
        dir.path(),
        &[
            "--mfa-key-file",
            "custom.mfa-keys.json",
            "account",
            "rotate-mfa-key",
        ],
    );
    assert!(
        rotated.status.success(),
        "{}",
        String::from_utf8_lossy(&rotated.stderr)
    );
    assert_eq!(
        String::from_utf8(rotated.stdout).unwrap().trim(),
        "MFA encryption key rotated; existing secrets rewrapped"
    );
    assert!(dir.path().join("custom.mfa-keys.json").exists());
    assert!(!dir.path().join("agent-lens-state.mfa-keys.json").exists());
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("agent-lens-state.sqlite").display()
    ))
    .await
    .unwrap();
    sqlx::query("INSERT INTO account_mfa(account_id,required,secret) SELECT id,1,'synthetic-unreadable-ciphertext' FROM accounts WHERE username='admin'").execute(&pool).await.unwrap();
    std::fs::rename(
        dir.path().join("custom.mfa-keys.json"),
        dir.path().join("saved-keyring.json"),
    )
    .unwrap();
    assert!(
        !run(
            dir.path(),
            &[
                "--mfa-key-file",
                "custom.mfa-keys.json",
                "account",
                "permissions",
                "--username",
                "admin"
            ]
        )
        .status
        .success()
    );
    let reset = run(
        dir.path(),
        &[
            "--mfa-key-file",
            "custom.mfa-keys.json",
            "account",
            "reset-mfa",
            "--username",
            "admin",
        ],
    );
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let status:(bool,bool,i64)=sqlx::query_as("SELECT m.required,m.secret IS NOT NULL,a.session_generation FROM account_mfa m JOIN accounts a ON a.id=m.account_id").fetch_one(&pool).await.unwrap();
    assert_eq!(status, (true, false, 1));
    assert!(!dir.path().join("agent-lens.sqlite").exists());
}

#[test]
fn serve_help_documents_loopback_port_and_session_options() {
    let dir = tempfile::tempdir().unwrap();
    let root = run(dir.path(), &["--help"]);
    assert!(root.status.success());
    let root_output = String::from_utf8(root.stdout).unwrap();
    assert!(root_output.contains("--state-database"));
    assert!(root_output.contains("--mfa-key-file"));
    assert!(root_output.contains("AGENT_LENS_STATE_DB"));
    assert!(root_output.contains("account"));
    let result = run(dir.path(), &["serve", "--help"]);
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("127.0.0.1:3001"));
    assert!(output.contains("AGENT_LENS_BIND"));
    assert!(output.contains("--secure-cookies"));
    assert!(output.contains("AGENT_LENS_SECURE_COOKIES"));
    assert!(output.contains("--projects-file"));
    assert!(output.contains("AGENT_LENS_PROJECTS_FILE"));
    assert!(output.contains("--local"));
    assert!(output.contains("AGENT_LENS_LOCAL"));

    let refresh = run(dir.path(), &["refresh", "--help"]);
    assert!(refresh.status.success());
    let refresh_output = String::from_utf8(refresh.stdout).unwrap();
    assert!(refresh_output.contains("AGENT_LENS_DATA"));

    let account = run(dir.path(), &["account", "--help"]);
    assert!(account.status.success());
    let account_output = String::from_utf8(account.stdout).unwrap();
    for command in [
        "create",
        "reset-password",
        "disable",
        "enable",
        "revoke-sessions",
        "set-role",
        "grant-source",
        "revoke-source",
        "permissions",
    ] {
        assert!(account_output.contains(command));
    }
}

#[test]
fn team_cli_explains_union_access_and_protects_the_last_administrator() {
    let dir = tempfile::tempdir().unwrap();
    let password = dir.path().join("password");
    std::fs::write(&password, "Synthetic1-password").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    for username in ["admin", "member"] {
        assert!(
            run(
                dir.path(),
                &[
                    "account",
                    "create",
                    "--username",
                    username,
                    "--password-file",
                    "password"
                ]
            )
            .status
            .success()
        );
    }
    let guard = run(dir.path(), &["account", "disable", "--username", "admin"]);
    assert!(!guard.status.success());
    assert!(String::from_utf8_lossy(&guard.stderr).contains("last enabled administrator"));
    for role in ["auditor", "operator", "viewer"] {
        assert!(
            run(
                dir.path(),
                &[
                    "account",
                    "set-role",
                    "--username",
                    "member",
                    "--role",
                    role
                ]
            )
            .status
            .success()
        );
    }
    let team = run(dir.path(), &["team", "create", "--name", "Synthetic team"]);
    assert!(team.status.success());
    let output = String::from_utf8(team.stdout).unwrap();
    let id = output.trim().strip_prefix("team: ").unwrap();
    assert!(id.starts_with("team_"));
    let source = format!("src_{}", "d".repeat(64));
    for args in [
        vec![
            "team",
            "add-member",
            "--team-id",
            id,
            "--username",
            "member",
        ],
        vec![
            "team",
            "grant-source",
            "--team-id",
            id,
            "--source-id",
            &source,
        ],
        vec![
            "account",
            "grant-source",
            "--username",
            "member",
            "--source-id",
            &source,
        ],
    ] {
        assert!(run(dir.path(), &args).status.success());
    }
    let inspect = run(dir.path(), &["team", "inspect", "--team-id", id]);
    assert!(inspect.status.success());
    let output = String::from_utf8(inspect.stdout).unwrap();
    assert!(output.contains("member: member"));
    assert!(output.contains(&format!("source: {source}")));
    let permissions = run(
        dir.path(),
        &["account", "permissions", "--username", "member"],
    );
    let output = String::from_utf8(permissions.stdout).unwrap();
    assert_eq!(
        output
            .lines()
            .filter(|line| line.starts_with("source:"))
            .count(),
        1
    );
    assert!(output.contains("via Direct grant"));
    assert!(output.contains("via Synthetic team"));
    assert!(
        run(
            dir.path(),
            &["team", "rename", "--team-id", id, "--name", "Renamed team"]
        )
        .status
        .success()
    );
    let listed = run(dir.path(), &["team", "list"]);
    assert!(
        String::from_utf8(listed.stdout)
            .unwrap()
            .contains("Renamed team")
    );
    assert!(
        !run(dir.path(), &["team", "list", "--page", "0"])
            .status
            .success()
    );
    assert!(
        run(dir.path(), &["team", "delete", "--team-id", id])
            .status
            .success()
    );
    let permissions = run(
        dir.path(),
        &["account", "permissions", "--username", "member"],
    );
    let output = String::from_utf8(permissions.stdout).unwrap();
    assert!(output.contains(&source));
    assert!(!output.contains("Renamed team"));
    assert!(!dir.path().join("agent-lens.sqlite").exists());
}

#[test]
fn invalid_project_configuration_fails_before_opening_the_database() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("projects.json"),
        r#"{"timezone":"named/zone"}"#,
    )
    .unwrap();
    let result = run(dir.path(), &["serve", "--projects-file", "projects.json"]);
    assert!(!result.status.success());
    assert!(!dir.path().join("agent-lens.sqlite").exists());
}

#[test]
fn serving_without_an_active_account_fails_before_creating_the_index() {
    let dir = tempfile::tempdir().unwrap();
    let result = run(dir.path(), &["serve"]);
    assert!(!result.status.success());
    assert!(!dir.path().join("agent-lens.sqlite").exists());
    assert!(dir.path().join("agent-lens-state.sqlite").exists());
    assert!(String::from_utf8_lossy(&result.stderr).contains("account create"));
}

#[test]
fn local_mode_rejects_non_loopback_bind_before_creating_state() {
    let dir = tempfile::tempdir().unwrap();
    let result = run(dir.path(), &["serve", "--local", "--bind", "0.0.0.0:3001"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("loopback bind address"));
    assert!(!dir.path().join("agent-lens-state.sqlite").exists());
    assert!(!dir.path().join("agent-lens.sqlite").exists());
}

#[test]
fn service_environment_selects_local_mode_and_bind_address() {
    let dir = tempfile::tempdir().unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_agent-lens"))
        .current_dir(dir.path())
        .env_remove("AGENT_LENS_DB")
        .env_remove("AGENT_LENS_STATE_DB")
        .env("AGENT_LENS_LOCAL", "true")
        .env("AGENT_LENS_BIND", "0.0.0.0:3001")
        .arg("serve")
        .output()
        .unwrap();

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("loopback bind address"));
    assert!(!dir.path().join("agent-lens-state.sqlite").exists());
}

#[tokio::test]
async fn account_cli_provisions_hashes_recovers_disables_and_revokes_without_touching_index() {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let password = dir.path().join("password");
    std::fs::write(&password, "First1! synthetic passphrase\n").unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    let create = run(
        dir.path(),
        &[
            "account",
            "create",
            "--username",
            "alice",
            "--password-file",
            "password",
        ],
    );
    assert!(
        create.status.success(),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );
    assert!(!dir.path().join("agent-lens.sqlite").exists());
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(dir.path().join("agent-lens-state.sqlite"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    let source = dir.path().join("sources/host/user/.codex/sessions");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("session.jsonl"),
        include_bytes!("fixtures/codex/rollout.jsonl"),
    )
    .unwrap();
    let rebuild = run(dir.path(), &["rebuild", "--data", "sources"]);
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );

    let state = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("agent-lens-state.sqlite").display()
    ))
    .await
    .unwrap();
    let (hash, enabled, generation, role): (String, bool, i64, String) = sqlx::query_as(
        "SELECT password_hash,enabled,session_generation,role FROM accounts WHERE username='alice'",
    )
    .fetch_one(&state)
    .await
    .unwrap();
    assert!(hash.starts_with("$argon2id$"));
    assert!(!hash.contains("First1! synthetic passphrase"));
    assert!(enabled);
    assert_eq!(generation, 0);
    assert_eq!(role, "admin");
    state.close().await;

    let create_viewer = run(
        dir.path(),
        &[
            "account",
            "create",
            "--username",
            "viewer",
            "--password-file",
            "password",
        ],
    );
    assert!(create_viewer.status.success());
    let index = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("agent-lens.sqlite").display()
    ))
    .await
    .unwrap();
    let source_id: String = sqlx::query_scalar("SELECT source_key FROM agent_instances")
        .fetch_one(&index)
        .await
        .unwrap();
    index.close().await;
    assert!(
        run(
            dir.path(),
            &[
                "account",
                "grant-source",
                "--username",
                "viewer",
                "--source-id",
                &source_id,
            ],
        )
        .status
        .success()
    );
    let rebuilt_with_grant = run(dir.path(), &["rebuild", "--data", "sources"]);
    assert!(
        rebuilt_with_grant.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuilt_with_grant.stderr)
    );
    let permissions = run(
        dir.path(),
        &["account", "permissions", "--username", "viewer"],
    );
    assert!(permissions.status.success());
    let permissions = String::from_utf8(permissions.stdout).unwrap();
    assert!(permissions.contains("role: viewer"));
    assert!(permissions.contains(&format!("source: {source_id}")));
    assert!(
        run(
            dir.path(),
            &[
                "account",
                "revoke-source",
                "--username",
                "viewer",
                "--source-id",
                &source_id,
            ],
        )
        .status
        .success()
    );
    assert!(
        run(
            dir.path(),
            &[
                "account",
                "set-role",
                "--username",
                "viewer",
                "--role",
                "admin",
            ],
        )
        .status
        .success()
    );

    #[cfg(unix)]
    {
        let exposed = dir.path().join("exposed-password");
        std::fs::write(&exposed, "private synthetic passphrase").unwrap();
        std::fs::set_permissions(&exposed, std::fs::Permissions::from_mode(0o644)).unwrap();
        let rejected = run(
            dir.path(),
            &[
                "account",
                "create",
                "--username",
                "bob",
                "--password-file",
                "exposed-password",
            ],
        );
        assert!(!rejected.status.success());
        assert!(
            !String::from_utf8_lossy(&rejected.stderr).contains("private synthetic passphrase")
        );
    }

    let duplicate = run(
        dir.path(),
        &[
            "account",
            "create",
            "--username",
            "ALICE",
            "--password-file",
            "password",
        ],
    );
    assert!(!duplicate.status.success());
    assert!(!String::from_utf8_lossy(&duplicate.stderr).contains("First1! synthetic passphrase"));
    assert!(
        run(
            dir.path(),
            &["account", "revoke-sessions", "--username", "alice"]
        )
        .status
        .success()
    );
    assert!(
        run(dir.path(), &["account", "disable", "--username", "alice"])
            .status
            .success()
    );
    assert!(
        run(dir.path(), &["account", "enable", "--username", "alice"])
            .status
            .success()
    );

    std::fs::write(&password, "Second2! synthetic passphrase").unwrap();
    let reset = run(
        dir.path(),
        &[
            "account",
            "reset-password",
            "--username",
            "alice",
            "--password-file",
            "password",
        ],
    );
    assert!(
        reset.status.success(),
        "{}",
        String::from_utf8_lossy(&reset.stderr)
    );
    let state = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("agent-lens-state.sqlite").display()
    ))
    .await
    .unwrap();
    let (hash, enabled, generation): (String, bool, i64) = sqlx::query_as(
        "SELECT password_hash,enabled,session_generation FROM accounts WHERE username='alice'",
    )
    .fetch_one(&state)
    .await
    .unwrap();
    assert!(hash.starts_with("$argon2id$"));
    assert!(!hash.contains("Second2! synthetic passphrase"));
    assert!(enabled);
    assert_eq!(generation, 3);
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

#[test]
fn health_command_reports_fixed_diagnostics_without_paths_or_parser_details() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("sources/host/user/.codex/sessions");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("PRIVATE_FILE.jsonl"),
        concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"synthetic-health\"}}\n",
            "PRIVATE MALFORMED CONTENT\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"text\":\"synthetic\"}]}}\n"
        ),
    )
    .unwrap();
    let refresh = run(
        dir.path(),
        &["--database", "index.sqlite", "refresh", "--data", "sources"],
    );
    assert!(
        refresh.status.success(),
        "{}",
        String::from_utf8_lossy(&refresh.stderr)
    );

    let result = run(dir.path(), &["--database", "index.sqlite", "health"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("latest ingest: complete"), "{output}");
    assert!(
        output.contains("sources: 1; ok 0; partial 1; error 0"),
        "{output}"
    );
    assert!(output.contains("reason=malformed_records"), "{output}");
    assert!(output.contains("file_"), "{output}");
    for private in [
        "PRIVATE_FILE",
        "PRIVATE MALFORMED CONTENT",
        dir.path().to_str().unwrap(),
    ] {
        assert!(!output.contains(private), "leaked {private}: {output}");
    }

    let missing = dir.path().join("PRIVATE_MISSING_ROOT");
    let failed = run(
        dir.path(),
        &[
            "--database",
            "index.sqlite",
            "refresh",
            "--data",
            missing.to_str().unwrap(),
        ],
    );
    assert!(!failed.status.success());
    let result = run(dir.path(), &["--database", "index.sqlite", "health"]);
    assert!(result.status.success());
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(output.contains("latest ingest: failed"), "{output}");
    assert!(output.contains("reason source_discovery"), "{output}");
    assert!(!output.contains("PRIVATE_MISSING_ROOT"), "{output}");
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
        let counts: (i64, i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM sessions),(SELECT count(*) FROM messages),(SELECT count(*) FROM ingest_runs),(SELECT count(*) FROM message_search WHERE message_search MATCH 'redacted')").fetch_one(&pool).await.unwrap();
        assert_eq!(counts, (1, 2, 1, 2));
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
async fn cli_rebuild_and_refresh_accept_a_direct_gemini_home() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("single/.gemini/tmp/project/chats");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("session.jsonl"),
        include_bytes!("fixtures/gemini/session.jsonl"),
    )
    .unwrap();
    let rebuild = run(
        dir.path(),
        &[
            "--database",
            "index.sqlite",
            "rebuild",
            "--data",
            "single/.gemini",
        ],
    );
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    assert!(
        String::from_utf8_lossy(&rebuild.stdout)
            .contains("indexed 1 sessions, 7 messages, 1 tool calls")
    );

    let refresh = run(
        dir.path(),
        &[
            "--database",
            "index.sqlite",
            "refresh",
            "--data",
            "single/.gemini",
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
    let origin: (String, String, String, String) = sqlx::query_as(
        "SELECT a.kind,h.name,u.os_user,(SELECT source_layout FROM ingest_runs WHERE status='complete' ORDER BY id DESC LIMIT 1) FROM agent_instances i JOIN agents a ON a.id=i.agent_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        origin,
        (
            "gemini".into(),
            String::new(),
            String::new(),
            "direct-gemini-home".into()
        )
    );
}

#[tokio::test]
async fn cli_rebuild_and_refresh_accept_a_direct_hermes_home() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("single/.hermes/sessions");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        source.join("request_dump_synthetic.json"),
        include_bytes!("fixtures/hermes/request_dump.json"),
    )
    .unwrap();
    let rebuild = run(
        dir.path(),
        &[
            "--database",
            "index.sqlite",
            "rebuild",
            "--data",
            "single/.hermes",
        ],
    );
    assert!(
        rebuild.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuild.stderr)
    );
    assert!(
        String::from_utf8_lossy(&rebuild.stdout)
            .contains("indexed 1 sessions, 5 messages, 1 tool calls")
    );

    let refresh = run(
        dir.path(),
        &[
            "--database",
            "index.sqlite",
            "refresh",
            "--data",
            "single/.hermes",
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
    let origin: (String, String, String, String) = sqlx::query_as(
        "SELECT a.kind,h.name,u.os_user,(SELECT source_layout FROM ingest_runs WHERE status='complete' ORDER BY id DESC LIMIT 1) FROM agent_instances i JOIN agents a ON a.id=i.agent_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        origin,
        (
            "hermes".into(),
            String::new(),
            String::new(),
            "direct-hermes-home".into()
        )
    );
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
    sqlx::raw_sql("DROP INDEX idx_agent_instances_source_key; DROP INDEX idx_sessions_activity_stable; DROP INDEX idx_sessions_stable_id; ALTER TABLE agent_instances DROP COLUMN source_key; ALTER TABLE sessions DROP COLUMN stable_id; ALTER TABLE sessions DROP COLUMN derived_title; ALTER TABLE source_files DROP COLUMN content_hash; ALTER TABLE source_files DROP COLUMN parser_version; ALTER TABLE ingest_runs DROP COLUMN source_layout;").execute(&pool).await.unwrap();
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
