use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Request, header},
};
use tower::ServiceExt;

fn cookies(response: &Response) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().split(';').next().unwrap())
        .collect::<Vec<_>>()
        .join("; ")
}
async fn get(app: &Router, path: &str, cookie: &str) -> Response {
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
async fn body(response: Response) -> String {
    assert_eq!(response.status(), StatusCode::OK);
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
fn csrf(html: &str) -> &str {
    html.split("name=\"csrf_token\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
}
async fn save(app: &Router, cookie: &str, csrf: &str, page: &str) -> Response {
    app.clone().oneshot(Request::builder().method("POST").uri("/account/preferences").header(header::COOKIE,cookie).header(header::CONTENT_TYPE,"application/x-www-form-urlencoded")
        .body(Body::from(format!("csrf_token={csrf}&timezone_mode=iana&timezone_name=Europe%2FLondon&date_time_format=long-24&appearance=dark&font_scale=125&page_size={page}&live_interval_seconds=30&live_paused=1&transcript_detail=compact&code_wrap=1&default_sessions_view=active"))).unwrap()).await.unwrap()
}

#[tokio::test]
async fn local_preferences_edit_without_accounts_are_browser_scoped_csrf_protected_and_survive_restart()
 {
    let dir = tempfile::tempdir().unwrap();
    let pool = crate::db::open(&dir.path().join("index.sqlite"))
        .await
        .unwrap();
    let state = crate::auth::StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    let mut local = state.clone();
    local.enable_local_mode();
    let app = session_router(pool.clone(), local);
    let account = get(&app, "/account", "").await;
    let initial_cookie = cookies(&account);
    let html = body(account).await;
    assert!(html.contains("Local mode is active"));
    assert!(
        html.contains("action=\"/account/preferences\""),
        "local mode must expose non-authentication preferences"
    );
    for hidden in [
        "action=\"/account/password\"",
        "action=\"/account/profile\"",
        "href=\"/account/mfa\"",
        "href=\"/account/tokens\"",
        "href=\"/account/sessions\"",
        "href=\"/admin/accounts\"",
        "Sign out",
    ] {
        assert!(
            !html.contains(hidden),
            "local mode must not expose identity/security control {hidden}"
        );
    }
    let response = save(&app, &initial_cookie, "wrong", "0").await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(!cookies(&response).contains("agent_lens_local_preferences="));
    let invalid = save(&app, &initial_cookie, csrf(&html), "999").await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert!(!cookies(&invalid).contains("agent_lens_local_preferences="));
    let response = save(&app, &initial_cookie, csrf(&html), "0").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response.headers()[header::LOCATION],
        "/account?preferences_saved=1"
    );
    let saved_cookie = cookies(&response);
    assert!(
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|value| value
                .to_str()
                .unwrap()
                .starts_with("agent_lens_local_preferences=")
                && value.to_str().unwrap().contains("HttpOnly")
                && value.to_str().unwrap().contains("Max-Age=31536000"))
    );
    let saved = body(get(&app, "/account?preferences_saved=1", &saved_cookie).await).await;
    for selected in [
        "value=\"dark\" selected",
        "value=\"0\" selected>All",
        "value=\"compact\" selected",
        "value=\"125\" selected",
        "value=\"Europe/London\"",
        "Preferences saved.",
    ] {
        assert!(
            saved.contains(selected),
            "missing saved local preference {selected}"
        );
    }
    assert!(
        body(get(&app, "/assets/preferences.css", &saved_cookie).await)
            .await
            .contains("font-size:125%")
    );
    assert!(
        body(get(&app, "/assets/preferences.js", &saved_cookie).await)
            .await
            .contains("liveIntervalMs: 30000")
    );
    let other = body(get(&app, "/account", "").await).await;
    assert!(
        other.contains("value=\"system\" selected") && other.contains("value=\"50\" selected"),
        "another browser must retain defaults"
    );
    for table in [
        "accounts",
        "account_preferences",
        "account_sessions",
        "security_events",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "local display changes must not create account/authentication state in {table}"
        );
    }
    // Display preferences are independent of anonymous CSRF/session expiry and state reopening.
    sqlx::query("DELETE FROM tower_sessions")
        .execute(&state.auth.pool)
        .await
        .unwrap();
    let mut reopened = crate::auth::StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    reopened.enable_local_mode();
    let app = session_router(pool.clone(), reopened);
    assert!(
        body(get(&app, "/account", &saved_cookie).await)
            .await
            .contains("value=\"0\" selected>All")
    );
    for path in [
        "/account/mfa",
        "/account/passkeys",
        "/admin/accounts",
        "/admin/audit",
    ] {
        assert_eq!(
            get(&app, path, &saved_cookie).await.status(),
            StatusCode::FORBIDDEN,
            "local settings must not enable {path}"
        );
    }
    // Switching back to authenticated serving ignores this unsigned, non-authentication cookie.
    let password = dir.path().join("password");
    std::fs::write(&password, "Synthetic1-password").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    crate::auth::create_account(&state, "alice", &password, None)
        .await
        .unwrap();
    let authenticated = session_router(pool, state);
    assert_eq!(
        get(&authenticated, "/account", &saved_cookie)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let auth_cookie = login_cookie(&authenticated, "alice", "Synthetic1-password").await;
    let local_cookie = saved_cookie
        .split("; ")
        .find(|value| value.starts_with("agent_lens_local_preferences="))
        .unwrap();
    let account = body(
        get(
            &authenticated,
            "/account",
            &format!("{auth_cookie}; {local_cookie}"),
        )
        .await,
    )
    .await;
    assert!(
        account.contains("value=\"system\" selected") && account.contains("value=\"50\" selected"),
        "local cookies cannot override authenticated account preferences"
    );
}
