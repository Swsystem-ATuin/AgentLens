use super::*;
use crate::auth::{AccountRole, StateStore};
use std::sync::Arc;
use tower_sessions::Session;

const PASSWORD: &str = "Synthetic1-password";
async fn fixture() -> (tempfile::TempDir, StateStore) {
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
    auth::create_account(&state, "member", &path, Some(AccountRole::Viewer))
        .await
        .unwrap();
    (dir, state)
}
async fn identity(state: &StateStore, name: &str) -> (CurrentUser, Session, String) {
    let (user, generation) = state
        .auth
        .authenticate(name, PASSWORD)
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
fn enrollment_code(enrollment: &Enrollment) -> String {
    totp(
        Secret::Encoded(enrollment.secret.clone())
            .to_bytes()
            .unwrap(),
        "synthetic",
    )
    .unwrap()
    .generate((auth::unix_now() + 30) as u64)
}
async fn enrolled(state: &StateStore) -> (CurrentUser, Vec<String>) {
    let (user, _session, inventory) = identity(state, "member").await;
    let setup = begin(&state.auth, &user, &inventory).await.unwrap();
    let code = enrollment_code(&setup);
    let (_, codes) = finish(&state.auth, &user, &inventory, &code)
        .await
        .unwrap()
        .unwrap();
    let user = state
        .auth
        .authenticate("member", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    (user, codes)
}

#[test]
fn totp_vectors_drift_replay_and_authenticated_encryption_are_strict() {
    let secret = b"12345678901234567890".to_vec();
    assert_eq!(
        matched_step(secret.clone(), "287082", 59, -1).unwrap(),
        Some(1)
    );
    assert_eq!(matched_step(secret.clone(), "287082", 59, 1).unwrap(), None);
    assert_eq!(
        matched_step(secret.clone(), "287082", 89, -1).unwrap(),
        Some(1)
    );
    assert_eq!(
        matched_step(secret.clone(), "287082", 119, -1).unwrap(),
        None
    );
    assert_eq!(matched_step(secret, "287082 ", 59, -1).unwrap(), None);
    let (id, key) = fresh_key();
    let ring = Keyring {
        version: 1,
        active: id.clone(),
        keys: BTreeMap::from([(id, key)]),
    };
    let ciphertext = seal(&ring, "account-a", "totp", b"synthetic secret").unwrap();
    assert_eq!(
        open(&ring, "account-a", "totp", &ciphertext).unwrap(),
        b"synthetic secret"
    );
    assert!(open(&ring, "account-b", "totp", &ciphertext).is_err());
    assert!(open(&ring, "account-a", "pending", &ciphertext).is_err());
    assert!(!ciphertext.contains("synthetic secret"));
    assert_ne!(
        ciphertext,
        seal(&ring, "account-a", "totp", b"synthetic secret").unwrap()
    );
    let mut envelope: Envelope = serde_json::from_str(&ciphertext).unwrap();
    envelope.version = 99;
    assert!(
        open(
            &ring,
            "account-a",
            "totp",
            &serde_json::to_string(&envelope).unwrap()
        )
        .is_err()
    );
    envelope.version = 1;
    envelope.nonce = STANDARD.encode([0u8; 3]);
    assert!(
        open(
            &ring,
            "account-a",
            "totp",
            &serde_json::to_string(&envelope).unwrap()
        )
        .is_err()
    );
}

#[tokio::test]
async fn enrollment_is_session_bound_encrypted_and_consumes_totp_and_recovery_once() {
    let (_dir, state) = fixture().await;
    let (user, session, inventory) = identity(&state, "member").await;
    assert!(!status(&state.auth, &user.id).await.unwrap().enrolled);
    let setup = begin(&state.auth, &user, &inventory).await.unwrap();
    let code = enrollment_code(&setup);
    let stored: String =
        sqlx::query_scalar("SELECT pending_secret FROM account_mfa WHERE account_id=?")
            .bind(&user.id)
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
    assert!(!stored.contains(&setup.secret));
    assert!(!stored.contains("otpauth://"));
    let (_, _, other) = identity(&state, "member").await;
    assert!(
        finish(&state.auth, &user, &other, &code)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        finish(&state.auth, &user, &inventory, "not-a-code")
            .await
            .unwrap()
            .is_none()
    );
    let (generation, codes) = finish(&state.auth, &user, &inventory, &code)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(generation, 1);
    assert_eq!(codes.len(), 10);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_sessions WHERE account_id=? AND revoked_at IS NULL",
    )
    .bind(&user.id)
    .fetch_one(&state.auth.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
    assert!(session.id().is_some());
    assert!(
        !verify(&state.auth, &user, &codes[0]).await.unwrap(),
        "stale credential generation must not consume recovery"
    );
    let user = state
        .auth
        .authenticate("member", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert!(
        !verify(&state.auth, &user, &code).await.unwrap(),
        "enrollment time step must not be replayable"
    );
    let (a, b) = tokio::join!(
        verify(&state.auth, &user, &codes[0]),
        verify(&state.auth, &user, &codes[0])
    );
    assert_eq!(
        usize::from(a.unwrap()) + usize::from(b.unwrap()),
        1,
        "recovery use must be atomic across concurrent requests"
    );
    assert_eq!(
        status(&state.auth, &user.id).await.unwrap().recovery_count,
        9
    );
    let digests: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT digest FROM mfa_recovery_codes WHERE account_id=?")
            .bind(&user.id)
            .fetch_all(&state.auth.pool)
            .await
            .unwrap();
    for digest in digests {
        assert_eq!(digest.len(), 32);
        assert!(codes.iter().all(|c| digest != c.as_bytes()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&state.auth.mfa_key_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[tokio::test]
async fn pending_enrollment_expires_and_stale_identity_cannot_activate_it() {
    let (_dir, state) = fixture().await;
    let (user, _session, inventory) = identity(&state, "member").await;
    let setup = begin(&state.auth, &user, &inventory).await.unwrap();
    let code = enrollment_code(&setup);
    sqlx::query("UPDATE account_mfa SET pending_expires_at=unixepoch()-1")
        .execute(&state.auth.pool)
        .await
        .unwrap();
    assert!(
        finish(&state.auth, &user, &inventory, &code)
            .await
            .unwrap()
            .is_none()
    );
    begin(&state.auth, &user, &inventory).await.unwrap();
    auth::revoke_sessions(&state, "member").await.unwrap();
    assert!(finish(&state.auth, &user, &inventory, &code).await.is_err());
    assert!(!status(&state.auth, &user.id).await.unwrap().enrolled);
}

#[tokio::test]
async fn required_mfa_cannot_be_self_disabled_and_recovery_rotation_invalidates_old_codes() {
    let (_dir, state) = fixture().await;
    let (user, codes) = enrolled(&state).await;
    let (user, _, inventory) = identity(&state, &user.username).await;
    sqlx::query("UPDATE account_mfa SET required=1 WHERE account_id=?")
        .bind(&user.id)
        .execute(&state.auth.pool)
        .await
        .unwrap();
    assert!(
        manage(&state.auth, &user, &inventory, &codes[0], true)
            .await
            .is_err()
    );
    assert_eq!(
        status(&state.auth, &user.id).await.unwrap().recovery_count,
        10
    );
    let (_, new_codes) = manage(&state.auth, &user, &inventory, &codes[0], false)
        .await
        .unwrap()
        .unwrap();
    let user = state
        .auth
        .authenticate("member", PASSWORD)
        .await
        .unwrap()
        .unwrap()
        .0;
    assert!(!verify(&state.auth, &user, &codes[1]).await.unwrap());
    assert!(verify(&state.auth, &user, &new_codes[0]).await.unwrap());
    cli_reset(&state.auth, "member").await.unwrap();
    let status = status(&state.auth, &user.id).await.unwrap();
    assert!(status.required);
    assert!(!status.enrolled);
    assert_eq!(status.recovery_count, 0);
    assert!(
        state
            .auth
            .token_account(&user.id, user.credential_generation + 1)
            .await
            .unwrap()
            .is_none(),
        "required enrollment must also gate token identities"
    );
}

#[tokio::test]
async fn factor_budget_survives_restart_without_disabling_accounts() {
    let (dir, state) = fixture().await;
    let (user, codes) = enrolled(&state).await;
    for _ in 0..10 {
        assert!(!verify(&state.auth, &user, "invalid").await.unwrap());
    }
    assert!(!verify(&state.auth, &user, &codes[0]).await.unwrap());
    state.auth.pool.close().await;
    let state = StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    assert!(!verify(&state.auth, &user, &codes[0]).await.unwrap());
    assert!(
        state
            .auth
            .authenticate("member", PASSWORD)
            .await
            .unwrap()
            .is_some()
    );
    sqlx::query("UPDATE account_mfa SET attempt_window=unixepoch()-301 WHERE account_id=?")
        .bind(&user.id)
        .execute(&state.auth.pool)
        .await
        .unwrap();
    assert!(verify(&state.auth, &user, &codes[0]).await.unwrap());
}

#[tokio::test]
async fn key_rotation_rewraps_pending_and_enrolled_secrets_and_missing_keys_fail_closed() {
    let (dir, state) = fixture().await;
    let (user, codes) = enrolled(&state).await;
    let (admin, _, inventory) = identity(&state, "admin").await;
    let setup = begin(&state.auth, &admin, &inventory).await.unwrap();
    let old: String = sqlx::query_scalar("SELECT secret FROM account_mfa WHERE account_id=?")
        .bind(&user.id)
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
    let old_ring = key_file(&state.auth.mfa_key_file).unwrap();
    rotate_key(&state.auth).await.unwrap();
    let new: String = sqlx::query_scalar("SELECT secret FROM account_mfa WHERE account_id=?")
        .bind(&user.id)
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
    let ring = key_file(&state.auth.mfa_key_file).unwrap();
    assert_ne!(ring.active, old_ring.active);
    assert_ne!(old, new);
    assert!(open(&old_ring, &user.id, "totp", &new).is_err());
    assert_eq!(
        open(&ring, &user.id, "totp", &old).unwrap(),
        open(&ring, &user.id, "totp", &new).unwrap()
    );
    assert!(verify(&state.auth, &user, &codes[0]).await.unwrap());
    assert!(
        finish(&state.auth, &admin, &inventory, &enrollment_code(&setup))
            .await
            .unwrap()
            .is_some()
    );
    sqlx::raw_sql("CREATE TRIGGER reject_mfa_rewrap BEFORE UPDATE OF secret ON account_mfa BEGIN SELECT RAISE(ABORT,'synthetic rewrap failure'); END;").execute(&state.auth.pool).await.unwrap();
    assert!(rotate_key(&state.auth).await.is_err());
    check_keys(&state.auth).await.unwrap();
    assert!(
        verify(&state.auth, &user, &codes[1]).await.unwrap(),
        "interrupted rewrap must retain decryptability of committed ciphertext"
    );
    sqlx::query("DROP TRIGGER reject_mfa_rewrap")
        .execute(&state.auth.pool)
        .await
        .unwrap();
    let backup = dir.path().join("saved-keyring.json");
    std::fs::rename(&state.auth.mfa_key_file, &backup).unwrap();
    assert!(check_keys(&state.auth).await.is_err());
    assert!(begin(&state.auth, &user, &inventory).await.is_err());
    state.auth.pool.close().await;
    assert!(
        StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .is_err()
    );
    let recovered = StateStore::open_for_mfa_recovery(&dir.path().join("state.sqlite"), None)
        .await
        .unwrap();
    cli_reset(&recovered.auth, "member").await.unwrap();
    cli_reset(&recovered.auth, "admin").await.unwrap();
    recovered.auth.pool.close().await;
    StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
}

#[tokio::test]
async fn v7_migration_preserves_team_provenance_audit_facts_and_credentials() {
    let (dir, state) = fixture().await;
    let (admin, _session, _inventory) = identity(&state, "admin").await;
    let token = crate::tokens::create(
        &state.auth,
        &admin,
        "Synthetic migration",
        "reports:read",
        1,
    )
    .await
    .unwrap();
    let team = crate::access::create(&state, "Synthetic team")
        .await
        .unwrap();
    crate::access::membership(&state, &team, "member", true)
        .await
        .unwrap();
    sqlx::query("INSERT INTO admin_events(actor_id,account_id,operation,subject,before_value,after_value) VALUES(?,?,'set_role','','viewer','viewer')").bind(&admin.id).bind(&admin.id).execute(&state.auth.pool).await.unwrap();
    let event_sql: String =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='security_events'")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
    let admin_sql: String =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='admin_events'")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
    let old_events=event_sql.replace(",'mfa_enrollment_started','mfa_enabled','mfa_disabled','mfa_failed','mfa_recovery_used','mfa_recovery_rotated','mfa_reset','mfa_required','mfa_released','mfa_key_rotated'","").replace(",'passkey_added','passkey_used','passkey_removed'","").replace(",'oidc_linked','oidc_unlinked'", "");
    assert!(!old_events.contains("'mfa_enabled'"));
    let old_admin = admin_sql.replace(",'require_mfa','release_mfa','reset_mfa'", "");
    assert!(!old_admin.contains("'require_mfa'"));
    let mut tx = state.auth.pool.begin().await.unwrap();
    sqlx::raw_sql("DROP TRIGGER security_events_no_update; DROP INDEX idx_security_events_account_time; DROP INDEX idx_security_events_time; ALTER TABLE security_events RENAME TO saved_events; DROP TRIGGER admin_events_no_update; DROP INDEX idx_admin_events_time; ALTER TABLE admin_events RENAME TO saved_admin;").execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(&old_events).execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(&old_admin).execute(&mut *tx).await.unwrap();
    sqlx::raw_sql("INSERT INTO security_events SELECT * FROM saved_events; DROP TABLE saved_events; INSERT INTO admin_events SELECT * FROM saved_admin; DROP TABLE saved_admin; CREATE TRIGGER admin_events_no_update BEFORE UPDATE ON admin_events BEGIN SELECT RAISE(ABORT,'administration events are append-only'); END; CREATE INDEX idx_admin_events_time ON admin_events(occurred_at DESC,id DESC); DROP TABLE mfa_recovery_codes; DROP TABLE account_mfa; UPDATE app_state_metadata SET value='7' WHERE key='schema_version';").execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    state.auth.pool.close().await;
    let state = StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM security_events WHERE team_id=?")
        .bind(&team)
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
    assert_eq!(retained, 2);
    let facts: (String, String) =
        sqlx::query_as("SELECT before_value,after_value FROM admin_events")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
    assert_eq!(facts, ("viewer".into(), "viewer".into()));
    assert!(matches!(
        crate::tokens::authenticate(
            &state.auth,
            &token,
            crate::authorization::Capability::ReportsRead
        )
        .await
        .unwrap(),
        crate::tokens::Authentication::Authorized(_)
    ));
    assert!(
        sqlx::query("UPDATE security_events SET event_type='mfa_enabled'")
            .execute(&state.auth.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE admin_events SET operation='require_mfa'")
            .execute(&state.auth.pool)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT value FROM app_state_metadata WHERE key='schema_version'"
        )
        .fetch_one(&state.auth.pool)
        .await
        .unwrap(),
        "10"
    );
    let (user, _session, inventory) = identity(&state, "member").await;
    begin(&state.auth, &user, &inventory).await.unwrap();
}
