//! Team membership, additive source grants, and persistent access-policy migration.
use crate::auth::{self, SecurityEvent, StateStore};
use anyhow::{Context, Result, ensure};
use sqlx::{Acquire, SqlitePool};

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS teams (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS team_members (
    team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    PRIMARY KEY(team_id,account_id)
);
CREATE INDEX IF NOT EXISTS idx_team_members_account ON team_members(account_id,team_id);
CREATE TABLE IF NOT EXISTS team_source_grants (
    team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    source_key TEXT NOT NULL,
    PRIMARY KEY(team_id,source_key)
);
CREATE TRIGGER IF NOT EXISTS accounts_keep_last_admin_update
BEFORE UPDATE OF role,enabled ON accounts
WHEN OLD.role='admin' AND OLD.enabled=1 AND (NEW.role<>'admin' OR NEW.enabled<>1)
AND NOT EXISTS(SELECT 1 FROM accounts WHERE role='admin' AND enabled=1 AND id<>OLD.id)
BEGIN SELECT RAISE(ABORT,'cannot remove the last enabled administrator'); END;
CREATE TRIGGER IF NOT EXISTS accounts_keep_last_admin_delete
BEFORE DELETE ON accounts
WHEN OLD.role='admin' AND OLD.enabled=1
AND NOT EXISTS(SELECT 1 FROM accounts WHERE role='admin' AND enabled=1 AND id<>OLD.id)
BEGIN SELECT RAISE(ABORT,'cannot remove the last enabled administrator'); END;
"#;

pub(crate) async fn migrate_roles(pool: &SqlitePool) -> Result<()> {
    let schema: String =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type='table' AND name='accounts'")
            .fetch_one(pool)
            .await
            .context("inspect account role vocabulary")?;
    if schema.contains("'auditor'") {
        return Ok(());
    }
    // SQLite cannot expand a CHECK constraint in place. Disable FK enforcement
    // only on this dedicated migration connection, outside the transaction, and
    // check every reference before commit. Other pool connections remain enabled.
    let mut connection = pool
        .acquire()
        .await
        .context("acquire role migration connection")?;
    // Never return this connection to the pool, including on cancellation.
    connection.close_on_drop();
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut *connection)
        .await
        .context("prepare account table replacement")?;
    let result: Result<()> = async {
        let mut transaction = connection.begin().await.context("begin account role migration")?;
        sqlx::raw_sql("CREATE TABLE accounts_v6 (
            id TEXT PRIMARY KEY,username TEXT NOT NULL COLLATE NOCASE UNIQUE,
            password_hash TEXT NOT NULL,enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
            role TEXT NOT NULL DEFAULT 'viewer' CHECK(role IN ('admin','viewer','auditor','operator')),
            session_generation INTEGER NOT NULL DEFAULT 0,display_name TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,password_changed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        ); INSERT INTO accounts_v6(id,username,password_hash,enabled,role,session_generation,display_name,created_at,password_changed_at)
        SELECT id,username,password_hash,enabled,role,session_generation,display_name,created_at,password_changed_at FROM accounts;
        DROP TABLE accounts; ALTER TABLE accounts_v6 RENAME TO accounts;")
            .execute(&mut *transaction).await.context("expand persistent account roles")?;
        let violation = sqlx::query("PRAGMA foreign_key_check").fetch_optional(&mut *transaction).await.context("validate account migration references")?;
        ensure!(violation.is_none(), "account migration violates a foreign key");
        transaction.commit().await.context("commit account role migration")
    }.await;
    let restored = sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&mut *connection)
        .await
        .context("restore migration foreign-key enforcement");
    if restored.is_err() {
        connection.close_on_drop();
    }
    restored?;
    result
}

#[derive(sqlx::FromRow)]
pub(crate) struct Team {
    pub(crate) id: String,
    pub(crate) name: String,
}

#[derive(sqlx::FromRow)]
pub(crate) struct GrantOrigin {
    pub(crate) source_key: String,
    pub(crate) origin: String,
    pub(crate) team_id: Option<String>,
}

pub(crate) fn name(value: &str) -> Result<&str> {
    ensure!(
        !value.chars().any(char::is_control),
        "team name must not contain control characters"
    );
    let value = value.trim();
    ensure!(
        !value.is_empty() && value.chars().count() <= 80 && !value.chars().any(char::is_control),
        "team name must contain 1–80 non-control characters"
    );
    Ok(value)
}

pub(crate) async fn create(state: &StateStore, requested_name: &str) -> Result<String> {
    let name = name(requested_name)?;
    let id = format!("team_{}", auth::random_token());
    let mut transaction = state
        .auth
        .pool
        .begin()
        .await
        .context("begin team creation")?;
    sqlx::query("INSERT INTO teams(id,name) VALUES(?,?)")
        .bind(&id)
        .bind(name)
        .execute(&mut *transaction)
        .await
        .context("create team")?;
    event(&mut transaction, None, &id, SecurityEvent::TeamCreated).await?;
    transaction.commit().await.context("commit team creation")?;
    Ok(id)
}

pub(crate) async fn rename(state: &StateStore, id: &str, requested_name: &str) -> Result<()> {
    let name = name(requested_name)?;
    let mut transaction = state.auth.pool.begin().await.context("begin team rename")?;
    let result = sqlx::query("UPDATE teams SET name=? WHERE id=?")
        .bind(name)
        .bind(id)
        .execute(&mut *transaction)
        .await
        .context("rename team")?;
    ensure!(result.rows_affected() == 1, "team not found");
    event(&mut transaction, None, id, SecurityEvent::TeamRenamed).await?;
    transaction.commit().await.context("commit team rename")
}

pub(crate) async fn delete(state: &StateStore, id: &str) -> Result<()> {
    let mut transaction = state
        .auth
        .pool
        .begin()
        .await
        .context("begin team deletion")?;
    let result = sqlx::query("DELETE FROM teams WHERE id=?")
        .bind(id)
        .execute(&mut *transaction)
        .await
        .context("delete team")?;
    ensure!(result.rows_affected() == 1, "team not found");
    event(&mut transaction, None, id, SecurityEvent::TeamDeleted).await?;
    transaction.commit().await.context("commit team deletion")
}

pub(crate) async fn membership(
    state: &StateStore,
    id: &str,
    username: &str,
    add: bool,
) -> Result<()> {
    let account_id = auth::account_id(state, username).await?;
    let mut transaction = state
        .auth
        .pool
        .begin()
        .await
        .context("begin team membership change")?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM teams WHERE id=?)")
        .bind(id)
        .fetch_one(&mut *transaction)
        .await
        .context("resolve membership team")?;
    ensure!(exists, "team not found");
    let sql = if add {
        "INSERT OR IGNORE INTO team_members(team_id,account_id) VALUES(?,?)"
    } else {
        "DELETE FROM team_members WHERE team_id=? AND account_id=?"
    };
    let result = sqlx::query(sql)
        .bind(id)
        .bind(&account_id)
        .execute(&mut *transaction)
        .await
        .context("change team membership")?;
    if result.rows_affected() > 0 {
        event(
            &mut transaction,
            Some(&account_id),
            id,
            if add {
                SecurityEvent::TeamMemberAdded
            } else {
                SecurityEvent::TeamMemberRemoved
            },
        )
        .await?;
    }
    transaction
        .commit()
        .await
        .context("commit team membership change")
}

pub(crate) async fn grant(state: &StateStore, id: &str, source_key: &str, add: bool) -> Result<()> {
    auth::validate_source_key(source_key)?;
    let mut transaction = state
        .auth
        .pool
        .begin()
        .await
        .context("begin team source grant change")?;
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM teams WHERE id=?)")
        .bind(id)
        .fetch_one(&mut *transaction)
        .await
        .context("resolve source grant team")?;
    ensure!(exists, "team not found");
    let sql = if add {
        "INSERT OR IGNORE INTO team_source_grants(team_id,source_key) VALUES(?,?)"
    } else {
        "DELETE FROM team_source_grants WHERE team_id=? AND source_key=?"
    };
    let result = sqlx::query(sql)
        .bind(id)
        .bind(source_key)
        .execute(&mut *transaction)
        .await
        .context("change team source grant")?;
    if result.rows_affected() > 0 {
        event(
            &mut transaction,
            None,
            id,
            if add {
                SecurityEvent::TeamSourceGranted
            } else {
                SecurityEvent::TeamSourceRevoked
            },
        )
        .await?;
    }
    transaction
        .commit()
        .await
        .context("commit team source grant change")
}

pub(crate) async fn list(pool: &SqlitePool, offset: i64, limit: i64) -> Result<Vec<Team>> {
    sqlx::query_as("SELECT id,name FROM teams ORDER BY name COLLATE NOCASE,id LIMIT ? OFFSET ?")
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await
        .context("list teams")
}

pub(crate) async fn inspect(
    state: &StateStore,
    id: &str,
) -> Result<(Team, Vec<String>, Vec<String>)> {
    let team = sqlx::query_as("SELECT id,name FROM teams WHERE id=?")
        .bind(id)
        .fetch_optional(&state.auth.pool)
        .await
        .context("inspect team")?
        .context("team not found")?;
    let members=sqlx::query_scalar("SELECT a.username FROM team_members m JOIN accounts a ON a.id=m.account_id WHERE m.team_id=? ORDER BY a.username COLLATE NOCASE")
        .bind(id).fetch_all(&state.auth.pool).await.context("list team members")?;
    let sources = sqlx::query_scalar(
        "SELECT source_key FROM team_source_grants WHERE team_id=? ORDER BY source_key",
    )
    .bind(id)
    .fetch_all(&state.auth.pool)
    .await
    .context("list team sources")?;
    Ok((team, members, sources))
}

pub(crate) async fn grant_origins(
    pool: &SqlitePool,
    account_id: &str,
    offset: i64,
    limit: Option<i64>,
) -> Result<Vec<GrantOrigin>> {
    sqlx::query_as("SELECT source_key,'Direct grant' AS origin,NULL AS team_id FROM account_source_grants WHERE account_id=? UNION ALL SELECT g.source_key,t.name AS origin,t.id AS team_id FROM team_source_grants g JOIN teams t ON t.id=g.team_id JOIN team_members m ON m.team_id=t.id WHERE m.account_id=? ORDER BY source_key,team_id LIMIT ? OFFSET ?")
        .bind(account_id).bind(account_id).bind(limit.unwrap_or(-1)).bind(offset).fetch_all(pool).await.context("explain effective source access")
}

pub(crate) async fn event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    account_id: Option<&str>,
    team_id: &str,
    event: SecurityEvent,
) -> Result<()> {
    sqlx::query("DELETE FROM security_events WHERE occurred_at<unixepoch()-?")
        .bind(auth::SECURITY_RETENTION_DAYS * 86_400)
        .execute(&mut **transaction)
        .await
        .context("expire retained team security events")?;
    sqlx::query("INSERT INTO security_events(account_id,team_id,event_type) VALUES(?,?,?)")
        .bind(account_id)
        .bind(team_id)
        .bind(event.as_str())
        .execute(&mut **transaction)
        .await
        .context("record team access event")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AccountRole;

    async fn fixture() -> (tempfile::TempDir, StateStore, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let state = StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        let password = dir.path().join("password");
        std::fs::write(&password, "Synthetic1-password").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        auth::create_account(&state, "admin", &password, None)
            .await
            .unwrap();
        auth::create_account(&state, "viewer", &password, None)
            .await
            .unwrap();
        (dir, state, password)
    }

    #[tokio::test]
    async fn team_union_provenance_rename_and_removal_preserve_independent_grants() {
        let (_dir, state, _password) = fixture().await;
        let a = format!("src_{}", "a".repeat(64));
        let b = format!("src_{}", "b".repeat(64));
        let one = create(&state, " First team ").await.unwrap();
        let two = create(&state, "Second team").await.unwrap();
        assert!(one.starts_with("team_"));
        assert!(create(&state, "first TEAM").await.is_err());
        assert!(create(&state, "invalid\nname").await.is_err());
        assert!(grant(&state, &one, "invalid-source", true).await.is_err());
        membership(&state, &one, "viewer", true).await.unwrap();
        membership(&state, &two, "viewer", true).await.unwrap();
        grant(&state, &one, &a, true).await.unwrap();
        grant(&state, &two, &a, true).await.unwrap();
        grant(&state, &two, &b, true).await.unwrap();
        auth::grant_source(&state, "viewer", &a).await.unwrap();
        let permissions = auth::account_permissions(&state, "viewer").await.unwrap();
        assert_eq!(permissions.source_keys, vec![a.clone(), b.clone()]);
        assert_eq!(permissions.origins.len(), 4);
        assert_eq!(
            permissions
                .origins
                .iter()
                .filter(|origin| origin.team_id.is_none())
                .count(),
            1
        );
        rename(&state, &one, "Renamed team").await.unwrap();
        assert_eq!(inspect(&state, &one).await.unwrap().0.id, one);
        assert_eq!(inspect(&state, &one).await.unwrap().0.name, "Renamed team");
        assert_eq!(
            auth::account_permissions(&state, "admin")
                .await
                .unwrap()
                .source_keys
                .len(),
            0
        );
        auth::revoke_source(&state, "viewer", &a).await.unwrap();
        membership(&state, &one, "viewer", false).await.unwrap();
        assert_eq!(
            auth::account_permissions(&state, "viewer")
                .await
                .unwrap()
                .source_keys,
            vec![a.clone(), b.clone()]
        );
        grant(&state, &two, &a, false).await.unwrap();
        assert_eq!(
            auth::account_permissions(&state, "viewer")
                .await
                .unwrap()
                .source_keys,
            vec![b]
        );
        auth::grant_source(&state, "viewer", &a).await.unwrap();
        delete(&state, &two).await.unwrap();
        assert_eq!(
            auth::account_permissions(&state, "viewer")
                .await
                .unwrap()
                .source_keys,
            vec![a]
        );
        assert!(inspect(&state, &two).await.is_err());
        let remaining: i64 =
            sqlx::query_scalar("SELECT count(*) FROM team_members WHERE team_id=?")
                .bind(&two)
                .fetch_one(&state.auth.pool)
                .await
                .unwrap();
        assert_eq!(remaining, 0);
        let retained: Option<String> = sqlx::query_scalar(
            "SELECT team_id FROM security_events WHERE event_type='team_deleted' AND team_id=?",
        )
        .bind(&two)
        .fetch_optional(&state.auth.pool)
        .await
        .unwrap();
        assert_eq!(retained, Some(two));
    }

    #[tokio::test]
    async fn last_admin_guards_are_atomic_and_cannot_be_bypassed_through_raw_updates() {
        let (_dir, state, password) = fixture().await;
        assert!(
            auth::set_role(&state, "admin", AccountRole::Auditor)
                .await
                .is_err()
        );
        assert!(auth::set_enabled(&state, "admin", false).await.is_err());
        assert!(
            sqlx::query("DELETE FROM accounts WHERE username='admin'")
                .execute(&state.auth.pool)
                .await
                .is_err()
        );
        assert!(
            sqlx::query("UPDATE accounts SET role='operator' WHERE username='admin'")
                .execute(&state.auth.pool)
                .await
                .is_err()
        );
        auth::set_role(&state, "viewer", AccountRole::Admin)
            .await
            .unwrap();
        let (first, second) = tokio::join!(
            auth::set_enabled(&state, "admin", false),
            auth::set_role(&state, "viewer", AccountRole::Auditor)
        );
        assert_ne!(
            first.is_ok(),
            second.is_ok(),
            "exactly one concurrent removal may succeed"
        );
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM accounts WHERE role='admin' AND enabled=1")
                .fetch_one(&state.auth.pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        auth::create_account(&state, "recovery", &password, Some(AccountRole::Admin))
            .await
            .unwrap();
        auth::set_role(&state, "admin", AccountRole::Auditor)
            .await
            .unwrap();
        auth::set_enabled(&state, "admin", true).await.unwrap();
        assert_eq!(
            auth::account_permissions(&state, "admin")
                .await
                .unwrap()
                .role,
            "auditor"
        );
    }

    async fn downgrade_account_fixture(path: &std::path::Path) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(path)
                    .foreign_keys(false),
            )
            .await
            .unwrap();
        sqlx::raw_sql("DROP TRIGGER accounts_keep_last_admin_update; DROP TRIGGER accounts_keep_last_admin_delete;
            CREATE TABLE accounts_v5(id TEXT PRIMARY KEY,username TEXT NOT NULL COLLATE NOCASE UNIQUE,password_hash TEXT NOT NULL,enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),role TEXT NOT NULL DEFAULT 'viewer' CHECK(role IN ('admin','viewer')),session_generation INTEGER NOT NULL DEFAULT 0,display_name TEXT,created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,password_changed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
            INSERT INTO accounts_v5 SELECT * FROM accounts; DROP TABLE accounts; ALTER TABLE accounts_v5 RENAME TO accounts;
            UPDATE app_state_metadata SET value='5' WHERE key='schema_version';")
            .execute(&pool).await.unwrap();
        pool.close().await;
    }

    #[tokio::test]
    async fn role_migration_preserves_sessions_preferences_tokens_and_foreign_keys() {
        use tower_sessions::{Session, SessionStore};
        let (dir, state, _password) = fixture().await;
        let (user, generation) = state
            .auth
            .authenticate("admin", "Synthetic1-password")
            .await
            .unwrap()
            .unwrap();
        auth::update_preferences(
            &state.auth,
            &user.id,
            &crate::preferences::UserPreferences {
                page_size: 25,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let token =
            crate::tokens::create(&state.auth, &user, "Synthetic migration", "reports:read", 1)
                .await
                .unwrap();
        let session = Session::new(None, std::sync::Arc::new(state.sessions.clone()), None);
        auth::establish_session(&state.auth, &session, &user, generation, None)
            .await
            .unwrap();
        let session_id = session.id().unwrap();
        let source = format!("src_{}", "c".repeat(64));
        auth::grant_source(&state, "viewer", &source).await.unwrap();
        state.auth.pool.close().await;
        let path = dir.path().join("state.sqlite");
        downgrade_account_fixture(&path).await;
        let state = StateStore::open(&path).await.unwrap();
        assert!(state.sessions.load(&session_id).await.unwrap().is_some());
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
        let user = state
            .auth
            .authenticate("admin", "Synthetic1-password")
            .await
            .unwrap()
            .unwrap()
            .0;
        assert_eq!(user.preferences.page_size, 25);
        assert_eq!(
            auth::account_permissions(&state, "viewer")
                .await
                .unwrap()
                .source_keys,
            vec![source]
        );
        auth::set_role(&state, "viewer", AccountRole::Operator)
            .await
            .unwrap();
        assert!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&state.auth.pool)
                .await
                .unwrap()
                .is_empty()
        );
        let mut connections = Vec::new();
        for _ in 0..5 {
            let mut connection = state.auth.pool.acquire().await.unwrap();
            let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&mut *connection)
                .await
                .unwrap();
            assert_eq!(enabled, 1);
            connections.push(connection);
        }
    }

    #[tokio::test]
    async fn invalid_legacy_references_roll_back_role_migration() {
        let (dir, state, _) = fixture().await;
        state.auth.pool.close().await;
        let path = dir.path().join("state.sqlite");
        downgrade_account_fixture(&path).await;
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&path)
                    .foreign_keys(false),
            )
            .await
            .unwrap();
        sqlx::query("INSERT INTO account_source_grants(account_id,source_key) VALUES('missing-account','synthetic-orphan')").execute(&pool).await.unwrap();
        pool.close().await;
        assert!(StateStore::open(&path).await.is_err());
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
        let schema: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='accounts'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            !schema.contains("'auditor'"),
            "failed migration must restore the original account table"
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
        let staged: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='accounts_v6')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(!staged);
    }
}
