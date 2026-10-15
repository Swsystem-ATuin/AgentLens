//! Synthetic browser and transactional regressions for the administration boundary.
use super::tests::login_cookie;
use super::*;
use crate::{
    admin::{self, Command, Operation},
    auth::{self, AccountRole, StateStore},
};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use tower::ServiceExt;

const PASSWORD: &str = "Synthetic1-password";

pub(super) async fn fixture() -> (tempfile::TempDir, StateStore, Router) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("password");
    std::fs::write(&path, PASSWORD).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let state = StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    auth::create_account(&state, "admin", &path, None)
        .await
        .unwrap();
    auth::create_account(&state, "member", &path, None)
        .await
        .unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    let app = session_router(pool, state.clone());
    (dir, state, app)
}
pub(super) async fn get(app: &Router, cookie: &str, path: &str) -> (StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}
pub(super) fn csrf(html: &str) -> String {
    html.split("name=\"csrf_token\" value=\"")
        .nth(1)
        .expect("admin form CSRF token")
        .split('"')
        .next()
        .unwrap()
        .into()
}
fn command(operation: Operation, target: &str, confirmation: &str, value: &str) -> Command {
    Command {
        operation,
        target: target.into(),
        confirmation: confirmation.into(),
        value: value.into(),
        expected: String::new(),
        new_password: String::new(),
    }
}
pub(super) async fn account_command(
    state: &StateStore,
    username: &str,
    operation: Operation,
    value: &str,
) -> Command {
    let id = auth::account_id(state, username).await.unwrap();
    let target = admin::account(&state.auth, &id).await.unwrap().unwrap();
    let mut cmd = command(operation, &id, username, value);
    cmd.expected = target.version();
    cmd
}
pub(super) async fn post(
    app: &Router,
    cookie: &str,
    csrf: &str,
    password: &str,
    cmd: &Command,
) -> Response {
    let body = [
        ("csrf_token", csrf),
        ("current_password", password),
        ("operation", cmd.operation.as_str()),
        ("target", &cmd.target),
        ("confirmation", &cmd.confirmation),
        ("expected", &cmd.expected),
        ("value", &cmd.value),
        ("new_password", &cmd.new_password),
    ]
    .into_iter()
    .map(|(k, v)| format!("{k}={}", percent_encode_component(v)))
    .collect::<Vec<_>>()
    .join("&");
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/actions")
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn success(app: &Router, cookie: &str, csrf: &str, cmd: &Command) {
    let response = post(app, cookie, csrf, PASSWORD, cmd).await;
    let status = response.status();
    let body = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    assert_eq!(
        status,
        StatusCode::SEE_OTHER,
        "{}: {}",
        cmd.operation.as_str(),
        String::from_utf8_lossy(&body)
    );
}

#[tokio::test]
async fn browser_admin_actions_are_confirmed_atomic_audited_and_secret_free() {
    let (_dir, state, app) = fixture().await;
    let cookie = login_cookie(&app, "admin", PASSWORD).await;
    let (status, html) = get(&app, &cookie, "/admin/accounts").await;
    assert_eq!(status, StatusCode::OK);
    let csrf = csrf(&html);
    let create = command(
        Operation::CreateTeam,
        "<b>Synthetic team</b>",
        "<b>Synthetic team</b>",
        "",
    );
    assert_eq!(
        post(&app, &cookie, "wrong", PASSWORD, &create)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(&app, &cookie, &csrf, "wrong", &create).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM teams")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap(),
        0
    );
    success(&app, &cookie, &csrf, &create).await;
    let team = crate::access::list(&state.auth.pool, 0, 10)
        .await
        .unwrap()
        .remove(0);
    let (_, html) = get(&app, &cookie, "/admin/teams").await;
    assert!(html.contains("&#60;b&#62;Synthetic team&#60;/b&#62;"));
    assert!(!html.contains("<b>Synthetic team</b>"));
    let source = format!("src_{}", "a".repeat(64));
    for (op, value) in [
        (Operation::AddMember, "member"),
        (Operation::GrantTeamSource, &source),
    ] {
        success(
            &app,
            &cookie,
            &csrf,
            &command(op, &team.id, &team.name, value),
        )
        .await;
    }
    let member_id = auth::account_id(&state, "member").await.unwrap();
    let (_, html) = get(&app, &cookie, &format!("/admin/accounts/{member_id}")).await;
    assert!(html.contains(&source));
    assert!(html.contains(&team.id));
    success(
        &app,
        &cookie,
        &csrf,
        &account_command(&state, "member", Operation::GrantSource, &source).await,
    )
    .await;
    success(
        &app,
        &cookie,
        &csrf,
        &command(Operation::RemoveMember, &team.id, &team.name, "member"),
    )
    .await;
    assert_eq!(
        auth::account_permissions(&state, "member")
            .await
            .unwrap()
            .source_keys,
        vec![source.clone()]
    );
    success(
        &app,
        &cookie,
        &csrf,
        &account_command(&state, "member", Operation::RevokeSource, &source).await,
    )
    .await;
    success(
        &app,
        &cookie,
        &csrf,
        &command(Operation::RevokeTeamSource, &team.id, &team.name, &source),
    )
    .await;
    success(
        &app,
        &cookie,
        &csrf,
        &command(
            Operation::RenameTeam,
            &team.id,
            &team.name,
            "Renamed synthetic team",
        ),
    )
    .await;
    assert_eq!(
        post(
            &app,
            &cookie,
            &csrf,
            PASSWORD,
            &command(Operation::DeleteTeam, &team.id, &team.name, "")
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    success(
        &app,
        &cookie,
        &csrf,
        &command(
            Operation::DeleteTeam,
            &team.id,
            "Renamed synthetic team",
            "",
        ),
    )
    .await;

    let user = state
        .auth
        .authenticate("member", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    let bearer = crate::tokens::create(
        &state.auth,
        &user,
        "<i>Synthetic token</i>",
        "reports:read",
        1,
    )
    .await
    .unwrap();
    let token_id = crate::tokens::list(&state.auth.pool, &member_id, 0, 10)
        .await
        .unwrap()
        .remove(0)
        .id;
    let (_, html) = get(&app, &cookie, &format!("/admin/accounts/{member_id}")).await;
    assert!(html.contains("&#60;i&#62;Synthetic token&#60;/i&#62;"));
    assert!(!html.contains(&bearer));
    assert!(!html.contains("$argon2"));
    success(
        &app,
        &cookie,
        &csrf,
        &account_command(&state, "member", Operation::RevokeToken, &token_id).await,
    )
    .await;
    assert!(matches!(
        crate::tokens::authenticate(
            &state.auth,
            &bearer,
            crate::authorization::Capability::ReportsRead
        )
        .await
        .unwrap(),
        crate::tokens::Authentication::Invalid
    ));
    let member_cookie = login_cookie(&app, "member", PASSWORD).await;
    success(
        &app,
        &cookie,
        &csrf,
        &account_command(&state, "member", Operation::Disable, "").await,
    )
    .await;
    assert_eq!(
        get(&app, &member_cookie, "/account").await.0,
        StatusCode::UNAUTHORIZED
    );
    success(
        &app,
        &cookie,
        &csrf,
        &account_command(&state, "member", Operation::Enable, "").await,
    )
    .await;
    let member_cookie = login_cookie(&app, "member", PASSWORD).await;
    success(
        &app,
        &cookie,
        &csrf,
        &account_command(&state, "member", Operation::RevokeSessions, "").await,
    )
    .await;
    assert_eq!(
        get(&app, &member_cookie, "/account").await.0,
        StatusCode::UNAUTHORIZED
    );

    let mut reset = account_command(&state, "member", Operation::ResetPassword, "").await;
    reset.new_password = "Replacement1-password".into();
    success(&app, &cookie, &csrf, &reset).await;
    assert!(
        state
            .auth
            .authenticate("member", PASSWORD)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        state
            .auth
            .authenticate("member", &reset.new_password)
            .await
            .unwrap()
            .is_some()
    );
    let mut create = command(Operation::CreateAccount, "created", "created", "auditor");
    create.new_password = "Created1-password".into();
    success(&app, &cookie, &csrf, &create).await;
    assert_eq!(
        state
            .auth
            .authenticate("created", &create.new_password)
            .await
            .unwrap()
            .unwrap()
            .0
            .access
            .role(),
        "auditor"
    );
    let stale = account_command(&state, "member", Operation::SetRole, "viewer").await;
    success(
        &app,
        &cookie,
        &csrf,
        &account_command(&state, "member", Operation::SetRole, "operator").await,
    )
    .await;
    assert_eq!(
        post(&app, &cookie, &csrf, PASSWORD, &stale).await.status(),
        StatusCode::CONFLICT
    );
    let mut wrong = account_command(&state, "member", Operation::SetRole, "viewer").await;
    wrong.confirmation = "admin".into();
    assert_eq!(
        post(&app, &cookie, &csrf, PASSWORD, &wrong).await.status(),
        StatusCode::CONFLICT
    );
    for op in [Operation::Disable, Operation::SetRole] {
        assert_eq!(
            post(
                &app,
                &cookie,
                &csrf,
                PASSWORD,
                &account_command(&state, "admin", op, "viewer").await
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
    }

    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM admin_events")
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
    sqlx::raw_sql("CREATE TRIGGER reject_admin_audit BEFORE INSERT ON admin_events BEGIN SELECT RAISE(ABORT,'synthetic audit failure'); END;").execute(&state.auth.pool).await.unwrap();
    assert_eq!(
        post(
            &app,
            &cookie,
            &csrf,
            PASSWORD,
            &account_command(&state, "member", Operation::SetRole, "viewer").await
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        auth::account_permissions(&state, "member")
            .await
            .unwrap()
            .role,
        "operator"
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM admin_events")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap(),
        before
    );
    sqlx::query("DROP TRIGGER reject_admin_audit")
        .execute(&state.auth.pool)
        .await
        .unwrap();
    assert!(
        sqlx::query("UPDATE admin_events SET after_value='changed'")
            .execute(&state.auth.pool)
            .await
            .is_err()
    );
    let (_, html) = get(&app, &cookie, "/admin/audit").await;
    for secret in [
        PASSWORD,
        &reset.new_password,
        &create.new_password,
        &bearer,
        "$argon2",
        "tower_session_id",
    ] {
        assert!(!html.contains(secret), "audit leaked secret material");
    }
    assert!(html.contains("set_role"));
    assert!(html.contains("operator"));
    assert!(html.contains(&member_id));
    let facts:(String,String)=sqlx::query_as("SELECT before_value,after_value FROM admin_events WHERE operation='set_role' ORDER BY id DESC LIMIT 1").fetch_one(&state.auth.pool).await.unwrap();
    assert_eq!(facts, ("viewer".into(), "operator".into()));
}

#[tokio::test]
async fn all_non_admin_credentials_local_mode_and_stale_administrators_are_denied() {
    let (_dir, mut state, app) = fixture().await;
    let admin_cookie = login_cookie(&app, "admin", PASSWORD).await;
    let (_, html) = get(&app, &admin_cookie, "/admin/accounts").await;
    let csrf = csrf(&html);
    let cmd = command(
        Operation::CreateTeam,
        "Denied synthetic",
        "Denied synthetic",
        "",
    );
    for role in [
        AccountRole::Viewer,
        AccountRole::Auditor,
        AccountRole::Operator,
    ] {
        auth::set_role(&state, "member", role).await.unwrap();
        let cookie = login_cookie(&app, "member", PASSWORD).await;
        for path in [
            "/admin/accounts",
            "/admin/teams",
            "/admin/audit",
            "/admin/accounts/unknown",
            "/admin/teams/unknown",
        ] {
            assert_eq!(
                get(&app, &cookie, path).await.0,
                StatusCode::FORBIDDEN,
                "{role:?}: {path}"
            );
        }
        assert_eq!(
            post(&app, &cookie, &csrf, PASSWORD, &cmd).await.status(),
            StatusCode::FORBIDDEN
        );
    }
    let admin = state
        .auth
        .authenticate("admin", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    let token = crate::tokens::create(
        &state.auth,
        &admin,
        "Synthetic admin ceiling",
        "reports:read",
        1,
    )
    .await
    .unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/admin/accounts")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::COOKIE, &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    auth::set_role(&state, "member", AccountRole::Admin)
        .await
        .unwrap();
    auth::set_role(&state, "admin", AccountRole::Viewer)
        .await
        .unwrap();
    assert_eq!(
        get(&app, &admin_cookie, "/admin/accounts").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&app, &admin_cookie, &csrf, PASSWORD, &cmd)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    auth::set_enabled(&state, "admin", false).await.unwrap();
    assert_eq!(
        get(&app, &admin_cookie, "/admin/accounts").await.0,
        StatusCode::UNAUTHORIZED
    );
    state.enable_local_mode();
    let local = session_router(
        crate::db::open(&_dir.path().join("local-index.sqlite"))
            .await
            .unwrap(),
        state,
    );
    assert_eq!(
        get(&local, "", "/admin/accounts").await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&local, "", &csrf, PASSWORD, &cmd).await.status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn writer_rechecks_live_actor_session_and_concurrent_target_state() {
    let (_dir, state, _app) = fixture().await;
    let (actor, generation) = state
        .auth
        .authenticate("admin", PASSWORD)
        .await
        .unwrap()
        .unwrap();
    let session =
        tower_sessions::Session::new(None, std::sync::Arc::new(state.sessions.clone()), None);
    auth::establish_session(&state.auth, &session, &actor, generation, None)
        .await
        .unwrap();
    let inventory = auth::current_inventory_id(&session).await.unwrap().unwrap();
    let one = account_command(&state, "member", Operation::SetRole, "operator").await;
    let two = account_command(&state, "member", Operation::SetRole, "auditor").await;
    let (a, b) = tokio::join!(
        admin::execute(&state.auth, &actor, &inventory, one),
        admin::execute(&state.auth, &actor, &inventory, two)
    );
    assert_eq!(
        usize::from(a.is_ok()) + usize::from(b.is_ok()),
        1,
        "stale concurrent target mutation must not win"
    );
    let cmd = command(
        Operation::CreateTeam,
        "Stale synthetic",
        "Stale synthetic",
        "",
    );
    sqlx::query("UPDATE account_sessions SET revoked_at=unixepoch() WHERE id=?")
        .bind(&inventory)
        .execute(&state.auth.pool)
        .await
        .unwrap();
    assert!(
        admin::execute(&state.auth, &actor, &inventory, cmd)
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM admin_events")
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let fresh_session =
        tower_sessions::Session::new(None, std::sync::Arc::new(state.sessions.clone()), None);
    auth::establish_session(&state.auth, &fresh_session, &actor, generation, None)
        .await
        .unwrap();
    let fresh_inventory = auth::current_inventory_id(&fresh_session)
        .await
        .unwrap()
        .unwrap();
    auth::set_role(&state, "member", AccountRole::Admin)
        .await
        .unwrap();
    auth::set_role(&state, "admin", AccountRole::Viewer)
        .await
        .unwrap();
    let cmd = command(
        Operation::CreateTeam,
        "Stale synthetic",
        "Stale synthetic",
        "",
    );
    assert!(
        admin::execute(&state.auth, &actor, &fresh_inventory, cmd)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn v6_migration_preserves_identity_sessions_tokens_teams_grants_and_preferences() {
    let (dir, state, app) = fixture().await;
    let cookie = login_cookie(&app, "admin", PASSWORD).await;
    let admin = state
        .auth
        .authenticate("admin", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    let bearer = crate::tokens::create(
        &state.auth,
        &admin,
        "Synthetic migration",
        "reports:read",
        1,
    )
    .await
    .unwrap();
    let team = crate::access::create(&state, "Synthetic retained team")
        .await
        .unwrap();
    crate::access::membership(&state, &team, "member", true)
        .await
        .unwrap();
    let source = format!("src_{}", "b".repeat(64));
    crate::access::grant(&state, &team, &source, true)
        .await
        .unwrap();
    auth::update_preferences(
        &state.auth,
        &admin.id,
        &crate::preferences::UserPreferences {
            page_size: 25,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    sqlx::raw_sql("DROP TRIGGER admin_events_no_update; DROP TABLE admin_events; UPDATE app_state_metadata SET value='6' WHERE key='schema_version';").execute(&state.auth.pool).await.unwrap();
    state.auth.pool.close().await;
    let state = StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    let current = state
        .auth
        .authenticate("admin", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(current.id, admin.id);
    assert_eq!(current.preferences.page_size, 25);
    assert_eq!(
        auth::account_permissions(&state, "member")
            .await
            .unwrap()
            .source_keys,
        vec![source]
    );
    assert!(matches!(
        crate::tokens::authenticate(
            &state.auth,
            &bearer,
            crate::authorization::Capability::ReportsRead
        )
        .await
        .unwrap(),
        crate::tokens::Authentication::Authorized(_)
    ));
    let app = session_router(
        crate::db::open(&dir.path().join("index.sqlite"))
            .await
            .unwrap(),
        state.clone(),
    );
    assert_eq!(
        get(&app, &cookie, "/admin/accounts").await.0,
        StatusCode::OK
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT value FROM app_state_metadata WHERE key='schema_version'"
        )
        .fetch_one(&state.auth.pool)
        .await
        .unwrap(),
        "8"
    );
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&state.auth.pool)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("INSERT INTO admin_events(actor_id,operation,subject,before_value,after_value,occurred_at) VALUES(?,'create_team','','absent','old synthetic',unixepoch()-181*86400)").bind(&admin.id).execute(&state.auth.pool).await.unwrap();
    state.auth.pool.close().await;
    let state = StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM admin_events")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap(),
        0,
        "startup must prune expired administrative facts"
    );
}

#[tokio::test]
async fn administration_pages_are_bounded_and_password_budget_does_not_lock_accounts() {
    let (_dir, state, app) = fixture().await;
    let cookie = login_cookie(&app, "admin", PASSWORD).await;
    let admin = state
        .auth
        .authenticate("admin", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    auth::update_preferences(
        &state.auth,
        &admin.id,
        &crate::preferences::UserPreferences {
            page_size: 25,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    for i in 0..30 {
        sqlx::query(
            "INSERT INTO accounts(id,username,password_hash) VALUES(?,?,'synthetic-unusable')",
        )
        .bind(format!("synthetic_{i}"))
        .bind(format!("synthetic-{i:02}"))
        .execute(&state.auth.pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO teams(id,name) VALUES(?,?)")
            .bind(format!("team_synthetic_{i}"))
            .bind(format!("Synthetic team {i:02}"))
            .execute(&state.auth.pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO admin_events(actor_id,operation,subject,before_value,after_value) VALUES(?,'create_team','','absent','synthetic')").bind(&admin.id).execute(&state.auth.pool).await.unwrap();
    }
    for path in ["/admin/accounts", "/admin/teams", "/admin/audit"] {
        let (status, html) = get(&app, &cookie, path).await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(&format!("{path}?page=2")));
        assert_eq!(
            get(&app, &cookie, &format!("{path}?page=0")).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let (_, html) = get(&app, &cookie, "/admin/accounts").await;
    assert_eq!(
        html.matches("<tr><td><a href=\"/admin/accounts/").count(),
        25
    );
    let csrf = csrf(&html);
    let cmd = command(
        Operation::CreateTeam,
        "Limited synthetic",
        "Limited synthetic",
        "",
    );
    while state.auth.accept_password_attempt() {}
    let response = post(&app, &cookie, &csrf, PASSWORD, &cmd).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.headers()[header::RETRY_AFTER], "60");
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "username=unknown&password=wrong&csrf_token={csrf}"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        state
            .auth
            .authenticate("admin", PASSWORD)
            .await
            .unwrap()
            .is_some(),
        "throttling must not disable an account or damage credentials"
    );
    assert_eq!(get(&app, &cookie, "/account").await.0, StatusCode::OK);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM admin_events WHERE after_value='Limited synthetic'"
        )
        .fetch_one(&state.auth.pool)
        .await
        .unwrap(),
        0
    );
}
