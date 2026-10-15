use super::*;
use super::{
    administration::{account_command, csrf, fixture, get, post},
    tests::login_cookie,
};
use crate::{admin::Operation, auth, mfa};
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use totp_rs::{Algorithm, Secret, TOTP};
use tower::ServiceExt;
const PASSWORD: &str = "Synthetic1-password";

async fn form(app: &Router, cookie: &str, path: &str, fields: &[(&str, &str)]) -> Response {
    let body = fields
        .iter()
        .map(|(k, v)| format!("{k}={}", percent_encode_component(v)))
        .collect::<Vec<_>>()
        .join("&");
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap()
}
fn changed_cookie(response: &Response) -> String {
    response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .into()
}
async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
async fn mfa_action(app: &Router, cookie: &str, csrf: &str, action: &str, code: &str) -> Response {
    form(
        app,
        cookie,
        "/account/mfa",
        &[
            ("csrf_token", csrf),
            ("current_password", PASSWORD),
            ("action", action),
            ("mfa_code", code),
        ],
    )
    .await
}
async fn enroll(app: &Router, cookie: &str) -> (String, Vec<String>, String) {
    let (_, page) = get(app, cookie, "/account/mfa").await;
    let csrf = csrf(&page);
    let response = mfa_action(app, cookie, &csrf, "begin", "").await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = html(response).await;
    let secret = page
        .split("<pre>")
        .nth(1)
        .unwrap()
        .split("</pre>")
        .next()
        .unwrap()
        .to_owned();
    assert!(page.contains("otpauth://totp/"));
    let generator = TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        Secret::Encoded(secret.clone()).to_bytes().unwrap(),
        Some("Agent Lens".into()),
        "member".into(),
    )
    .unwrap();
    let code = generator.generate((auth::unix_now() + 30) as u64);
    let response = mfa_action(app, cookie, &csrf, "finish", &code).await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookie = changed_cookie(&response);
    let page = html(response).await;
    let codes = page
        .split("<pre>")
        .nth(1)
        .unwrap()
        .split("</pre>")
        .next()
        .unwrap()
        .lines()
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    assert_eq!(codes.len(), 10);
    (cookie, codes, secret)
}
async fn login_factor(app: &Router, username: &str, code: &str) -> Response {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let cookie = changed_cookie(&response);
    let page = html(response).await;
    let csrf = csrf(&page);
    form(
        app,
        &cookie,
        "/login",
        &[
            ("username", username),
            ("password", PASSWORD),
            ("mfa_code", code),
            ("csrf_token", &csrf),
            ("return_to", "/account"),
        ],
    )
    .await
}

#[tokio::test]
async fn opt_in_enrollment_login_recovery_and_disable_preserve_the_factor_boundary() {
    let (_dir, state, app) = fixture().await;
    let user = state
        .auth
        .authenticate("member", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    let old_token = crate::tokens::create(
        &state.auth,
        &user,
        "Synthetic before MFA",
        "reports:read",
        1,
    )
    .await
    .unwrap();
    let cookie = login_cookie(&app, "member", PASSWORD).await;
    let (_, page) = get(&app, &cookie, "/account/mfa").await;
    let old_csrf = csrf(&page);
    assert!(!page.contains("otpauth://"));
    assert_eq!(
        mfa_action(&app, &cookie, "wrong", "begin", "")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let bad = form(
        &app,
        &cookie,
        "/account/mfa",
        &[
            ("csrf_token", &old_csrf),
            ("current_password", "wrong"),
            ("action", "begin"),
        ],
    )
    .await;
    assert_eq!(bad.status(), StatusCode::FORBIDDEN);
    let (cookie, codes, secret) = enroll(&app, &cookie).await;
    let (_, page) = get(&app, &cookie, "/account/mfa").await;
    let csrf = csrf(&page);
    assert!(!page.contains(&secret));
    assert!(!page.contains("otpauth://"));
    for code in &codes {
        assert!(!page.contains(code));
    }
    assert!(matches!(
        crate::tokens::authenticate(
            &state.auth,
            &old_token,
            crate::authorization::Capability::ReportsRead
        )
        .await
        .unwrap(),
        crate::tokens::Authentication::Invalid
    ));
    assert_eq!(get(&app, &cookie, "/account").await.0, StatusCode::OK);
    for (name, code) in [("member", ""), ("unknown", ""), ("member", "invalid")] {
        let response = login_factor(&app, name, code).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(
            response.headers()[header::LOCATION]
                .to_str()
                .unwrap()
                .starts_with("/login?error=1")
        );
    }
    let response = login_factor(&app, "member", &codes[0]).await;
    assert_eq!(response.headers()[header::LOCATION], "/account");
    let recovered_cookie = changed_cookie(&response);
    assert_eq!(
        get(&app, &recovered_cookie, "/account").await.0,
        StatusCode::OK
    );
    let response = login_factor(&app, "member", &codes[0]).await;
    assert!(
        response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .starts_with("/login?error=1")
    );
    use base64::{Engine, engine::general_purpose::STANDARD};
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header(
                    header::AUTHORIZATION,
                    format!("Basic {}", STANDARD.encode(format!("member:{PASSWORD}"))),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = form(
        &app,
        &cookie,
        "/account/tokens",
        &[
            ("csrf_token", &csrf),
            ("current_password", PASSWORD),
            ("description", "Synthetic MFA token"),
            ("scope", "reports:read"),
            ("days", "1"),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = form(
        &app,
        &cookie,
        "/account/tokens",
        &[
            ("csrf_token", &csrf),
            ("current_password", PASSWORD),
            ("mfa_code", &codes[1]),
            ("description", "Synthetic MFA token"),
            ("scope", "reports:read"),
            ("days", "1"),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = html(response).await;
    assert!(page.contains("al_pat_"));
    assert!(!page.contains(&codes[1]));
    let response = mfa_action(&app, &cookie, &csrf, "disable", &codes[2]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let new_cookie = changed_cookie(&response);
    assert!(!mfa::status(&state.auth, &user.id).await.unwrap().enrolled);
    assert_eq!(
        get(&app, &recovered_cookie, "/account").await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(get(&app, &new_cookie, "/account").await.0, StatusCode::OK);
    let admin_cookie = login_cookie(&app, "admin", PASSWORD).await;
    let (_, audit) = get(&app, &admin_cookie, "/admin/audit").await;
    for sensitive in [&secret, &codes[0], &codes[1], &old_token, PASSWORD] {
        assert!(
            !audit.contains(sensitive),
            "MFA secret escaped into audit HTML"
        );
    }
}

#[tokio::test]
async fn administrator_requirement_gates_existing_credentials_and_reset_preserves_enforcement() {
    let (_dir, state, app) = fixture().await;
    let admin_cookie = login_cookie(&app, "admin", PASSWORD).await;
    let (_, page) = get(&app, &admin_cookie, "/admin/accounts").await;
    let admin_csrf = csrf(&page);
    let member_cookie = login_cookie(&app, "member", PASSWORD).await;
    let member = state
        .auth
        .authenticate("member", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    let token = crate::tokens::create(
        &state.auth,
        &member,
        "Synthetic required gate",
        "reports:read",
        1,
    )
    .await
    .unwrap();
    let cmd = account_command(&state, "member", Operation::RequireMfa, "").await;
    assert_eq!(
        post(&app, &member_cookie, &admin_csrf, PASSWORD, &cmd)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&app, &admin_cookie, &admin_csrf, PASSWORD, &cmd)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        get(&app, &member_cookie, "/account").await.0,
        StatusCode::UNAUTHORIZED
    );
    assert!(matches!(
        crate::tokens::authenticate(
            &state.auth,
            &token,
            crate::authorization::Capability::ReportsRead
        )
        .await
        .unwrap(),
        crate::tokens::Authentication::Invalid
    ));
    let cookie = login_cookie(&app, "member", PASSWORD).await;
    for path in [
        "/account",
        "/sessions",
        "/usage",
        "/sessions/unknown/messages/1",
        "/admin/accounts",
        "/account/tokens",
    ] {
        assert_eq!(
            get(&app, &cookie, path).await.0,
            StatusCode::SEE_OTHER,
            "mandatory enrollment bypass at {path}"
        );
    }
    let current = state
        .auth
        .authenticate("member", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert!(
        crate::tokens::create(
            &state.auth,
            &current,
            "Denied while unenrolled",
            "reports:read",
            1
        )
        .await
        .is_err()
    );
    let (cookie, codes, _) = enroll(&app, &cookie).await;
    let (_, page) = get(&app, &cookie, "/account/mfa").await;
    let member_csrf = csrf(&page);
    assert!(!page.contains("value=\"disable\""));
    assert_eq!(
        mfa_action(&app, &cookie, &member_csrf, "disable", &codes[0])
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        mfa::status(&state.auth, &member.id)
            .await
            .unwrap()
            .recovery_count,
        10
    );
    let cmd = account_command(&state, "member", Operation::ReleaseMfa, "").await;
    assert_eq!(
        post(&app, &admin_cookie, &admin_csrf, PASSWORD, &cmd)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let status = mfa::status(&state.auth, &member.id).await.unwrap();
    assert!(!status.required);
    assert!(status.enrolled);
    assert!(
        login_factor(&app, "member", "").await.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .starts_with("/login?error=1")
    );
    for op in [Operation::RequireMfa, Operation::ResetMfa] {
        let cmd = account_command(&state, "member", op, "").await;
        assert_eq!(
            post(&app, &admin_cookie, &admin_csrf, PASSWORD, &cmd)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    let status = mfa::status(&state.auth, &member.id).await.unwrap();
    assert!(status.required);
    assert!(!status.enrolled);
    assert_eq!(status.recovery_count, 0);
    let cookie = login_cookie(&app, "member", PASSWORD).await;
    assert_eq!(
        get(&app, &cookie, "/sessions").await.0,
        StatusCode::SEE_OTHER
    );
    let cmd = account_command(&state, "member", Operation::ReleaseMfa, "").await;
    assert_eq!(
        post(&app, &admin_cookie, &admin_csrf, PASSWORD, &cmd)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let cookie = login_cookie(&app, "member", PASSWORD).await;
    assert_eq!(get(&app, &cookie, "/sessions").await.0, StatusCode::OK);
}

#[tokio::test]
async fn enrolled_administrators_need_a_fresh_factor_for_management_and_unproven_sessions_fail() {
    let (_dir, state, app) = fixture().await;
    let cookie = login_cookie(&app, "admin", PASSWORD).await;
    let (cookie, codes, _) = enroll(&app, &cookie).await;
    let (_, page) = get(&app, &cookie, "/admin/accounts").await;
    let csrf = csrf(&page);
    let cmd = account_command(&state, "member", Operation::RequireMfa, "").await;
    assert_eq!(
        post(&app, &cookie, &csrf, PASSWORD, &cmd).await.status(),
        StatusCode::FORBIDDEN
    );
    let response = form(
        &app,
        &cookie,
        "/admin/actions",
        &[
            ("csrf_token", &csrf),
            ("current_password", PASSWORD),
            ("mfa_code", &codes[0]),
            ("operation", "require_mfa"),
            ("target", &cmd.target),
            ("confirmation", &cmd.confirmation),
            ("expected", &cmd.expected),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let (user, generation) = state
        .auth
        .authenticate("admin", PASSWORD)
        .await
        .unwrap()
        .unwrap();
    let session =
        tower_sessions::Session::new(None, std::sync::Arc::new(state.sessions.clone()), None);
    auth::establish_session(&state.auth, &session, &user, generation, None)
        .await
        .unwrap();
    let unproven = format!("agent_lens_session={}", session.id().unwrap());
    assert_eq!(
        get(&app, &unproven, "/admin/accounts").await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn mfa_password_change_needs_both_factors_and_keeps_the_rotated_session_verified() {
    let (_dir, state, app) = fixture().await;
    let cookie = login_cookie(&app, "member", PASSWORD).await;
    let (cookie, codes, _) = enroll(&app, &cookie).await;
    let (_, page) = get(&app, &cookie, "/account").await;
    let csrf = csrf(&page);
    let response = form(
        &app,
        &cookie,
        "/account/password",
        &[
            ("csrf_token", &csrf),
            ("current_password", PASSWORD),
            ("new_password", "Replacement1-password"),
            ("confirm_password", "Replacement1-password"),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let response = form(
        &app,
        &cookie,
        "/account/password",
        &[
            ("csrf_token", &csrf),
            ("current_password", "wrong"),
            ("mfa_code", &codes[0]),
            ("new_password", "Replacement1-password"),
            ("confirm_password", "Replacement1-password"),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let user = state
        .auth
        .authenticate("member", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert_eq!(
        mfa::status(&state.auth, &user.id)
            .await
            .unwrap()
            .recovery_count,
        10,
        "incorrect password must not consume a recovery code"
    );
    let response = form(
        &app,
        &cookie,
        "/account/password",
        &[
            ("csrf_token", &csrf),
            ("current_password", PASSWORD),
            ("mfa_code", &codes[0]),
            ("new_password", "Replacement1-password"),
            ("confirm_password", "Replacement1-password"),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let rotated = changed_cookie(&response);
    assert_eq!(get(&app, &rotated, "/account").await.0, StatusCode::OK);
    assert_eq!(
        get(&app, &cookie, "/account").await.0,
        StatusCode::UNAUTHORIZED
    );
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
            .authenticate("member", "Replacement1-password")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        mfa::status(&state.auth, &user.id).await.unwrap().enrolled,
        "password change must preserve the authenticator"
    );
}
