use super::*;
use crate::oidc::browser::Completion;

async fn completed(state: &auth::StateStore) -> (Session, Completion) {
    let (session, candidate, csrf) = candidate(state).await;
    let completion = candidate
        .complete(&state.auth, &session, &browser_headers(), &csrf, "")
        .await
        .unwrap()
        .unwrap();
    (session, completion)
}

#[tokio::test]
async fn oidc_logout_revokes_local_session_first_and_prepares_only_pinned_provider_post_fields() {
    let (_dir, state, _) = browser_fixture().await;
    let (session, completion) = completed(&state).await;
    let (other, _) = completed(&state).await;
    let old_id = session.id().unwrap();
    let csrf = auth::csrf_token(&session).await.unwrap();
    let expected_hint = completion.identity.id_token_hint.clone();
    let flow = BrowserFlow::new(&config(true), protocol(true)).unwrap();
    let request = flow
        .logout(&state.auth, &session, &browser_headers(), &csrf, completion)
        .await
        .unwrap();
    assert_eq!(request.endpoint, "https://identity.example.test/logout");
    assert!(
        !request.endpoint.contains('?'),
        "provider hint must be sent by form POST, never in a redirect URL"
    );
    assert_eq!(request.id_token_hint, expected_hint);
    assert_eq!(request.client_id, CLIENT_ID);
    assert_eq!(
        request.post_logout_redirect_uri,
        "https://lens.example.test/login/oidc/logout/callback"
    );
    assert_eq!(request.state.len(), 43);
    let response = request.response().unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert_eq!(
        response.headers()["content-security-policy"],
        "default-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action https://identity.example.test"
    );
    let html = String::from_utf8(
        axum::body::to_bytes(response.into_body(), 512 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("method=\"post\" action=\"https://identity.example.test/logout\""));
    assert!(html.contains("name=\"id_token_hint\""));
    assert!(!html.contains(&format!("logout?id_token_hint={expected_hint}")));
    assert_ne!(
        session.id().unwrap(),
        old_id,
        "logout callback must use a fresh anonymous browser ID"
    );
    assert!(
        state.auth.current_user(&session).await.unwrap().is_none(),
        "local login must end even if provider logout is never visited"
    );
    let old = Session::new(
        Some(old_id),
        std::sync::Arc::new(state.sessions.clone()),
        None,
    );
    assert!(state.auth.current_user(&old).await.unwrap().is_none());
    assert!(
        state.auth.current_user(&other).await.unwrap().is_some(),
        "RP logout targets this local session, not unrelated sessions"
    );
    let foreign = browser_session(&state);
    assert!(flow.finish_logout(&foreign, &request.state).await.is_err());
    let resumed = Session::new(
        session.id(),
        std::sync::Arc::new(state.sessions.clone()),
        None,
    );
    flow.finish_logout(&resumed, &request.state).await.unwrap();
    assert!(
        flow.finish_logout(&session, &request.state).await.is_err(),
        "stale parallel session data cannot replay logout callback"
    );
    assert!(state.auth.current_user(&resumed).await.unwrap().is_none());
    let data: Vec<Vec<u8>> = sqlx::query_scalar("SELECT data FROM tower_sessions")
        .fetch_all(&state.auth.pool)
        .await
        .unwrap();
    for data in data {
        assert!(
            !data
                .windows(expected_hint.len())
                .any(|part| part == expected_hint.as_bytes()),
            "logout hints must not be stored in browser transaction payloads"
        );
    }
}

#[tokio::test]
async fn oidc_logout_requires_csrf_origin_and_cannot_use_another_sessions_identity_context() {
    for mutation in ["csrf", "origin", "context", "issuer"] {
        let (_dir, state, _) = browser_fixture().await;
        let (session, mut completion) = completed(&state).await;
        let csrf = auth::csrf_token(&session).await.unwrap();
        let mut headers = browser_headers();
        let csrf = if mutation == "csrf" { "wrong" } else { &csrf };
        if mutation == "origin" {
            headers.insert("origin", "https://other.example.test".parse().unwrap());
        }
        if mutation == "context" {
            completion = completed(&state).await.1;
        }
        if mutation == "issuer" {
            completion.identity.issuer.push('/');
        }
        let flow = BrowserFlow::new(&config(true), protocol(true)).unwrap();
        assert!(
            flow.logout(&state.auth, &session, &headers, csrf, completion)
                .await
                .is_err()
        );
        assert_eq!(
            state.auth.current_user(&session).await.unwrap().is_some(),
            matches!(mutation, "csrf" | "origin"),
            "invalid requests cannot trigger logout, but valid logout requests must revoke locally even when provider context is unusable"
        );
    }
}

#[tokio::test]
async fn oidc_logout_bad_state_consumes_callback_without_ever_reauthenticating_or_ending_new_login()
{
    let (_dir, state, _) = browser_fixture().await;
    let (session, completion) = completed(&state).await;
    let csrf = auth::csrf_token(&session).await.unwrap();
    let flow = BrowserFlow::new(&config(true), protocol(true)).unwrap();
    let request = flow
        .logout(&state.auth, &session, &browser_headers(), &csrf, completion)
        .await
        .unwrap();
    assert!(flow.finish_logout(&session, "wrong-state").await.is_err());
    assert!(flow.finish_logout(&session, &request.state).await.is_err());
    assert!(state.auth.current_user(&session).await.unwrap().is_none());
    let (session, completion) = completed(&state).await;
    let csrf = auth::csrf_token(&session).await.unwrap();
    let request = flow
        .logout(&state.auth, &session, &browser_headers(), &csrf, completion)
        .await
        .unwrap();
    let (user, generation) = state
        .auth
        .authenticate("member", "Synthetic1-password")
        .await
        .unwrap()
        .unwrap();
    auth::establish_session(&state.auth, &session, &user, generation, None)
        .await
        .unwrap();
    flow.finish_logout(&session, &request.state).await.unwrap();
    assert!(
        state.auth.current_user(&session).await.unwrap().is_some(),
        "a late callback must not revoke a later local login"
    );
}
