//! Personal access token storage and verification. Bearer values are never stored.
use anyhow::{Context, Result, ensure};
use password_hash::rand_core::{OsRng, RngCore};
use sqlx::SqlitePool;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use subtle::ConstantTimeEq;

#[derive(Clone)]
pub(crate) struct AttemptBudget(Arc<Mutex<(Instant, u32)>>);

impl Default for AttemptBudget {
    fn default() -> Self {
        Self(Arc::new(Mutex::new((Instant::now(), 0))))
    }
}

impl AttemptBudget {
    pub(crate) fn accept(&self) -> bool {
        let mut window = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if window.0.elapsed().as_secs() >= 60 {
            *window = (Instant::now(), 0);
        }
        if window.1 >= 600 {
            return false;
        }
        window.1 += 1;
        true
    }
}

use crate::{
    auth::{AuthState, CurrentUser},
    authorization::Capability,
};

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS personal_access_tokens (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    description TEXT NOT NULL,
    scope TEXT NOT NULL CHECK(scope IN ('metrics:read','reports:read','content:read')),
    digest_version INTEGER NOT NULL DEFAULT 1,
    digest BLOB NOT NULL,
    account_generation INTEGER NOT NULL,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    expires_at INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at INTEGER,
    rate_window INTEGER NOT NULL DEFAULT 0,
    rate_count INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_tokens_account_created ON personal_access_tokens(account_id,created_at DESC,id DESC);
"#;

pub(crate) fn scope_capability(scope: &str) -> Option<Capability> {
    match scope {
        "metrics:read" => Some(Capability::OperationsRead),
        "reports:read" => Some(Capability::ReportsRead),
        "content:read" => Some(Capability::ContentRead),
        _ => None,
    }
}

pub(crate) fn validate(description: &str, scope: &str, days: i64) -> Result<(), &'static str> {
    if description.trim().is_empty()
        || description.chars().count() > 80
        || description.chars().any(char::is_control)
    {
        return Err("Description must contain 1–80 characters without control characters.");
    }
    if scope_capability(scope).is_none() {
        return Err("Choose a supported read scope.");
    }
    if !(1..=90).contains(&days) {
        return Err("Expiry must be between 1 and 90 days.");
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct CreationDenied;

impl std::fmt::Display for CreationDenied {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("account unavailable or token creation limit reached")
    }
}

impl std::error::Error for CreationDenied {}

pub(crate) async fn create(
    auth: &AuthState,
    user: &CurrentUser,
    description: &str,
    scope: &str,
    days: i64,
) -> Result<String> {
    validate(description, scope, days).map_err(anyhow::Error::msg)?;
    let capability = scope_capability(scope).context("invalid token scope")?;
    ensure!(
        user.access.permits(Capability::TokensManage) && user.access.permits(capability),
        "token scope is unavailable to this account"
    );
    let mut identifier = [0_u8; 8];
    let mut secret = [0_u8; 32];
    OsRng.fill_bytes(&mut identifier);
    OsRng.fill_bytes(&mut secret);
    let id = hex(&identifier);
    let bearer = format!("al_pat_{id}_{}", hex(&secret));
    let digest = digest(&bearer);
    let mut transaction = auth.pool.begin().await.context("begin token creation")?;
    prune(&mut transaction).await?;
    // The INSERT takes the writer lock and checks current account and limits atomically.
    let result = sqlx::query("INSERT INTO personal_access_tokens(id,account_id,description,scope,digest,account_generation,expires_at) SELECT ?,id,?,?,?,session_generation,unixepoch()+? FROM accounts WHERE id=? AND enabled=1 AND session_generation=? AND role=? AND (SELECT count(*) FROM personal_access_tokens WHERE account_id=? AND revoked_at IS NULL AND expires_at>unixepoch())<100 AND (SELECT count(*) FROM personal_access_tokens WHERE account_id=? AND created_at>=unixepoch()-3600)<10")
        .bind(&id).bind(description.trim()).bind(scope).bind(digest.as_slice())
        .bind(days * 86_400).bind(&user.id).bind(user.credential_generation).bind(user.access.role()).bind(&user.id).bind(&user.id)
        .execute(&mut *transaction).await.context("create personal access token")?;
    if result.rows_affected() != 1 {
        return Err(CreationDenied.into());
    }
    crate::auth::insert_security_event(
        &mut transaction,
        Some(&user.id),
        crate::auth::SecurityEvent::TokenCreated,
    )
    .await?;
    transaction
        .commit()
        .await
        .context("commit token creation")?;
    Ok(bearer)
}

#[derive(sqlx::FromRow)]
pub(crate) struct TokenView {
    pub(crate) id: String,
    pub(crate) description: String,
    pub(crate) scope: String,
    pub(crate) created_at: String,
    pub(crate) expires_at: String,
    pub(crate) last_used_at: Option<String>,
    pub(crate) status: String,
}

pub(crate) async fn list(
    pool: &SqlitePool,
    account_id: &str,
    offset: i64,
    limit: usize,
) -> Result<Vec<TokenView>> {
    sqlx::query_as("SELECT t.id,t.description,t.scope,strftime('%Y-%m-%dT%H:%M:%SZ',t.created_at,'unixepoch') AS created_at,strftime('%Y-%m-%dT%H:%M:%SZ',t.expires_at,'unixepoch') AS expires_at,strftime('%Y-%m-%dT%H:%M:%SZ',t.last_used_at,'unixepoch') AS last_used_at,CASE WHEN t.revoked_at IS NOT NULL OR t.account_generation<>a.session_generation THEN 'revoked' WHEN t.expires_at<=unixepoch() THEN 'expired' ELSE 'active' END AS status FROM personal_access_tokens t JOIN accounts a ON a.id=t.account_id WHERE t.account_id=? ORDER BY t.created_at DESC,t.id DESC LIMIT ? OFFSET ?")
        .bind(account_id).bind(limit as i64).bind(offset).fetch_all(pool).await.context("list personal access tokens")
}

pub(crate) async fn revoke(auth: &AuthState, account_id: &str, id: &str) -> Result<bool> {
    let mut transaction = auth.pool.begin().await.context("begin token revocation")?;
    let result = sqlx::query("UPDATE personal_access_tokens SET revoked_at=unixepoch() WHERE id=? AND account_id=? AND revoked_at IS NULL")
        .bind(id).bind(account_id).execute(&mut *transaction).await.context("revoke personal access token")?;
    if result.rows_affected() == 0 {
        return Ok(false);
    }
    crate::auth::insert_security_event(
        &mut transaction,
        Some(account_id),
        crate::auth::SecurityEvent::TokenRevoked,
    )
    .await?;
    transaction
        .commit()
        .await
        .context("commit token revocation")?;
    Ok(true)
}

pub(crate) enum Authentication {
    Authorized(Box<CurrentUser>),
    Invalid,
    Denied,
    RateLimited,
}

pub(crate) async fn authenticate(
    auth: &AuthState,
    bearer: &str,
    required: Capability,
) -> Result<Authentication> {
    // Fixed syntax keeps lookup bounded and excludes arbitrary header content.
    let Some((id, secret)) = bearer
        .strip_prefix("al_pat_")
        .and_then(|value| value.split_once('_'))
    else {
        return Ok(Authentication::Invalid);
    };
    if id.len() != 16
        || secret.len() != 64
        || !id
            .bytes()
            .chain(secret.bytes())
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Ok(Authentication::Invalid);
    }
    type Row = (String, Vec<u8>, i64, String, i64);
    let row: Option<Row> = sqlx::query_as("SELECT account_id,digest,digest_version,scope,account_generation FROM personal_access_tokens WHERE id=? AND revoked_at IS NULL AND expires_at>unixepoch()")
        .bind(id).fetch_optional(&auth.pool).await.context("load token verifier")?;
    let candidate = digest(bearer);
    let valid = row.as_ref().is_some_and(|row| {
        row.2 == 1 && row.1.len() == 32 && bool::from(row.1.as_slice().ct_eq(candidate.as_slice()))
    });
    if !valid {
        return Ok(Authentication::Invalid);
    }
    let Some((account_id, _, _, scope, generation)) = row else {
        return Ok(Authentication::Invalid);
    };
    let Some(mut user) = auth.token_account(&account_id, generation).await? else {
        return Ok(Authentication::Invalid);
    };
    let Some(capability) = scope_capability(&scope) else {
        return Ok(Authentication::Invalid);
    };
    user.access
        .constrain(&[capability, Capability::AllSourcesRead]);
    if !user.access.permits(required) {
        return Ok(Authentication::Denied);
    }
    let result = sqlx::query("UPDATE personal_access_tokens SET last_used_at=unixepoch(),rate_count=CASE WHEN rate_window<>unixepoch()/60 THEN 1 ELSE rate_count+1 END,rate_window=unixepoch()/60 WHERE id=? AND revoked_at IS NULL AND expires_at>unixepoch() AND account_generation=? AND EXISTS(SELECT 1 FROM accounts WHERE id=personal_access_tokens.account_id AND enabled=1 AND session_generation=? AND role=?) AND (rate_window<>unixepoch()/60 OR rate_count<60)")
        .bind(id).bind(generation).bind(generation).bind(user.access.role())
        .execute(&auth.pool).await.context("record authorized token use")?;
    if result.rows_affected() == 0 {
        // Distinguish exhaustion from concurrent revocation/disablement without accepting the request.
        let exhausted: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM personal_access_tokens t JOIN accounts a ON a.id=t.account_id WHERE t.id=? AND t.revoked_at IS NULL AND t.expires_at>unixepoch() AND a.enabled=1 AND a.session_generation=t.account_generation AND t.rate_window=unixepoch()/60 AND t.rate_count>=60)")
            .bind(id).fetch_one(&auth.pool).await.context("check token rate limit")?;
        return Ok(if exhausted {
            Authentication::RateLimited
        } else {
            Authentication::Invalid
        });
    }
    Ok(Authentication::Authorized(Box::new(user)))
}

pub(crate) async fn prune(transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>) -> Result<()> {
    sqlx::query("DELETE FROM personal_access_tokens WHERE (revoked_at IS NOT NULL AND revoked_at<unixepoch()-30*86400) OR expires_at<unixepoch()-30*86400")
        .execute(&mut **transaction).await.context("prune expired token metadata")?;
    Ok(())
}

fn digest(bearer: &str) -> [u8; 32] {
    blake3::derive_key(
        "Agent Lens personal access token digest v1",
        bearer.as_bytes(),
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AccountRole, StateStore};

    async fn fixture() -> (tempfile::TempDir, StateStore, CurrentUser, CurrentUser) {
        let dir = tempfile::tempdir().unwrap();
        let state = StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        let password_file = dir.path().join("password");
        std::fs::write(&password_file, "Synthetic1-password").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&password_file, std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        crate::auth::create_account(&state, "admin", &password_file, None)
            .await
            .unwrap();
        crate::auth::create_account(&state, "viewer", &password_file, None)
            .await
            .unwrap();
        let admin = state
            .auth
            .authenticate("admin", "Synthetic1-password")
            .await
            .unwrap()
            .unwrap()
            .0;
        let viewer = state
            .auth
            .authenticate("viewer", "Synthetic1-password")
            .await
            .unwrap()
            .unwrap()
            .0;
        (dir, state, admin, viewer)
    }

    #[tokio::test]
    async fn tokens_store_only_digests_and_enforce_scope_revocation_and_account_state() {
        let (_dir, state, admin, viewer) = fixture().await;
        let bearer = create(&state.auth, &admin, "Synthetic metrics", "metrics:read", 30)
            .await
            .unwrap();
        assert_eq!(bearer.len(), 88);
        assert!(bearer.starts_with("al_pat_"));
        let rows = list(&state.auth.pool, &admin.id, 0, 26).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].scope, "metrics:read");
        assert!(rows[0].last_used_at.is_none());
        assert!(
            list(&state.auth.pool, &viewer.id, 0, 26)
                .await
                .unwrap()
                .is_empty()
        );
        let stored: Vec<u8> =
            sqlx::query_scalar("SELECT digest FROM personal_access_tokens WHERE id=?")
                .bind(&rows[0].id)
                .fetch_one(&state.auth.pool)
                .await
                .unwrap();
        assert_eq!(stored.len(), 32);
        assert!(!stored.windows(7).any(|window| window == b"al_pat_"));
        assert!(matches!(
            authenticate(&state.auth, &bearer, Capability::OperationsRead)
                .await
                .unwrap(),
            Authentication::Authorized(_)
        ));
        assert!(
            list(&state.auth.pool, &admin.id, 0, 26).await.unwrap()[0]
                .last_used_at
                .is_some()
        );
        assert!(matches!(
            authenticate(&state.auth, &bearer, Capability::ReportsRead)
                .await
                .unwrap(),
            Authentication::Denied
        ));
        let altered = format!(
            "{}{}",
            &bearer[..87],
            if bearer.ends_with('a') { 'b' } else { 'a' }
        );
        assert!(matches!(
            authenticate(&state.auth, &altered, Capability::OperationsRead)
                .await
                .unwrap(),
            Authentication::Invalid
        ));
        assert!(
            create(
                &state.auth,
                &viewer,
                "Invalid privilege",
                "metrics:read",
                30
            )
            .await
            .is_err()
        );
        crate::auth::set_role(&state, "admin", AccountRole::Viewer)
            .await
            .unwrap();
        assert!(matches!(
            authenticate(&state.auth, &bearer, Capability::OperationsRead)
                .await
                .unwrap(),
            Authentication::Denied
        ));
        crate::auth::set_role(&state, "admin", AccountRole::Admin)
            .await
            .unwrap();
        assert!(!revoke(&state.auth, &viewer.id, &rows[0].id).await.unwrap());
        assert!(revoke(&state.auth, &admin.id, &rows[0].id).await.unwrap());
        assert!(!revoke(&state.auth, &admin.id, &rows[0].id).await.unwrap());
        assert!(matches!(
            authenticate(&state.auth, &bearer, Capability::OperationsRead)
                .await
                .unwrap(),
            Authentication::Invalid
        ));
        let bearer = create(&state.auth, &viewer, "Synthetic reports", "reports:read", 1)
            .await
            .unwrap();
        crate::auth::set_enabled(&state, "viewer", false)
            .await
            .unwrap();
        assert!(matches!(
            authenticate(&state.auth, &bearer, Capability::ReportsRead)
                .await
                .unwrap(),
            Authentication::Invalid
        ));
        crate::auth::set_enabled(&state, "viewer", true)
            .await
            .unwrap();
        assert!(matches!(
            authenticate(&state.auth, &bearer, Capability::ReportsRead)
                .await
                .unwrap(),
            Authentication::Invalid
        ));
        let events: Vec<String> = sqlx::query_scalar("SELECT event_type FROM security_events WHERE event_type IN ('token_created','token_revoked')")
            .fetch_all(&state.auth.pool).await.unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|kind| *kind == "token_created")
                .count(),
            2
        );
        assert_eq!(
            events
                .iter()
                .filter(|kind| *kind == "token_revoked")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn expiry_rate_limits_versioning_retention_and_creation_limits_fail_closed() {
        let (_dir, state, admin, _viewer) = fixture().await;
        for (description, scope, days) in [
            ("", "reports:read", 1),
            ("name\n", "reports:read", 1),
            ("name", "unknown", 1),
            ("name", "reports:read", 0),
            ("name", "reports:read", 91),
        ] {
            assert!(
                create(&state.auth, &admin, description, scope, days)
                    .await
                    .is_err()
            );
        }
        let bearer = create(&state.auth, &admin, "Synthetic report", "reports:read", 1)
            .await
            .unwrap();
        let id = &bearer[7..23];
        sqlx::query("UPDATE personal_access_tokens SET digest_version=2 WHERE id=?")
            .bind(id)
            .execute(&state.auth.pool)
            .await
            .unwrap();
        assert!(matches!(
            authenticate(&state.auth, &bearer, Capability::ReportsRead)
                .await
                .unwrap(),
            Authentication::Invalid
        ));
        sqlx::query("UPDATE personal_access_tokens SET digest_version=1,expires_at=unixepoch()-1 WHERE id=?").bind(id).execute(&state.auth.pool).await.unwrap();
        assert!(matches!(
            authenticate(&state.auth, &bearer, Capability::ReportsRead)
                .await
                .unwrap(),
            Authentication::Invalid
        ));
        sqlx::query("UPDATE personal_access_tokens SET expires_at=unixepoch()+3600,rate_window=unixepoch()/60,rate_count=60 WHERE id=?").bind(id).execute(&state.auth.pool).await.unwrap();
        assert!(matches!(
            authenticate(&state.auth, &bearer, Capability::ReportsRead)
                .await
                .unwrap(),
            Authentication::RateLimited
        ));
        sqlx::query("UPDATE personal_access_tokens SET rate_window=unixepoch()/60-1 WHERE id=?")
            .bind(id)
            .execute(&state.auth.pool)
            .await
            .unwrap();
        assert!(matches!(
            authenticate(&state.auth, &bearer, Capability::ReportsRead)
                .await
                .unwrap(),
            Authentication::Authorized(_)
        ));
        for index in 0..9 {
            create(
                &state.auth,
                &admin,
                &format!("Synthetic {index}"),
                "reports:read",
                1,
            )
            .await
            .unwrap();
        }
        assert!(
            create(&state.auth, &admin, "Over limit", "reports:read", 1)
                .await
                .is_err()
        );
        sqlx::query("UPDATE personal_access_tokens SET expires_at=unixepoch()-31*86400 WHERE id=?")
            .bind(id)
            .execute(&state.auth.pool)
            .await
            .unwrap();
        let mut transaction = state.auth.pool.begin().await.unwrap();
        prune(&mut transaction).await.unwrap();
        transaction.commit().await.unwrap();
        assert_eq!(
            list(&state.auth.pool, &admin.id, 0, 100)
                .await
                .unwrap()
                .len(),
            9
        );
        sqlx::query("UPDATE personal_access_tokens SET created_at=unixepoch()-7200")
            .execute(&state.auth.pool)
            .await
            .unwrap();
        sqlx::query("WITH RECURSIVE n(value) AS (SELECT 1 UNION ALL SELECT value+1 FROM n WHERE value<91) INSERT INTO personal_access_tokens(id,account_id,description,scope,digest,account_generation,created_at,expires_at) SELECT 'limit-'||value,?,'Synthetic active limit','reports:read',zeroblob(32),0,unixepoch()-7200,unixepoch()+3600 FROM n")
            .bind(&admin.id).execute(&state.auth.pool).await.unwrap();
        assert!(
            create(&state.auth, &admin, "Over active limit", "reports:read", 1)
                .await
                .is_err()
        );
        crate::auth::revoke_sessions(&state, "admin").await.unwrap();
        assert!(
            create(&state.auth, &admin, "Stale confirmation", "reports:read", 1)
                .await
                .is_err()
        );
    }

    #[test]
    fn failed_attempt_budget_is_bounded_and_shared() {
        let budget = AttemptBudget::default();
        let second = budget.clone();
        for _ in 0..600 {
            assert!(budget.accept());
        }
        assert!(!second.accept());
        budget.0.lock().unwrap().0 = Instant::now() - std::time::Duration::from_secs(60);
        assert!(second.accept());
    }

    #[tokio::test]
    async fn concurrent_revocation_cannot_restore_a_token_and_stale_confirmation_cannot_mint_one() {
        let (_dir, state, admin, _) = fixture().await;
        let bearer = create(
            &state.auth,
            &admin,
            "Synthetic concurrent token",
            "reports:read",
            1,
        )
        .await
        .unwrap();
        let id = &bearer[7..23];
        let (revoked, authenticated) = tokio::join!(
            revoke(&state.auth, &admin.id, id),
            authenticate(&state.auth, &bearer, Capability::ReportsRead),
        );
        assert!(revoked.unwrap());
        assert!(matches!(
            authenticated.unwrap(),
            Authentication::Authorized(_) | Authentication::Invalid
        ));
        assert!(matches!(
            authenticate(&state.auth, &bearer, Capability::ReportsRead)
                .await
                .unwrap(),
            Authentication::Invalid
        ));
        crate::auth::revoke_sessions(&state, "admin").await.unwrap();
        assert!(
            create(&state.auth, &admin, "Stale confirmation", "reports:read", 1)
                .await
                .is_err()
        );
        let fresh = state
            .auth
            .authenticate("admin", "Synthetic1-password")
            .await
            .unwrap()
            .unwrap()
            .0;
        let fresh_token = create(&state.auth, &fresh, "Fresh confirmation", "reports:read", 1)
            .await
            .unwrap();
        assert!(matches!(
            authenticate(&state.auth, &fresh_token, Capability::ReportsRead)
                .await
                .unwrap(),
            Authentication::Authorized(_)
        ));
    }
}
