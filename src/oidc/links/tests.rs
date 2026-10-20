use super::*;
use crate::{auth::StateStore, authorization::Capability, tokens};
use std::sync::Arc;
use tower_sessions::Session;

const PASSWORD: &str = "Synthetic1-password";
const ISSUER: &str = "https://identity.example.test/tenant";
const SUBJECT: &str = "synthetic-subject-SensitiveMarker";
type RetainedEvent = (i64, Option<String>, Option<String>, String, i64);

fn private_file(path: &Path, value: &[u8]) {
    std::fs::write(path, value).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
}

async fn fixture() -> (tempfile::TempDir, StateStore) {
    let dir = tempfile::tempdir().unwrap();
    let state = StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    let password = dir.path().join("password");
    private_file(&password, PASSWORD.as_bytes());
    for username in ["admin", "member", "other"] {
        auth::create_account(&state, username, &password, None)
            .await
            .unwrap();
    }
    (dir, state)
}

async fn identity(state: &StateStore, username: &str) -> (auth::CurrentUser, Session, String) {
    let (user, generation) = state
        .auth
        .authenticate(username, PASSWORD)
        .await
        .unwrap()
        .unwrap();
    let session = Session::new(None, Arc::new(state.sessions.clone()), None);
    auth::establish_session(&state.auth, &session, &user, generation, None)
        .await
        .unwrap();
    let token = tokens::create(
        &state.auth,
        &user,
        "Synthetic OIDC regression",
        "reports:read",
        1,
    )
    .await
    .unwrap();
    (user, session, token)
}

async fn generation(state: &StateStore, username: &str) -> i64 {
    sqlx::query_scalar("SELECT session_generation FROM accounts WHERE username=?")
        .bind(username)
        .fetch_one(&state.auth.pool)
        .await
        .unwrap()
}

async fn assert_valid(state: &StateStore, session: &Session, token: &str, valid: bool) {
    assert_eq!(
        state.auth.current_user(session).await.unwrap().is_some(),
        valid
    );
    assert_eq!(
        matches!(
            tokens::authenticate(&state.auth, token, Capability::ReportsRead)
                .await
                .unwrap(),
            tokens::Authentication::Authorized(_)
        ),
        valid
    );
}

#[test]
fn subjects_are_exact_bounded_ascii_and_files_are_private_without_normalization() {
    assert!(validate_identity(ISSUER, SUBJECT).is_ok());
    assert!(validate_identity(ISSUER, &"s".repeat(255)).is_ok());
    for value in [
        "".to_string(),
        "s".repeat(256),
        "subject\n".into(),
        "subject\0".into(),
        "sub\té".into(),
    ] {
        assert!(validate_identity(ISSUER, &value).is_err());
    }
    for issuer in [
        "http://identity.example.test",
        " https://identity.example.test",
        "https://identity.example.test\n",
        "https://identity.example.test/#x",
        "https://user:secret@identity.example.test",
    ] {
        assert!(validate_identity(issuer, SUBJECT).is_err());
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("subject");
    for (bytes, expected) in [
        (b" subject \r\n".as_slice(), " subject "),
        (b"subject\n", "subject"),
        (b"subject", "subject"),
    ] {
        private_file(&path, bytes);
        assert_eq!(
            read_subject(&path).unwrap(),
            expected,
            "strip only one file line ending, not identity whitespace"
        );
    }
    private_file(&path, b"subject\n\n");
    assert!(validate_identity(ISSUER, &read_subject(&path).unwrap()).is_err());
    private_file(&path, &vec![b's'; 258]);
    assert!(read_subject(&path).is_err());
    assert!(read_subject(dir.path()).is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::{PermissionsExt, symlink};
        private_file(&path, SUBJECT.as_bytes());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_subject(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let alias = dir.path().join("alias");
        symlink(&path, &alias).unwrap();
        assert!(read_subject(&alias).is_err());
    }
}

#[tokio::test]
async fn linking_is_exact_unique_account_scoped_and_revokes_credentials_without_changing_permissions()
 {
    let (_dir, state) = fixture().await;
    let source = format!("src_{}", "a".repeat(64));
    auth::grant_source(&state, "member", &source).await.unwrap();
    let (user, session, token) = identity(&state, "member").await;
    assert_valid(&state, &session, &token, true).await;
    let id = link(&state.auth, "MEMBER", ISSUER, SUBJECT).await.unwrap();
    assert_valid(&state, &session, &token, false).await;
    assert_eq!(
        generation(&state, "member").await,
        user.credential_generation + 1
    );
    let permissions = auth::account_permissions(&state, "member").await.unwrap();
    assert_eq!(permissions.role, "viewer");
    assert_eq!(permissions.source_keys, vec![source]);
    assert!(
        !list(&state.auth, "other")
            .await
            .unwrap()
            .iter()
            .any(|link| link.id == id)
    );
    let before = generation(&state, "member").await;
    for (username, subject) in [
        ("other", SUBJECT),
        ("member", "replacement-subject"),
        ("member", SUBJECT),
    ] {
        let error = link(&state.auth, username, ISSUER, subject)
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains(SUBJECT));
    }
    assert_eq!(
        generation(&state, "member").await,
        before,
        "failed linking must not revoke credentials"
    );
    assert!(unlink(&state.auth, "other", &id).await.is_err());
    assert!(unlink(&state.auth, "member", "invalid").await.is_err());
    // Subject and issuer comparisons are binary, not email/username matching or normalization.
    link(&state.auth, "other", ISSUER, &SUBJECT.to_lowercase())
        .await
        .unwrap();
    link(&state.auth, "other", &format!("{ISSUER}/"), SUBJECT)
        .await
        .unwrap();
    link(
        &state.auth,
        "other",
        "https://IDENTITY.example.test/tenant",
        SUBJECT,
    )
    .await
    .unwrap();
    let (current, current_session, current_token) = identity(&state, "member").await;
    assert_valid(&state, &current_session, &current_token, true).await;
    unlink(&state.auth, "member", &id).await.unwrap();
    assert_valid(&state, &current_session, &current_token, false).await;
    assert_eq!(
        generation(&state, "member").await,
        current.credential_generation + 1
    );
    assert!(list(&state.auth, "member").await.unwrap().is_empty());
    let events: Vec<String> = sqlx::query_scalar("SELECT event_type FROM security_events WHERE account_id=? AND event_type LIKE 'oidc_%' ORDER BY id")
        .bind(&user.id).fetch_all(&state.auth.pool).await.unwrap();
    assert_eq!(events, ["oidc_linked", "oidc_unlinked"]);
    assert!(
        state
            .auth
            .authenticate("member", PASSWORD)
            .await
            .unwrap()
            .is_some(),
        "local password remains usable"
    );
}

#[tokio::test]
async fn audit_failure_rolls_back_link_unlink_generation_and_session_revocation() {
    let (_dir, state) = fixture().await;
    let (_user, session, token) = identity(&state, "member").await;
    let before = generation(&state, "member").await;
    sqlx::raw_sql("CREATE TRIGGER reject_oidc_audit BEFORE INSERT ON security_events WHEN NEW.event_type='oidc_linked' BEGIN SELECT RAISE(ABORT,'synthetic audit failure'); END;")
        .execute(&state.auth.pool).await.unwrap();
    assert!(link(&state.auth, "member", ISSUER, SUBJECT).await.is_err());
    assert_eq!(generation(&state, "member").await, before);
    assert!(list(&state.auth, "member").await.unwrap().is_empty());
    assert_valid(&state, &session, &token, true).await;
    sqlx::raw_sql("DROP TRIGGER reject_oidc_audit")
        .execute(&state.auth.pool)
        .await
        .unwrap();
    let id = link(&state.auth, "member", ISSUER, SUBJECT).await.unwrap();
    let (_user, session, token) = identity(&state, "member").await;
    let before = generation(&state, "member").await;
    sqlx::raw_sql("CREATE TRIGGER reject_oidc_audit BEFORE INSERT ON security_events WHEN NEW.event_type='oidc_unlinked' BEGIN SELECT RAISE(ABORT,'synthetic audit failure'); END;")
        .execute(&state.auth.pool).await.unwrap();
    assert!(unlink(&state.auth, "member", &id).await.is_err());
    assert_eq!(generation(&state, "member").await, before);
    assert_eq!(list(&state.auth, "member").await.unwrap()[0].id, id);
    assert_valid(&state, &session, &token, true).await;
}

#[tokio::test]
async fn concurrent_claims_limits_disable_and_mfa_recovery_preserve_local_boundaries() {
    let (_dir, state) = fixture().await;
    let (a, b) = tokio::join!(
        link(&state.auth, "member", ISSUER, SUBJECT),
        link(&state.auth, "other", ISSUER, SUBJECT)
    );
    assert_ne!(
        a.is_ok(),
        b.is_ok(),
        "one exact identity cannot claim two local accounts"
    );
    let winner = if a.is_ok() { "member" } else { "other" };
    let loser = if a.is_ok() { "other" } else { "member" };
    assert_eq!(generation(&state, winner).await, 1);
    assert_eq!(generation(&state, loser).await, 0);
    for i in 0..MAX_LINKS {
        link(
            &state.auth,
            "admin",
            &format!("https://identity.example.test/tenant/{i}"),
            "subject",
        )
        .await
        .unwrap();
    }
    assert!(
        link(
            &state.auth,
            "admin",
            "https://another.example.test",
            "subject"
        )
        .await
        .is_err()
    );
    assert_eq!(
        list(&state.auth, "admin").await.unwrap().len(),
        MAX_LINKS as usize
    );
    assert_eq!(generation(&state, "admin").await, MAX_LINKS);
    let id = list(&state.auth, winner).await.unwrap().remove(0).id;
    sqlx::query(
        "INSERT INTO account_mfa(account_id,required) SELECT id,1 FROM accounts WHERE username=?",
    )
    .bind(winner)
    .execute(&state.auth.pool)
    .await
    .unwrap();
    auth::set_enabled(&state, winner, false).await.unwrap();
    assert!(
        link(
            &state.auth,
            winner,
            "https://disabled.example.test",
            "subject"
        )
        .await
        .is_err()
    );
    unlink(&state.auth, winner, &id).await.unwrap();
    let required: bool = sqlx::query_scalar("SELECT required FROM account_mfa WHERE account_id=(SELECT id FROM accounts WHERE username=?)")
        .bind(winner).fetch_one(&state.auth.pool).await.unwrap();
    assert!(
        required,
        "unlink must not clear an administrator's MFA requirement"
    );
    auth::set_enabled(&state, winner, true).await.unwrap();
    assert!(
        state
            .auth
            .authenticate(winner, PASSWORD)
            .await
            .unwrap()
            .is_some()
    );
    let admin_link = list(&state.auth, "admin").await.unwrap().remove(0).id;
    unlink(&state.auth, "admin", &admin_link).await.unwrap();
    assert_eq!(
        auth::account_permissions(&state, "admin")
            .await
            .unwrap()
            .role,
        "admin",
        "unlinking must preserve the last local administrator"
    );
    assert!(
        state
            .auth
            .authenticate("admin", PASSWORD)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn v9_migration_preserves_credentials_mfa_public_keys_permissions_and_append_only_audit() {
    let (dir, state) = fixture().await;
    let (user, session, token) = identity(&state, "member").await;
    let inventory = auth::current_inventory_id(&session).await.unwrap().unwrap();
    let _setup = crate::mfa::begin(&state.auth, &user, &inventory)
        .await
        .unwrap();
    let ciphertext: String =
        sqlx::query_scalar("SELECT pending_secret FROM account_mfa WHERE account_id=?")
            .bind(&user.id)
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
    // A public credential snapshot owned by another account; no private key material.
    sqlx::query("INSERT INTO account_passkeys(id,account_id,credential_id,credential,origin,name,sign_count) SELECT 'synthetic-key',id,X'0102','{}','https://lens.example.test','Synthetic device',7 FROM accounts WHERE username='admin'")
        .execute(&state.auth.pool).await.unwrap();
    let team = crate::access::create(&state, "Synthetic migration team")
        .await
        .unwrap();
    crate::access::membership(&state, &team, "member", true)
        .await
        .unwrap();
    let history: Vec<RetainedEvent> = sqlx::query_as(
        "SELECT id,account_id,team_id,event_type,occurred_at FROM security_events ORDER BY id",
    )
    .fetch_all(&state.auth.pool)
    .await
    .unwrap();
    let schema: String =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='security_events'")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
    let legacy = schema.replace(",'oidc_linked','oidc_unlinked'", "");
    assert!(!legacy.contains("'oidc_linked'"));
    let mut tx = state.auth.pool.begin().await.unwrap();
    sqlx::raw_sql("DROP TRIGGER security_events_no_update; DROP INDEX idx_security_events_account_time; DROP INDEX idx_security_events_time; ALTER TABLE security_events RENAME TO saved_events;")
        .execute(&mut *tx).await.unwrap();
    sqlx::raw_sql(&legacy).execute(&mut *tx).await.unwrap();
    sqlx::raw_sql("INSERT INTO security_events SELECT * FROM saved_events; DROP TABLE saved_events; DROP TABLE account_oidc_links; UPDATE app_state_metadata SET value='9' WHERE key='schema_version';")
        .execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    state.auth.pool.close().await;
    let reopened = StateStore::open(&dir.path().join("state.sqlite"))
        .await
        .unwrap();
    let retained: Vec<RetainedEvent> = sqlx::query_as(
        "SELECT id,account_id,team_id,event_type,occurred_at FROM security_events ORDER BY id",
    )
    .fetch_all(&reopened.auth.pool)
    .await
    .unwrap();
    assert_eq!(retained, history);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT value FROM app_state_metadata WHERE key='schema_version'"
        )
        .fetch_one(&reopened.auth.pool)
        .await
        .unwrap(),
        "11"
    );
    let ciphertext_after: String =
        sqlx::query_scalar("SELECT pending_secret FROM account_mfa WHERE account_id=?")
            .bind(&user.id)
            .fetch_one(&reopened.auth.pool)
            .await
            .unwrap();
    assert_eq!(ciphertext, ciphertext_after);
    let key: (String, i64) = sqlx::query_as(
        "SELECT credential,sign_count FROM account_passkeys WHERE id='synthetic-key'",
    )
    .fetch_one(&reopened.auth.pool)
    .await
    .unwrap();
    assert_eq!(key, ("{}".into(), 7));
    assert_eq!(
        crate::access::inspect(&reopened, &team).await.unwrap().1,
        vec!["member"]
    );
    assert_eq!(
        generation(&reopened, "member").await,
        user.credential_generation
    );
    assert_valid(&reopened, &session, &token, true).await;
    assert!(
        sqlx::query("UPDATE security_events SET event_type='oidc_linked'")
            .execute(&reopened.auth.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("INSERT INTO security_events(event_type) VALUES('unknown_event')")
            .execute(&reopened.auth.pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&reopened.auth.pool)
            .await
            .unwrap()
            .is_empty()
    );
    link(&reopened.auth, "member", ISSUER, SUBJECT)
        .await
        .unwrap();
}
