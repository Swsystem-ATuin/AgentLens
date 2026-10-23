use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Request, header},
};
use tower::ServiceExt;

async fn get(app: &Router, cookie: &str, path: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn html(response: Response) -> String {
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn all_rows_preference_persists_and_renders_all_authorized_rows_and_transcript_events() {
    let dir = tempfile::tempdir().unwrap();
    let password = dir.path().join("password");
    std::fs::write(&password, "Synthetic1-password").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let state = crate::auth::StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    for user in ["admin", "viewer"] {
        crate::auth::create_account(&state, user, &password, None)
            .await
            .unwrap();
    }
    let source = format!("src_{}", "a".repeat(64));
    let forbidden = format!("src_{}", "b".repeat(64));
    crate::auth::grant_source(&state, "viewer", &source)
        .await
        .unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    sqlx::raw_sql("INSERT INTO agents VALUES(1,'codex','Synthetic'); INSERT INTO hosts VALUES(1,'host'); INSERT INTO users VALUES(1,1,'user');").execute(&pool).await.unwrap();
    for (id, key) in [(1, &source), (2, &forbidden)] {
        sqlx::query("INSERT INTO agent_instances(id,agent_id,user_id,source_home,source_key) VALUES(?,1,1,?,?)").bind(id).bind(format!("synthetic-{id}")).bind(key).execute(&pool).await.unwrap();
    }
    for index in 0..130 {
        sqlx::query("INSERT INTO sessions(id,instance_id,external_id,stable_id,title,started_at,model) VALUES(?,1,?,?,?,'2026-01-01','matching')")
            .bind(index+1).bind(format!("native-{index}")).bind(format!("all-{index:03}")).bind(format!("All row {index:03}")).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO messages(session_id,sequence,role,content) VALUES(1,?,'user',?)")
            .bind(index * 2 + 1)
            .bind(format!("Ordinary event {index:03}"))
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::raw_sql("INSERT INTO sessions(id,instance_id,external_id,stable_id,title,model) VALUES(1000,2,'forbidden','forbidden','Forbidden private row','matching'); INSERT INTO messages(session_id,sequence,role,content) VALUES(1,0,'developer','<multi_agent_role>Initial synthetic setup</multi_agent_role>'),(1,66,'developer','<multi_agent_role>Resumed synthetic setup</multi_agent_role>');").execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO messages(session_id,sequence,role,content) VALUES(1,1001,'assistant',?)",
    )
    .bind(format!(
        "Synthetic body prefix {}Synthetic body tail",
        "x".repeat(70000)
    ))
    .execute(&pool)
    .await
    .unwrap();
    let app = session_router(pool, state.clone());
    let cookie = login_cookie(&app, "viewer", "Synthetic1-password").await;
    let before = html(get(&app, &cookie, "/sessions").await).await;
    assert_eq!(before.matches("href=\"/sessions/all-").count(), 50);
    let account = html(get(&app, &cookie, "/account").await).await;
    assert!(account.contains("value=\"0\">All</option>"));
    let csrf = account
        .split("name=\"csrf_token\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let response=app.clone().oneshot(Request::builder().method("POST").uri("/account/preferences").header(header::COOKIE,&cookie).header(header::CONTENT_TYPE,"application/x-www-form-urlencoded")
        .body(Body::from(format!("csrf_token={csrf}&timezone_mode=utc&timezone_name=UTC&date_time_format=iso&appearance=system&font_scale=100&page_size=0&live_interval_seconds=5&live_paused=1&transcript_detail=standard&default_sessions_view=all"))).unwrap()).await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let account = html(get(&app, &cookie, "/account").await).await;
    assert!(account.contains("value=\"0\" selected>All</option>"));
    let rows = html(get(&app, &cookie, "/sessions?page=2").await).await;
    assert_eq!(
        rows.matches("href=\"/sessions/all-").count(),
        130,
        "All must not stop at the old 100-row maximum or start at page 2"
    );
    assert!(!rows.contains("Forbidden private row"));
    assert!(!rows.contains("rel=\"next\""));
    assert_eq!(
        get(&app, &cookie, "/sessions/forbidden").await.status(),
        StatusCode::NOT_FOUND
    );
    let transcript = html(get(&app, &cookie, "/sessions/all-000?section=2").await).await;
    assert!(
        transcript.contains("Ordinary event 000") && transcript.contains("Ordinary event 129"),
        "All must include the initial and final ordinary conversation events"
    );
    assert!(
        transcript.contains("Initial synthetic setup")
            && transcript.contains("Resumed synthetic setup")
    );
    assert!(
        !transcript.contains("section=2"),
        "All must not render transcript continuation links"
    );
    assert!(
        transcript.contains("Synthetic body prefix") && !transcript.contains("Synthetic body tail"),
        "All events must not disable independent message preview bounds"
    );
    let chunk = html(get(&app, &cookie, "/sessions/all-000/messages/1001").await).await;
    assert!(
        chunk.contains("part=2") && !chunk.contains("Synthetic body tail"),
        "message detail chunks must retain their independent continuation"
    );
    assert_eq!(
        transcript
            .matches("<details class=\"message developer\">")
            .count(),
        2,
        "initial and resumed setup must remain collapsed under All"
    );
    let filtered = html(get(&app, &cookie, "/sessions/all-000?event=tools").await).await;
    assert!(
        !filtered.contains("Ordinary event"),
        "All must still apply event filters"
    );
    for path in [
        "/account/sessions?page=2&events_page=2",
        "/account/access?page=2",
        "/account/tokens?page=2",
        "/usage?page=2",
        "/sessions/all-000/turns?page=2",
        "/sessions/all-000/messages/1",
        "/diagnostics",
    ] {
        assert_eq!(
            get(&app, &cookie, path).await.status(),
            StatusCode::OK,
            "All must keep pagination and evidence arithmetic safe on {path}"
        );
    }
    for path in ["/health?page=2", "/admin/accounts?page=2"] {
        assert_eq!(
            get(&app, &cookie, path).await.status(),
            StatusCode::FORBIDDEN,
            "All must never grant access to {path}"
        );
    }
    for index in 0..130 {
        sqlx::query("INSERT INTO accounts(id,username,password_hash,role) SELECT ?,?,password_hash,'viewer' FROM accounts WHERE username='viewer'")
            .bind(format!("synthetic-account-{index}")).bind(format!("synthetic-user-{index}")).execute(&state.auth.pool).await.unwrap();
    }
    let mut admin = state
        .auth
        .authenticate("admin", "Synthetic1-password")
        .await
        .unwrap()
        .unwrap()
        .0;
    admin.preferences.page_size = 0;
    crate::auth::update_preferences(&state.auth, &admin.id, &admin.preferences)
        .await
        .unwrap();
    let admin_cookie = login_cookie(&app, "admin", "Synthetic1-password").await;
    let accounts = html(get(&app, &admin_cookie, "/admin/accounts?page=2").await).await;
    assert_eq!(
        accounts.matches("href=\"/admin/accounts/").count(),
        132,
        "All also applies to administrative rows"
    );
    for path in ["/health?page=2", "/diagnostics", "/admin/audit?page=2"] {
        assert_eq!(
            get(&app, &admin_cookie, path).await.status(),
            StatusCode::OK,
            "All must render authorized {path}"
        );
    }
    let reopened = crate::auth::StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    assert_eq!(
        reopened
            .auth
            .authenticate("viewer", "Synthetic1-password")
            .await
            .unwrap()
            .unwrap()
            .0
            .preferences
            .page_size,
        0
    );
}
