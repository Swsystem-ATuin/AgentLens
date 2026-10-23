use super::*;
use crate::{
    auth,
    oidc::browser::{
        BrowserFlow,
        runtime::{LogoutOutcome, Runtime},
    },
};
use tower_sessions::Session;

fn runtime() -> Runtime {
    Runtime::new(BrowserFlow::new(&config(true), protocol(true)).unwrap())
}

async fn pending(runtime: &Runtime, state: &auth::StateStore) -> (Session, String, String) {
    let session = browser_session(state);
    let csrf = auth::csrf_token(&session).await.unwrap();
    let args = params(
        &runtime
            .begin(&state.auth, &session, &browser_headers(), &csrf)
            .await
            .unwrap(),
    );
    let jwt = token(&claims(&args["nonce"]));
    runtime
        .callback(
            &state.auth,
            &session,
            &args["state"],
            "synthetic-code",
            &Provider::new(&jwt),
        )
        .await
        .unwrap();
    (session, csrf, jwt)
}

#[tokio::test]
async fn runtime_contexts_resume_one_use_without_persisting_identities_or_credentials() {
    let (_dir, state, _) = browser_fixture().await;
    let runtime = runtime();
    let (session, csrf, jwt) = pending(&runtime, &state).await;
    let data: Vec<Vec<u8>> = sqlx::query_scalar("SELECT data FROM tower_sessions")
        .fetch_all(&state.auth.pool)
        .await
        .unwrap();
    for data in data {
        for private in [jwt.as_str(), SUBJECT, ACCESS, "synthetic-code"] {
            assert!(
                !data
                    .windows(private.len())
                    .any(|part| part == private.as_bytes()),
                "runtime session payload must hold opaque handles, not provider credentials/identity"
            );
        }
    }
    let resumed = Session::new(
        session.id(),
        std::sync::Arc::new(state.sessions.clone()),
        None,
    );
    let other = browser_session(&state);
    assert!(
        runtime
            .complete(
                &state.auth,
                &other,
                &browser_headers(),
                &auth::csrf_token(&other).await.unwrap(),
                ""
            )
            .await
            .is_err()
    );
    assert!(
        runtime
            .complete(&state.auth, &resumed, &browser_headers(), "wrong-csrf", "")
            .await
            .is_err()
    );
    let (user, enrollment) = runtime
        .complete(&state.auth, &resumed, &browser_headers(), &csrf, "")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.username, "member");
    assert!(!enrollment);
    assert!(
        runtime
            .complete(&state.auth, &session, &browser_headers(), &csrf, "")
            .await
            .is_err()
    );
    assert!(state.auth.current_user(&resumed).await.unwrap().is_some());
    let payloads: Vec<Vec<u8>> = sqlx::query_scalar("SELECT data FROM tower_sessions")
        .fetch_all(&state.auth.pool)
        .await
        .unwrap();
    for payload in payloads {
        assert!(
            !payload
                .windows(jwt.len())
                .any(|part| part == jwt.as_bytes()),
            "completed logout contexts must stay in memory, not session payloads"
        );
    }
    let csrf = auth::csrf_token(&resumed).await.unwrap();
    match runtime
        .logout(&state.auth, &resumed, &browser_headers(), &csrf)
        .await
        .unwrap()
    {
        LogoutOutcome::Provider(request) => {
            assert_eq!(request.id_token_hint, jwt);
            runtime
                .finish_logout(&resumed, &request.state)
                .await
                .unwrap();
        }
        LogoutOutcome::LocalOnly => {
            panic!("live matching logout context must prepare provider handoff")
        }
    }
    assert!(state.auth.current_user(&resumed).await.unwrap().is_none());
}

#[tokio::test]
async fn runtime_restart_loses_pending_proofs_but_logout_still_revokes_locally() {
    let (_dir, state, _) = browser_fixture().await;
    let first = runtime();
    let (session, csrf, _) = pending(&first, &state).await;
    assert!(
        runtime()
            .complete(&state.auth, &session, &browser_headers(), &csrf, "")
            .await
            .is_err()
    );
    let first = runtime();
    let (session, csrf, _) = pending(&first, &state).await;
    first
        .complete(&state.auth, &session, &browser_headers(), &csrf, "")
        .await
        .unwrap()
        .unwrap();
    let csrf = auth::csrf_token(&session).await.unwrap();
    assert!(matches!(
        runtime()
            .logout(&state.auth, &session, &browser_headers(), &csrf)
            .await
            .unwrap(),
        LogoutOutcome::LocalOnly
    ));
    assert!(state.auth.current_user(&session).await.unwrap().is_none());
}

#[tokio::test]
async fn runtime_new_start_cancels_pending_context_and_factor_completion_is_single_consumer() {
    let (_dir, state, _) = browser_fixture().await;
    let runtime = runtime();
    let (session, csrf, _) = pending(&runtime, &state).await;
    runtime
        .begin(&state.auth, &session, &browser_headers(), &csrf)
        .await
        .unwrap();
    assert!(
        runtime
            .complete(&state.auth, &session, &browser_headers(), &csrf, "")
            .await
            .is_err()
    );
    let (session, csrf, _) = pending(&runtime, &state).await;
    let headers = browser_headers();
    let (one, two) = tokio::join!(
        runtime.complete(&state.auth, &session, &headers, &csrf, ""),
        runtime.complete(&state.auth, &session, &headers, &csrf, "")
    );
    assert_ne!(
        one.is_ok(),
        two.is_ok(),
        "only one request may consume a pending candidate"
    );
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_sessions WHERE revoked_at IS NULL")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
    assert_eq!(active, 1);
}

#[tokio::test]
async fn runtime_passkey_completion_borrows_pending_context_then_consumes_it_once() {
    use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
    let (_dir, mut state, _) = browser_fixture().await;
    state.auth.passkeys =
        crate::passkeys::Runtime::configured(Some("https://lens.example.test")).unwrap();
    let mut device = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (user, generation) = state
        .auth
        .authenticate("member", "Synthetic1-password")
        .await
        .unwrap()
        .unwrap();
    let setup = browser_session(&state);
    auth::establish_session(&state.auth, &setup, &user, generation, None)
        .await
        .unwrap();
    let inventory = auth::current_inventory_id(&setup).await.unwrap().unwrap();
    let registration = crate::passkeys::start_registration(
        &state.auth,
        &user,
        &inventory,
        "Synthetic runtime device",
    )
    .await
    .unwrap();
    let credential = device
        .do_registration(
            Url::parse("https://lens.example.test").unwrap(),
            registration.options,
        )
        .unwrap();
    crate::passkeys::finish_registration(
        &state.auth,
        &user,
        &inventory,
        &registration.ticket,
        credential,
    )
    .await
    .unwrap();
    let runtime = runtime();
    let (session, csrf, _) = pending(&runtime, &state).await;
    assert!(
        runtime
            .start_passkey(
                &state.auth,
                &session,
                &browser_headers(),
                "wrong",
                "Synthetic1-password"
            )
            .await
            .is_err()
    );
    let challenge = runtime
        .start_passkey(
            &state.auth,
            &session,
            &browser_headers(),
            &csrf,
            "Synthetic1-password",
        )
        .await
        .unwrap();
    let credential = device
        .do_authentication(
            Url::parse("https://lens.example.test").unwrap(),
            challenge.options,
        )
        .unwrap();
    let result = runtime
        .complete_passkey(
            &state.auth,
            &session,
            &browser_headers(),
            &csrf,
            crate::oidc::browser::PasskeyAssertion {
                ticket: challenge.ticket,
                credential,
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.0.username, "member");
    assert!(!result.1);
    assert_eq!(
        session.get::<bool>("auth.mfa_verified").await.unwrap(),
        Some(true)
    );
}
