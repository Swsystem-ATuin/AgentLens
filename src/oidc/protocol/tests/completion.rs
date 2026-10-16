use super::*;
use crate::{
    auth, mfa,
    oidc::{
        browser::{BrowserFlow, Candidate},
        links,
    },
};
use tower_sessions::Session;

async fn candidate(state: &auth::StateStore) -> (Session, Candidate, String) {
    let flow = BrowserFlow::new(&config(true), protocol(true)).unwrap();
    let session = browser_session(state);
    let csrf = auth::csrf_token(&session).await.unwrap();
    let args = params(
        &flow
            .begin(&state.auth, &session, &browser_headers(), &csrf)
            .await
            .unwrap(),
    );
    let provider = Provider::new(&token(&claims(&args["nonce"])));
    let candidate = flow
        .finish(&state.auth, &session, &args["state"], "code", &provider)
        .await
        .unwrap();
    (session, candidate, csrf)
}

async fn enroll(state: &auth::StateStore) -> (totp_rs::TOTP, Vec<String>) {
    let (user, generation) = state
        .auth
        .authenticate("member", "Synthetic1-password")
        .await
        .unwrap()
        .unwrap();
    let session = browser_session(state);
    auth::establish_session(&state.auth, &session, &user, generation, None)
        .await
        .unwrap();
    let inventory = auth::current_inventory_id(&session).await.unwrap().unwrap();
    let setup = mfa::begin(&state.auth, &user, &inventory).await.unwrap();
    let generator = totp_rs::TOTP::from_url(setup.uri).unwrap();
    // Enrollment consumes the previous step, leaving the current step for login verification.
    let code = generator.generate((auth::unix_now() - 30) as u64);
    let (_, codes) = mfa::finish(&state.auth, &user, &inventory, &code)
        .await
        .unwrap()
        .unwrap();
    (generator, codes)
}

#[tokio::test]
async fn oidc_completion_rotates_session_and_preserves_local_policy_and_revocation() {
    let (_dir, state, _) = browser_fixture().await;
    let (session, candidate, csrf) = candidate(&state).await;
    let before = session.id().unwrap();
    auth::set_role(&state, "member", auth::AccountRole::Auditor)
        .await
        .unwrap();
    let completion = candidate
        .complete(&state.auth, &session, &browser_headers(), &csrf, "")
        .await
        .unwrap()
        .unwrap();
    assert_ne!(session.id().unwrap(), before);
    assert_eq!(
        completion.user.access.role(),
        "auditor",
        "completion must refresh permissions changed since callback"
    );
    assert!(!completion.enrollment_required);
    assert_eq!(completion.identity.subject, SUBJECT);
    assert_eq!(
        session.get::<bool>("auth.mfa_verified").await.unwrap(),
        Some(false),
        "no enrolled factor means no fabricated MFA proof"
    );
    assert_eq!(
        state.auth.current_user(&session).await.unwrap().unwrap().id,
        completion.user.id
    );
    let stale = Session::new(
        Some(before),
        std::sync::Arc::new(state.sessions.clone()),
        None,
    );
    assert!(state.auth.current_user(&stale).await.unwrap().is_none());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM security_events WHERE event_type='login_succeeded'",
    )
    .fetch_one(&state.auth.pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    auth::revoke_sessions(&state, "member").await.unwrap();
    assert!(state.auth.current_user(&session).await.unwrap().is_none());
}

#[tokio::test]
async fn oidc_completion_requires_local_totp_or_one_use_recovery_and_retains_attempt_budget() {
    let (_dir, state, _) = browser_fixture().await;
    let (generator, codes) = enroll(&state).await;
    for invalid in ["", "invalid", "pk_forged-proof"] {
        let (session, candidate, csrf) = candidate(&state).await;
        assert!(
            candidate
                .complete(&state.auth, &session, &browser_headers(), &csrf, invalid)
                .await
                .unwrap()
                .is_none()
        );
        assert!(state.auth.current_user(&session).await.unwrap().is_none());
    }
    let attempts: i64 = sqlx::query_scalar("SELECT attempt_count FROM account_mfa WHERE account_id=(SELECT id FROM accounts WHERE username='member')").fetch_one(&state.auth.pool).await.unwrap();
    assert_eq!(
        attempts, 2,
        "invalid TOTP attempts must persist, while forged passkey proofs never count as TOTP"
    );
    let code = generator.generate(auth::unix_now() as u64);
    for (factor, accepted) in [
        (&code, true),
        (&code, false),
        (&codes[0], true),
        (&codes[0], false),
    ] {
        let (session, candidate, csrf) = candidate(&state).await;
        let result = candidate
            .complete(&state.auth, &session, &browser_headers(), &csrf, factor)
            .await
            .unwrap();
        assert_eq!(
            result.is_some(),
            accepted,
            "local factor replay policy must remain effective"
        );
        assert_eq!(
            state.auth.current_user(&session).await.unwrap().is_some(),
            accepted
        );
        if accepted {
            assert_eq!(
                session.get::<bool>("auth.mfa_verified").await.unwrap(),
                Some(true)
            );
        }
    }
}

#[tokio::test]
async fn oidc_completion_preserves_required_enrollment_and_rejects_stale_or_foreign_candidates() {
    for mutation in [
        "enrollment",
        "unlink",
        "disable",
        "revoke",
        "expiry",
        "browser",
        "csrf",
        "origin",
    ] {
        let (_dir, state, link) = browser_fixture().await;
        if mutation == "enrollment" {
            sqlx::query("INSERT INTO account_mfa(account_id,required) SELECT id,1 FROM accounts WHERE username='member'").execute(&state.auth.pool).await.unwrap();
        }
        let (session, mut candidate, csrf) = candidate(&state).await;
        match mutation {
            "unlink" => links::unlink(&state.auth, "member", &link).await.unwrap(),
            "disable" => auth::set_enabled(&state, "member", false).await.unwrap(),
            "revoke" => auth::revoke_sessions(&state, "member").await.unwrap(),
            "expiry" => candidate.expires = std::time::Instant::now(),
            _ => {}
        }
        let other = browser_session(&state);
        let other_csrf = auth::csrf_token(&other).await.unwrap();
        other.save().await.unwrap();
        let mut headers = browser_headers();
        if mutation == "origin" {
            headers.insert("origin", "https://other.example.test".parse().unwrap());
        }
        let target = if mutation == "browser" {
            &other
        } else {
            &session
        };
        let csrf = if mutation == "browser" {
            &other_csrf
        } else if mutation == "csrf" {
            "wrong"
        } else {
            &csrf
        };
        let result = candidate
            .complete(&state.auth, target, &headers, csrf, "")
            .await;
        if mutation == "enrollment" {
            let completion = result.unwrap().unwrap();
            assert!(completion.enrollment_required);
            assert_eq!(
                target.get::<bool>("auth.mfa_verified").await.unwrap(),
                Some(false)
            );
            assert!(
                mfa::status(&state.auth, &completion.user.id)
                    .await
                    .unwrap()
                    .required
            );
            assert!(
                state
                    .auth
                    .token_account(&completion.user.id, completion.user.credential_generation)
                    .await
                    .unwrap()
                    .is_none(),
                "required enrollment must not enable bearer access"
            );
            use axum::{
                Router,
                body::Body,
                http::{Request, StatusCode, header},
                middleware,
                routing::get,
            };
            use tower::ServiceExt;
            let app = Router::new()
                .route("/sessions", get(|| async { "private sessions" }))
                .route("/account/mfa", get(|| async { "enrollment" }))
                .layer(middleware::from_fn_with_state(
                    state.auth.clone(),
                    auth::require_auth,
                ))
                .layer(
                    tower_sessions::SessionManagerLayer::new(state.sessions.clone())
                        .with_name("agent_lens_session")
                        .with_secure(false),
                );
            let cookie = format!("agent_lens_session={}", target.id().unwrap());
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/sessions")
                        .header(header::COOKIE, &cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(response.headers()[header::LOCATION], "/account/mfa");
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/account/mfa")
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        } else {
            assert!(
                result.is_err(),
                "{mutation} must reject stale/foreign completion"
            );
            assert!(state.auth.current_user(target).await.unwrap().is_none());
        }
    }
}

#[tokio::test]
async fn oidc_completion_audit_failure_cannot_leave_an_authenticated_or_adoptable_session() {
    let (_dir, state, _) = browser_fixture().await;
    let (session, candidate, csrf) = candidate(&state).await;
    sqlx::raw_sql("CREATE TRIGGER reject_oidc_login BEFORE INSERT ON security_events WHEN NEW.event_type='login_succeeded' BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").execute(&state.auth.pool).await.unwrap();
    assert!(
        candidate
            .complete(&state.auth, &session, &browser_headers(), &csrf, "")
            .await
            .is_err()
    );
    assert!(state.auth.current_user(&session).await.unwrap().is_none());
    let active: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_sessions WHERE revoked_at IS NULL")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
    assert_eq!(
        active, 0,
        "failed session binding must revoke inventory, not allow legacy adoption"
    );
}

#[tokio::test]
async fn oidc_completion_rechecks_revocation_during_session_store_rotation() {
    for mutation in [
        "UPDATE accounts SET enabled=0,session_generation=session_generation+1 WHERE username='member';",
        "DELETE FROM account_oidc_links;",
        "UPDATE account_sessions SET revoked_at=unixepoch();",
    ] {
        let (_dir, state, _) = browser_fixture().await;
        let (session, candidate, csrf) = candidate(&state).await;
        // Inject a concurrent state change precisely when the new session payload is saved,
        // after factor/inventory commit but before final binding. No sleeps or timing races.
        sqlx::raw_sql(&format!("CREATE TRIGGER revoke_during_oidc_rotation AFTER INSERT ON tower_sessions WHEN EXISTS(SELECT 1 FROM account_sessions WHERE tower_session_id LIKE 'oidc_pending:%') BEGIN {mutation} END;"))
            .execute(&state.auth.pool).await.unwrap();
        assert!(
            candidate
                .complete(&state.auth, &session, &browser_headers(), &csrf, "")
                .await
                .is_err()
        );
        assert!(state.auth.current_user(&session).await.unwrap().is_none());
        let active: i64 =
            sqlx::query_scalar("SELECT count(*) FROM account_sessions WHERE revoked_at IS NULL")
                .fetch_one(&state.auth.pool)
                .await
                .unwrap();
        assert_eq!(
            active, 0,
            "revocation during rotation must never produce an adoptable session"
        );
        let logins: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM security_events WHERE event_type='login_succeeded'",
        )
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
        assert_eq!(
            logins, 0,
            "failed rotation must not record a successful sign-in"
        );
    }
}
