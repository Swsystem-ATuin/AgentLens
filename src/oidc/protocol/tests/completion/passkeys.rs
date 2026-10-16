use super::*;
use crate::{oidc::browser::PasskeyAssertion, passkeys};
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};

const ORIGIN: &str = "https://lens.example.test";
type Device = WebauthnAuthenticator<SoftPasskey>;

async fn fixture() -> (tempfile::TempDir, auth::StateStore, Device) {
    let (dir, mut state, _) = browser_fixture().await;
    state.auth.passkeys = passkeys::Runtime::configured(Some(ORIGIN)).unwrap();
    let mut device = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let (user, generation) = state
        .auth
        .authenticate("member", "Synthetic1-password")
        .await
        .unwrap()
        .unwrap();
    let session = browser_session(&state);
    auth::establish_session(&state.auth, &session, &user, generation, None)
        .await
        .unwrap();
    let inventory = auth::current_inventory_id(&session).await.unwrap().unwrap();
    let challenge = passkeys::start_registration(
        &state.auth,
        &user,
        &inventory,
        "Synthetic OIDC second factor",
    )
    .await
    .unwrap();
    let credential = device
        .do_registration(Url::parse(ORIGIN).unwrap(), challenge.options)
        .unwrap();
    passkeys::finish_registration(
        &state.auth,
        &user,
        &inventory,
        &challenge.ticket,
        credential,
    )
    .await
    .unwrap();
    (dir, state, device)
}

#[tokio::test]
async fn oidc_passkey_requires_password_and_real_signature_before_rotated_local_session() {
    let (_dir, state, mut device) = fixture().await;
    let (session, candidate, csrf) = candidate(&state).await;
    assert!(
        candidate
            .start_passkey(
                &state.auth,
                &session,
                &browser_headers(),
                &csrf,
                "wrong-private-password"
            )
            .await
            .is_err()
    );
    assert!(state.auth.current_user(&session).await.unwrap().is_none());
    let challenge = candidate
        .start_passkey(
            &state.auth,
            &session,
            &browser_headers(),
            &csrf,
            "Synthetic1-password",
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&challenge.options).unwrap()["publicKey"]["userVerification"],
        "required"
    );
    let credential = device
        .do_authentication(Url::parse(ORIGIN).unwrap(), challenge.options)
        .unwrap();
    let before = session.id().unwrap();
    let completion = candidate
        .complete_passkey(
            &state.auth,
            &session,
            &browser_headers(),
            &csrf,
            PasskeyAssertion {
                ticket: challenge.ticket.clone(),
                credential: credential.clone(),
            },
        )
        .await
        .unwrap()
        .unwrap();
    assert_ne!(session.id().unwrap(), before);
    assert_eq!(completion.user.username, "member");
    assert_eq!(completion.user.access.role(), "viewer");
    assert!(!completion.enrollment_required);
    assert_eq!(
        session.get::<bool>("auth.mfa_verified").await.unwrap(),
        Some(true)
    );
    assert!(state.auth.current_user(&session).await.unwrap().is_some());
    let failed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM security_events WHERE event_type='login_failed'")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
    assert_eq!(failed, 1);
    assert!(
        passkeys::finish_oidc_authentication(
            &state.auth,
            &challenge.ticket,
            "o:wrong-candidate",
            credential
        )
        .await
        .is_err(),
        "used assertion tickets cannot mint another proof"
    );
    auth::revoke_sessions(&state, "member").await.unwrap();
    assert!(state.auth.current_user(&session).await.unwrap().is_none());
}

#[tokio::test]
async fn oidc_passkey_guards_browser_origin_csrf_runtime_and_candidate_age_before_password() {
    let (_dir, mut state, _) = fixture().await;
    let (session, mut candidate, csrf) = candidate(&state).await;
    let other = browser_session(&state);
    let other_csrf = auth::csrf_token(&other).await.unwrap();
    other.save().await.unwrap();
    assert!(
        candidate
            .start_passkey(
                &state.auth,
                &other,
                &browser_headers(),
                &other_csrf,
                "Synthetic1-password"
            )
            .await
            .is_err()
    );
    assert!(
        candidate
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
    let mut headers = browser_headers();
    headers.insert("origin", "https://other.example.test".parse().unwrap());
    assert!(
        candidate
            .start_passkey(
                &state.auth,
                &session,
                &headers,
                &csrf,
                "Synthetic1-password"
            )
            .await
            .is_err()
    );
    for origin in [None, Some("https://other.example.test")] {
        state.auth.passkeys = passkeys::Runtime::configured(origin).unwrap();
        assert!(
            candidate
                .start_passkey(
                    &state.auth,
                    &session,
                    &browser_headers(),
                    &csrf,
                    "Synthetic1-password"
                )
                .await
                .is_err()
        );
    }
    state.auth.passkeys = passkeys::Runtime::configured(Some(ORIGIN)).unwrap();
    candidate.expires = std::time::Instant::now();
    assert!(
        candidate
            .start_passkey(
                &state.auth,
                &session,
                &browser_headers(),
                &csrf,
                "Synthetic1-password"
            )
            .await
            .is_err()
    );
    candidate.expires = std::time::Instant::now() + std::time::Duration::from_secs(300);
    for _ in 0..60 {
        assert!(state.auth.accept_password_attempt());
    }
    let error = candidate
        .start_passkey(
            &state.auth,
            &session,
            &browser_headers(),
            &csrf,
            "Synthetic1-password",
        )
        .await
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("rate limited"),
        "OIDC must share the existing password-attempt budget before hashing/challenge creation"
    );
    assert!(state.auth.current_user(&session).await.unwrap().is_none());
}

#[tokio::test]
async fn oidc_passkey_assertions_cannot_transfer_between_candidates_in_the_same_browser() {
    let (_dir, state, mut device) = fixture().await;
    let (session, first, csrf) = candidate(&state).await;
    let challenge = first
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
        .do_authentication(Url::parse(ORIGIN).unwrap(), challenge.options)
        .unwrap();
    let flow = BrowserFlow::new(&config(true), protocol(true)).unwrap();
    let args = params(
        &flow
            .begin(&state.auth, &session, &browser_headers(), &csrf)
            .await
            .unwrap(),
    );
    let provider = Provider::new(&token(&claims(&args["nonce"])));
    let second = flow
        .finish(&state.auth, &session, &args["state"], "code", &provider)
        .await
        .unwrap();
    assert!(
        second
            .complete_passkey(
                &state.auth,
                &session,
                &browser_headers(),
                &csrf,
                PasskeyAssertion {
                    ticket: challenge.ticket.clone(),
                    credential: credential.clone()
                }
            )
            .await
            .is_err()
    );
    assert!(state.auth.current_user(&session).await.unwrap().is_none());
    assert!(
        first
            .complete_passkey(
                &state.auth,
                &session,
                &browser_headers(),
                &csrf,
                PasskeyAssertion {
                    ticket: challenge.ticket,
                    credential
                }
            )
            .await
            .unwrap()
            .is_some(),
        "a mismatched candidate must not consume the original bound challenge"
    );
}

#[tokio::test]
async fn oidc_passkey_rechecks_link_account_generation_and_assertion_signature() {
    for mutation in ["unlink", "disable", "revoke", "signature", "expired"] {
        let (_dir, state, mut device) = fixture().await;
        let (session, mut candidate, csrf) = candidate(&state).await;
        let challenge = candidate
            .start_passkey(
                &state.auth,
                &session,
                &browser_headers(),
                &csrf,
                "Synthetic1-password",
            )
            .await
            .unwrap();
        let mut credential = device
            .do_authentication(Url::parse(ORIGIN).unwrap(), challenge.options)
            .unwrap();
        match mutation {
            "unlink" => links::unlink(&state.auth, "member", &candidate.link_id)
                .await
                .unwrap(),
            "disable" => auth::set_enabled(&state, "member", false).await.unwrap(),
            "revoke" => auth::revoke_sessions(&state, "member").await.unwrap(),
            "expired" => candidate.expires = std::time::Instant::now(),
            _ => {
                let mut json = serde_json::to_value(credential).unwrap();
                json["response"]["signature"] =
                    json!(URL_SAFE_NO_PAD.encode(b"invalid synthetic signature"));
                credential = serde_json::from_value(json).unwrap();
            }
        }
        assert!(
            candidate
                .complete_passkey(
                    &state.auth,
                    &session,
                    &browser_headers(),
                    &csrf,
                    PasskeyAssertion {
                        ticket: challenge.ticket,
                        credential
                    }
                )
                .await
                .is_err(),
            "{mutation} must reject passkey completion"
        );
        assert!(state.auth.current_user(&session).await.unwrap().is_none());
    }
}
