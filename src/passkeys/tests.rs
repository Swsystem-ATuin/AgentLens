use super::*;
use crate::auth::StateStore;
use tower_sessions::Session;
use webauthn_authenticator_rs::{WebauthnAuthenticator, softpasskey::SoftPasskey};
const ORIGIN: &str = "https://lens.example.test";
const PASSWORD: &str = "Synthetic1-password";
async fn fixture() -> (tempfile::TempDir, StateStore) {
    let dir = tempfile::tempdir().unwrap();
    let password = dir.path().join("password");
    std::fs::write(&password, PASSWORD).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let mut state = StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    state.auth.passkeys = Runtime::configured(Some(ORIGIN)).unwrap();
    auth::create_account(&state, "admin", &password, None)
        .await
        .unwrap();
    auth::create_account(&state, "member", &password, None)
        .await
        .unwrap();
    (dir, state)
}
async fn identity(state: &StateStore) -> (CurrentUser, Session, String) {
    let (user, generation) = state
        .auth
        .authenticate("member", PASSWORD)
        .await
        .unwrap()
        .unwrap();
    let session = Session::new(None, Arc::new(state.sessions.clone()), None);
    auth::establish_session(&state.auth, &session, &user, generation, None)
        .await
        .unwrap();
    let inventory = auth::current_inventory_id(&session).await.unwrap().unwrap();
    (user, session, inventory)
}
async fn enroll(
    state: &StateStore,
    device: &mut WebauthnAuthenticator<SoftPasskey>,
) -> Vec<String> {
    let (user, _session, inventory) = identity(state).await;
    let start = start_registration(&state.auth, &user, &inventory, "<b>Synthetic device</b>")
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&start.options).unwrap()["publicKey"]["authenticatorSelection"]["userVerification"],
        "required"
    );
    let credential = device
        .do_registration(Url::parse(ORIGIN).unwrap(), start.options)
        .unwrap();
    finish_registration(&state.auth, &user, &inventory, &start.ticket, credential)
        .await
        .unwrap()
        .1
}

#[test]
fn configuration_requires_explicit_https_origin_and_challenges_are_bounded_and_expiring() {
    assert!(!Runtime::configured(None).unwrap().enabled());
    for url in [
        "http://lens.example.test",
        "https://lens.example.test/path",
        "https://user@lens.example.test",
        "https://lens.example.test?q=1",
        "https://lens.example.test/#x",
        "https://127.0.0.1",
    ] {
        assert!(Runtime::configured(Some(url)).is_err(), "{url}");
    }
    assert_eq!(
        Runtime::configured(Some("https://lens.example.test:8443"))
            .unwrap()
            .origin(),
        Some("https://lens.example.test:8443")
    );
}

#[tokio::test]
async fn signed_passkeys_require_password_identity_and_produce_one_use_session_bound_proofs() {
    let (_dir, state) = fixture().await;
    let mut device = WebauthnAuthenticator::new(SoftPasskey::new(true));
    let codes = enroll(&state, &mut device).await;
    assert_eq!(codes.len(), 10);
    let (user, session, inventory) = identity(&state).await;
    let status = mfa::status(&state.auth, &user.id).await.unwrap();
    assert!(status.enrolled);
    assert!(!status.totp_enrolled);
    assert!(
        !mfa::verify(&state.auth, &user, "").await.unwrap(),
        "passkey-only enrollment must not silently allow password-only sign-in"
    );
    assert!(mfa::verify(&state.auth, &user, &codes[0]).await.unwrap());
    let start = start_authentication(&state.auth, &user, &format!("i:{inventory}"), false)
        .await
        .unwrap();
    let assertion = device
        .do_authentication(Url::parse(ORIGIN).unwrap(), start.options)
        .unwrap();
    assert!(
        finish_authentication(
            &state.auth,
            &start.ticket,
            "i:other",
            assertion.clone(),
            false
        )
        .await
        .is_err()
    );
    let verified = finish_authentication(
        &state.auth,
        &start.ticket,
        &format!("i:{inventory}"),
        assertion.clone(),
        false,
    )
    .await
    .unwrap();
    assert!(
        finish_authentication(
            &state.auth,
            &start.ticket,
            &format!("i:{inventory}"),
            assertion,
            false
        )
        .await
        .is_err()
    );
    let proof = factor_proof(&state.auth, &verified, &inventory).unwrap();
    let (_other, other_session, _) = identity(&state).await;
    assert!(
        !mfa::verify_in_session(&state.auth, &user, &proof, &other_session)
            .await
            .unwrap()
    );
    assert!(
        mfa::verify_in_session(&state.auth, &user, &proof, &session)
            .await
            .unwrap()
    );
    assert!(
        !mfa::verify_in_session(&state.auth, &user, &proof, &session)
            .await
            .unwrap()
    );
    assert_eq!(list(&state.auth, &user.id).await.unwrap().len(), 1);
    let key = list(&state.auth, &user.id).await.unwrap().remove(0);
    sqlx::query("UPDATE account_mfa SET required=1 WHERE account_id=?")
        .bind(&user.id)
        .execute(&state.auth.pool)
        .await
        .unwrap();
    assert!(
        delete(&state.auth, &user, &inventory, &key.id)
            .await
            .is_err(),
        "last required factor cannot be removed"
    );
    mfa::cli_reset(&state.auth, "member").await.unwrap();
    assert!(list(&state.auth, &user.id).await.unwrap().is_empty());
    assert!(mfa::status(&state.auth, &user.id).await.unwrap().required);
}

#[tokio::test]
async fn real_signatures_origin_expiry_counter_and_concurrent_revocation_are_checked() {
    let (_dir, state) = fixture().await;
    let mut device = WebauthnAuthenticator::new(SoftPasskey::new(true));
    enroll(&state, &mut device).await;
    let (user, _session, _inventory) = identity(&state).await;
    let start = start_authentication(&state.auth, &user, "s:origin", true)
        .await
        .unwrap();
    let assertion = device
        .do_authentication(
            Url::parse("https://lens.example.test:8443").unwrap(),
            start.options,
        )
        .unwrap();
    assert!(
        finish_authentication(&state.auth, &start.ticket, "s:origin", assertion, true)
            .await
            .is_err(),
        "a valid signature from the wrong exact origin must fail"
    );
    let start = start_authentication(&state.auth, &user, "s:signature", true)
        .await
        .unwrap();
    let mut assertion = device
        .do_authentication(Url::parse(ORIGIN).unwrap(), start.options)
        .unwrap();
    assertion.response.signature = vec![0u8; 64].into();
    assert!(
        finish_authentication(&state.auth, &start.ticket, "s:signature", assertion, true)
            .await
            .is_err()
    );
    let start = start_authentication(&state.auth, &user, "s:expired", true)
        .await
        .unwrap();
    let assertion = device
        .do_authentication(Url::parse(ORIGIN).unwrap(), start.options)
        .unwrap();
    state
        .auth
        .passkeys
        .pending
        .lock()
        .unwrap()
        .get_mut(&start.ticket)
        .unwrap()
        .expires = Instant::now() - Duration::from_secs(1);
    assert!(
        finish_authentication(&state.auth, &start.ticket, "s:expired", assertion, true)
            .await
            .is_err()
    );
    let first = start_authentication(&state.auth, &user, "s:first", true)
        .await
        .unwrap();
    let second = start_authentication(&state.auth, &user, "s:second", true)
        .await
        .unwrap();
    let older = device
        .do_authentication(Url::parse(ORIGIN).unwrap(), first.options)
        .unwrap();
    let newer = device
        .do_authentication(Url::parse(ORIGIN).unwrap(), second.options)
        .unwrap();
    finish_authentication(&state.auth, &second.ticket, "s:second", newer, true)
        .await
        .unwrap();
    assert!(
        finish_authentication(&state.auth, &first.ticket, "s:first", older, true)
            .await
            .is_err(),
        "counter regression must be checked against committed state, not just challenge snapshot"
    );
    let start = start_authentication(&state.auth, &user, "s:revoked", true)
        .await
        .unwrap();
    let assertion = device
        .do_authentication(Url::parse(ORIGIN).unwrap(), start.options)
        .unwrap();
    mfa::cli_reset(&state.auth, "member").await.unwrap();
    assert!(
        finish_authentication(&state.auth, &start.ticket, "s:revoked", assertion, true)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn device_limit_user_verification_restart_and_disabled_configuration_fail_closed() {
    let (dir, state) = fixture().await;
    let mut device = WebauthnAuthenticator::new(SoftPasskey::new(false));
    let (user, _session, inventory) = identity(&state).await;
    let start = start_registration(&state.auth, &user, &inventory, "No UV")
        .await
        .unwrap();
    if let Ok(response) = device.do_registration(Url::parse(ORIGIN).unwrap(), start.options) {
        assert!(
            finish_registration(&state.auth, &user, &inventory, &start.ticket, response)
                .await
                .is_err()
        );
    }
    let mut device = WebauthnAuthenticator::new(SoftPasskey::new(true));
    enroll(&state, &mut device).await;
    let (user, _, _) = identity(&state).await;
    let challenge = start_authentication(&state.auth, &user, "s:restart", true)
        .await
        .unwrap();
    let assertion = device
        .do_authentication(Url::parse(ORIGIN).unwrap(), challenge.options)
        .unwrap();
    state.auth.pool.close().await;
    let mut reopened = StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    assert!(
        mfa::status(&reopened.auth, &user.id)
            .await
            .unwrap()
            .enrolled,
        "disabling configuration must not remove factor enforcement"
    );
    assert!(
        start_authentication(&reopened.auth, &user, "s:disabled", true)
            .await
            .is_err()
    );
    reopened.auth.passkeys = Runtime::configured(Some(ORIGIN)).unwrap();
    assert!(
        finish_authentication(
            &reopened.auth,
            &challenge.ticket,
            "s:restart",
            assertion,
            true
        )
        .await
        .is_err()
    );
    let start = start_authentication(&reopened.auth, &user, "s:restored", true)
        .await
        .unwrap();
    let assertion = device
        .do_authentication(Url::parse(ORIGIN).unwrap(), start.options)
        .unwrap();
    finish_authentication(&reopened.auth, &start.ticket, "s:restored", assertion, true)
        .await
        .unwrap();
    assert!(!list(&reopened.auth, &user.id).await.unwrap().is_empty());
    let runtime = Runtime::configured(Some(ORIGIN)).unwrap();
    for i in 0..1024 {
        runtime
            .insert(&user, &format!("i:{i}"), State::Proof, 90)
            .unwrap();
    }
    assert!(
        runtime
            .insert(&user, "i:overflow", State::Proof, 90)
            .is_err()
    );
}

#[tokio::test]
async fn v8_migration_preserves_encrypted_totp_recovery_and_audit_history() {
    let (dir, state) = fixture().await;
    let (user, _session, inventory) = identity(&state).await;
    let setup = mfa::begin(&state.auth, &user, &inventory).await.unwrap();
    let generator = totp_rs::TOTP::new(
        totp_rs::Algorithm::SHA1,
        6,
        1,
        30,
        totp_rs::Secret::Encoded(setup.secret).to_bytes().unwrap(),
        Some("Agent Lens".into()),
        "member".into(),
    )
    .unwrap();
    let code = generator.generate((auth::unix_now() + 30) as u64);
    let (_, codes) = mfa::finish(&state.auth, &user, &inventory, &code)
        .await
        .unwrap()
        .unwrap();
    let schema: String =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='security_events'")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
    let legacy = schema.replace(",'passkey_added','passkey_used','passkey_removed'", "");
    assert!(!legacy.contains("'passkey_added'"));
    let mut tx = state.auth.pool.begin().await.unwrap();
    sqlx::raw_sql("DROP TRIGGER security_events_no_update; DROP INDEX idx_security_events_account_time; DROP INDEX idx_security_events_time; ALTER TABLE security_events RENAME TO saved_events;").execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(&legacy).execute(&mut *tx).await.unwrap();
    sqlx::raw_sql("INSERT INTO security_events SELECT * FROM saved_events; DROP TABLE saved_events; DROP TABLE account_passkeys; UPDATE app_state_metadata SET value='8' WHERE key='schema_version';").execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    state.auth.pool.close().await;
    let reopened = StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    let user = reopened
        .auth
        .authenticate("member", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert!(
        mfa::status(&reopened.auth, &user.id)
            .await
            .unwrap()
            .totp_enrolled
    );
    assert!(mfa::verify(&reopened.auth, &user, &codes[0]).await.unwrap());
    assert_eq!(
        mfa::status(&reopened.auth, &user.id)
            .await
            .unwrap()
            .recovery_count,
        9
    );
    assert!(list(&reopened.auth, &user.id).await.unwrap().is_empty());
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM security_events WHERE event_type='mfa_enabled'"
        )
        .fetch_one(&reopened.auth.pool)
        .await
        .unwrap(),
        1
    );
}
