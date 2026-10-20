use super::super::backchannel::{logout_claims, logout_token};
use super::*;

async fn provider_candidate(
    state: &auth::StateStore,
    sid: Option<&str>,
    client: &str,
    issued: i64,
) -> (Session, Candidate, String) {
    let mut config = config(true);
    config.client_id = client.into();
    let engine = Protocol::from_documents(&config, &discovery(), &jwks(key()), None).unwrap();
    let flow = BrowserFlow::new(&config, engine).unwrap();
    let session = browser_session(state);
    let csrf = auth::csrf_token(&session).await.unwrap();
    let args = params(
        &flow
            .begin(&state.auth, &session, &browser_headers(), &csrf)
            .await
            .unwrap(),
    );
    let mut payload = claims(&args["nonce"]);
    payload["iat"] = json!(issued);
    payload["aud"] = json!(client);
    payload["azp"] = json!(client);
    if let Some(sid) = sid {
        payload["sid"] = json!(sid);
    }
    let provider = Provider::new(&token(&payload));
    let candidate = flow
        .finish(&state.auth, &session, &args["state"], "code", &provider)
        .await
        .unwrap();
    (session, candidate, csrf)
}
async fn login(state: &auth::StateStore, sid: Option<&str>, client: &str, issued: i64) -> Session {
    let (session, candidate, csrf) = provider_candidate(state, sid, client, issued).await;
    candidate
        .complete(&state.auth, &session, &browser_headers(), &csrf, "")
        .await
        .unwrap()
        .unwrap();
    session
}
fn event(jti: &str, subject: Option<&str>, sid: Option<&str>) -> String {
    let mut claims = logout_claims();
    claims["jti"] = json!(jti);
    claims.as_object_mut().unwrap().remove("sub");
    claims.as_object_mut().unwrap().remove("sid");
    if let Some(subject) = subject {
        claims["sub"] = json!(subject);
    }
    if let Some(sid) = sid {
        claims["sid"] = json!(sid);
    }
    logout_token(&claims)
}
async fn active(state: &auth::StateStore, session: &Session) -> bool {
    state.auth.current_user(session).await.unwrap().is_some()
}

#[tokio::test]
async fn durable_logout_targets_both_claims_and_preserves_password_and_other_client_sessions() {
    let (_dir, state, _) = browser_fixture().await;
    let issued = auth::unix_now() - 10;
    let a = login(&state, Some("provider-a"), CLIENT_ID, issued).await;
    let b = login(&state, Some("provider-b"), CLIENT_ID, issued).await;
    let no_sid = login(&state, None, CLIENT_ID, issued).await;
    let other_client = login(&state, Some("provider-a"), "other-client", issued).await;
    let (user, generation) = state
        .auth
        .authenticate("member", "Synthetic1-password")
        .await
        .unwrap()
        .unwrap();
    let local = browser_session(&state);
    auth::establish_session(&state.auth, &local, &user, generation, None)
        .await
        .unwrap();
    let bearer = crate::tokens::create(
        &state.auth,
        &user,
        "Synthetic independent token",
        "reports:read",
        1,
    )
    .await
    .unwrap();
    let engine = protocol(true);
    assert_eq!(
        engine
            .apply_logout(
                &state.auth,
                &event("one", Some(SUBJECT), Some("provider-a"))
            )
            .await
            .unwrap(),
        1
    );
    assert!(!active(&state, &a).await);
    assert_eq!(
        engine
            .apply_logout(&state.auth, &event("miss", Some(SUBJECT), Some("missing")))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        engine
            .apply_logout(
                &state.auth,
                &event("both", Some("different-subject"), Some("provider-b"))
            )
            .await
            .unwrap(),
        0
    );
    assert!(active(&state, &b).await && active(&state, &no_sid).await);
    assert_eq!(
        engine
            .apply_logout(&state.auth, &event("subject", Some(SUBJECT), None))
            .await
            .unwrap(),
        2
    );
    assert!(!active(&state, &b).await && !active(&state, &no_sid).await);
    assert!(active(&state, &local).await && active(&state, &other_client).await);
    assert!(
        matches!(
            crate::tokens::authenticate(
                &state.auth,
                &bearer,
                crate::authorization::Capability::ReportsRead
            )
            .await
            .unwrap(),
            crate::tokens::Authentication::Authorized(_)
        ),
        "provider logout must not revoke independently issued PATs"
    );
    assert_eq!(
        state
            .auth
            .current_user(&local)
            .await
            .unwrap()
            .unwrap()
            .credential_generation,
        user.credential_generation
    );
    let stored: Vec<Vec<u8>> = sqlx::query_scalar("SELECT subject_key FROM oidc_session_bindings")
        .fetch_all(&state.auth.pool)
        .await
        .unwrap();
    assert!(stored.iter().all(|value| {
        value.len() == 32
            && !value
                .windows(SUBJECT.len())
                .any(|part| part == SUBJECT.as_bytes())
    }));
}

#[tokio::test]
async fn durable_logout_replay_survives_restart_and_serializes_concurrent_retries() {
    let (dir, state, _) = browser_fixture().await;
    let session = login(
        &state,
        Some("same-session"),
        CLIENT_ID,
        auth::unix_now() - 10,
    )
    .await;
    let jwt = event("repeat", None, Some("same-session"));
    let engine = protocol(true);
    let (one, two) = tokio::join!(
        engine.apply_logout(&state.auth, &jwt),
        engine.apply_logout(&state.auth, &jwt)
    );
    assert_eq!(one.unwrap() + two.unwrap(), 1);
    assert!(!active(&state, &session).await);
    let newer = login(
        &state,
        Some("same-session"),
        CLIENT_ID,
        auth::unix_now() + 1,
    )
    .await;
    let reopened = auth::StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    assert_eq!(
        protocol(true)
            .apply_logout(&reopened.auth, &jwt)
            .await
            .unwrap(),
        0
    );
    assert!(active(&reopened, &newer).await);
}

#[tokio::test]
async fn durable_logout_failure_rolls_back_replay_payload_revocation_and_audit_then_allows_retry() {
    let (_dir, state, _) = browser_fixture().await;
    let session = login(&state, Some("rollback"), CLIENT_ID, auth::unix_now() - 10).await;
    sqlx::raw_sql("CREATE TRIGGER fail_provider_logout BEFORE INSERT ON security_events WHEN NEW.event_type='session_revoked' BEGIN SELECT RAISE(ABORT,'synthetic audit failure'); END;").execute(&state.auth.pool).await.unwrap();
    let jwt = event("rollback", Some(SUBJECT), Some("rollback"));
    let engine = protocol(true);
    assert!(engine.apply_logout(&state.auth, &jwt).await.is_err());
    assert!(active(&state, &session).await);
    let ledger: i64 = sqlx::query_scalar("SELECT count(*) FROM oidc_logout_events")
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
    assert_eq!(ledger, 0);
    sqlx::raw_sql("DROP TRIGGER fail_provider_logout;")
        .execute(&state.auth.pool)
        .await
        .unwrap();
    assert_eq!(engine.apply_logout(&state.auth, &jwt).await.unwrap(), 1);
    assert!(!active(&state, &session).await);
}

#[tokio::test]
async fn durable_logout_tombstones_block_pending_login_and_pending_inventory_cannot_be_rebound() {
    let (_dir, state, _) = browser_fixture().await;
    let (session, candidate, csrf) =
        provider_candidate(&state, Some("raced"), CLIENT_ID, auth::unix_now() - 10).await;
    assert_eq!(
        protocol(true)
            .apply_logout(
                &state.auth,
                &event("before-login", Some(SUBJECT), Some("raced"))
            )
            .await
            .unwrap(),
        0
    );
    assert!(
        candidate
            .complete(&state.auth, &session, &browser_headers(), &csrf, "")
            .await
            .is_err()
    );
    assert!(!active(&state, &session).await);
    let (session, candidate, csrf) =
        provider_candidate(&state, Some("newer"), CLIENT_ID, auth::unix_now() + 1).await;
    sqlx::raw_sql("CREATE TRIGGER logout_during_provider_rotation AFTER INSERT ON tower_sessions WHEN EXISTS(SELECT 1 FROM account_sessions WHERE tower_session_id LIKE 'oidc_pending:%') BEGIN UPDATE account_sessions SET revoked_at=unixepoch() WHERE id IN (SELECT inventory_id FROM oidc_session_bindings); END;").execute(&state.auth.pool).await.unwrap();
    assert!(
        candidate
            .complete(&state.auth, &session, &browser_headers(), &csrf, "")
            .await
            .is_err()
    );
    assert!(!active(&state, &session).await);
}

#[tokio::test]
async fn durable_logout_v10_migration_preserves_local_sessions_links_and_foreign_keys() {
    let (dir, state, _) = browser_fixture().await;
    let (user, generation) = state
        .auth
        .authenticate("member", "Synthetic1-password")
        .await
        .unwrap()
        .unwrap();
    let local = browser_session(&state);
    auth::establish_session(&state.auth, &local, &user, generation, None)
        .await
        .unwrap();
    sqlx::raw_sql("DROP TABLE oidc_session_bindings; DROP TABLE oidc_logout_events; UPDATE app_state_metadata SET value='10';").execute(&state.auth.pool).await.unwrap();
    let reopened = auth::StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    assert!(active(&reopened, &local).await);
    let version: String =
        sqlx::query_scalar("SELECT value FROM app_state_metadata WHERE key='schema_version'")
            .fetch_one(&reopened.auth.pool)
            .await
            .unwrap();
    assert_eq!(version, "11");
    let links: i64 = sqlx::query_scalar("SELECT count(*) FROM account_oidc_links")
        .fetch_one(&reopened.auth.pool)
        .await
        .unwrap();
    assert_eq!(links, 1);
    assert!(
        sqlx::query(
            "INSERT INTO oidc_session_bindings VALUES('missing',zeroblob(32),zeroblob(32),NULL,0)"
        )
        .execute(&reopened.auth.pool)
        .await
        .is_err()
    );
}

#[tokio::test]
async fn durable_logout_capacity_is_bounded_and_only_expired_tombstones_are_pruned() {
    let (_dir, state, _) = browser_fixture().await;
    let session = login(&state, Some("capacity"), CLIENT_ID, auth::unix_now() - 10).await;
    sqlx::raw_sql("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<4096) INSERT INTO oidc_logout_events SELECT randomblob(32),randomblob(32),randomblob(32),NULL,unixepoch(),unixepoch()+1200 FROM n;").execute(&state.auth.pool).await.unwrap();
    let engine = protocol(true);
    let jwt = event("capacity", None, Some("capacity"));
    assert!(engine.apply_logout(&state.auth, &jwt).await.is_err());
    assert!(active(&state, &session).await);
    sqlx::query("UPDATE oidc_logout_events SET retain_until=unixepoch()-1")
        .execute(&state.auth.pool)
        .await
        .unwrap();
    assert_eq!(engine.apply_logout(&state.auth, &jwt).await.unwrap(), 1);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM oidc_logout_events")
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn durable_logout_provider_session_claims_are_optional_bounded_and_typed() {
    for value in [
        json!(null),
        json!(1),
        json!(""),
        json!("nonascii-é"),
        json!("control\n"),
        json!("a".repeat(1025)),
    ] {
        let engine = protocol(true);
        let args = params(&engine.begin("sid-shape").unwrap());
        let mut payload = claims(&args["nonce"]);
        payload["sid"] = value;
        let provider = Provider::new(&token(&payload));
        assert!(
            engine
                .finish("sid-shape", &args["state"], "code", &provider)
                .await
                .is_err()
        );
    }
    for sid in ["exact provider sid".to_owned(), "a".repeat(1024)] {
        let engine = protocol(true);
        let args = params(&engine.begin("sid-shape").unwrap());
        let mut payload = claims(&args["nonce"]);
        payload["sid"] = json!(sid);
        let provider = Provider::new(&token(&payload));
        assert_eq!(
            engine
                .finish("sid-shape", &args["state"], "code", &provider)
                .await
                .unwrap()
                .session_id,
            Some(sid)
        );
    }
}
