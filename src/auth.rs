use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use argon2::{Algorithm, Argon2, Params, Version};
use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
    rand_core::{OsRng, RngCore},
};
use serde::Deserialize;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use subtle::ConstantTimeEq;
use tower_sessions::{Session, session_store::ExpiredDeletion};
use tower_sessions_sqlx_store::SqliteStore;

pub const INACTIVITY_MINUTES: i64 = 30;
pub const ABSOLUTE_HOURS: i64 = 12;
pub const MIN_PASSWORD_CHARS: usize = 7;
const MAX_PASSWORD_BYTES: usize = 1_024;
const ACCOUNT_ID: &str = "auth.account_id";
const SESSION_GENERATION: &str = "auth.generation";
const ISSUED_AT: &str = "auth.issued_at";
const CSRF_TOKEN: &str = "auth.csrf";
const INVENTORY_ID: &str = "auth.inventory_id";
pub const SECURITY_RETENTION_DAYS: i64 = 180;
pub const SESSION_HISTORY_RETENTION_DAYS: i64 = 30;

const STATE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS app_state_metadata (key TEXT PRIMARY KEY,value TEXT NOT NULL);
INSERT OR IGNORE INTO app_state_metadata(key,value) VALUES('schema_version','5');
CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL COLLATE NOCASE UNIQUE,
    password_hash TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
    role TEXT NOT NULL DEFAULT 'viewer' CHECK(role IN ('admin','viewer')),
    session_generation INTEGER NOT NULL DEFAULT 0,
    display_name TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    password_changed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS account_source_grants (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    source_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(account_id,source_key)
);
CREATE TABLE IF NOT EXISTS account_preferences (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    timezone_mode TEXT NOT NULL DEFAULT 'utc',
    timezone_name TEXT NOT NULL DEFAULT 'UTC',
    date_time_format TEXT NOT NULL DEFAULT 'iso',
    appearance TEXT NOT NULL DEFAULT 'system',
    font_scale INTEGER NOT NULL DEFAULT 100,
    page_size INTEGER NOT NULL DEFAULT 50,
    live_interval_seconds INTEGER NOT NULL DEFAULT 5,
    live_paused INTEGER NOT NULL DEFAULT 1,
    transcript_detail TEXT NOT NULL DEFAULT 'standard',
    code_wrap INTEGER NOT NULL DEFAULT 0,
    default_sessions_view TEXT NOT NULL DEFAULT 'all',
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS account_sessions (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    tower_session_id TEXT NOT NULL UNIQUE,
    session_generation INTEGER NOT NULL,
    logged_in_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    absolute_expires_at INTEGER NOT NULL,
    user_agent_summary TEXT NOT NULL,
    revoked_at INTEGER
);
CREATE INDEX IF NOT EXISTS idx_account_sessions_account_activity ON account_sessions(account_id,last_seen_at DESC,id DESC);
CREATE TABLE IF NOT EXISTS security_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id TEXT REFERENCES accounts(id),
    event_type TEXT NOT NULL CHECK(event_type IN ('login_succeeded','login_failed','password_changed','password_reset','session_revoked','sessions_revoked','account_created','account_enabled','account_disabled','role_changed','source_granted','source_revoked','profile_changed','token_created','token_revoked')),
    occurred_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_security_events_account_time ON security_events(account_id,occurred_at DESC,id DESC);
CREATE TRIGGER IF NOT EXISTS security_events_no_update BEFORE UPDATE ON security_events BEGIN SELECT RAISE(ABORT,'security events are append-only'); END;
"#;

#[derive(Clone)]
pub struct AuthState {
    pub(crate) pool: SqlitePool,
    local_mode: bool,
    dummy_hash: String,
    absolute_lifetime_seconds: i64,
    token_attempts: crate::tokens::AttemptBudget,
    #[cfg(test)]
    test_digest: Option<[u8; 32]>,
}

#[derive(Clone)]
pub struct StateStore {
    pub auth: AuthState,
    pub sessions: SqliteStore,
}

#[derive(Clone)]
pub struct CurrentUser {
    pub id: String,
    pub(crate) credential_generation: i64,
    pub username: String,
    pub display_name: Option<String>,
    pub password_changed_at: String,
    pub access: AccessScope,
    pub(crate) preferences: crate::preferences::UserPreferences,
}

#[derive(Clone, Debug)]
pub struct AccessScope {
    role: AccountRole,
    capabilities: crate::authorization::Capabilities,
    source_keys_json: String,
    source_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccountRole {
    Admin,
    Viewer,
}

impl AccountRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Viewer => "viewer",
        }
    }
}

impl AccessScope {
    pub(crate) fn constrain(&mut self, ceiling: &[crate::authorization::Capability]) {
        self.capabilities = self.capabilities.constrained(ceiling);
    }
    pub(crate) fn permits(&self, capability: crate::authorization::Capability) -> bool {
        self.capabilities.permits(capability)
    }

    pub fn role(&self) -> &'static str {
        self.role.as_str()
    }

    pub fn source_count(&self) -> usize {
        self.source_count
    }

    pub(crate) fn unrestricted(&self) -> bool {
        self.can_read_sources() && self.permits(crate::authorization::Capability::AllSourcesRead)
    }

    pub(crate) fn source_keys_json(&self) -> &str {
        if self.can_read_sources() {
            &self.source_keys_json
        } else {
            "[]"
        }
    }

    fn can_read_sources(&self) -> bool {
        self.permits(crate::authorization::Capability::ContentRead)
            || self.permits(crate::authorization::Capability::ReportsRead)
    }

    fn permits_route(&self, method: &axum::http::Method, path: &str) -> bool {
        crate::authorization::required_capability(method, path)
            .is_some_and(|capability| self.permits(capability))
    }

    pub(crate) fn all() -> Self {
        Self {
            role: AccountRole::Admin,
            capabilities: crate::authorization::Capabilities::for_role(true),
            source_keys_json: "[]".to_owned(),
            source_count: 0,
        }
    }
}

#[derive(sqlx::FromRow)]
struct AccountRow {
    id: String,
    username: String,
    password_hash: String,
    enabled: bool,
    role: String,
    session_generation: i64,
    display_name: Option<String>,
    password_changed_at: String,
}

pub struct AccountPermissions {
    pub username: String,
    pub role: &'static str,
    pub source_keys: Vec<String>,
}

#[derive(Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
    pub csrf_token: String,
    #[serde(default)]
    pub return_to: String,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ChangePasswordResult {
    Changed { generation: i64 },
    InvalidCurrentPassword,
    InvalidNewPassword(PasswordPolicyError),
}

#[derive(Clone, Copy)]
pub(crate) enum SecurityEvent {
    LoginSucceeded,
    LoginFailed,
    PasswordChanged,
    PasswordReset,
    SessionRevoked,
    SessionsRevoked,
    AccountCreated,
    AccountEnabled,
    AccountDisabled,
    RoleChanged,
    SourceGranted,
    SourceRevoked,
    ProfileChanged,
    TokenCreated,
    TokenRevoked,
}

impl SecurityEvent {
    fn as_str(self) -> &'static str {
        match self {
            Self::LoginSucceeded => "login_succeeded",
            Self::LoginFailed => "login_failed",
            Self::PasswordChanged => "password_changed",
            Self::PasswordReset => "password_reset",
            Self::SessionRevoked => "session_revoked",
            Self::SessionsRevoked => "sessions_revoked",
            Self::AccountCreated => "account_created",
            Self::AccountEnabled => "account_enabled",
            Self::AccountDisabled => "account_disabled",
            Self::RoleChanged => "role_changed",
            Self::SourceGranted => "source_granted",
            Self::SourceRevoked => "source_revoked",
            Self::ProfileChanged => "profile_changed",
            Self::TokenCreated => "token_created",
            Self::TokenRevoked => "token_revoked",
        }
    }
}

#[derive(Clone)]
pub(crate) struct AccountSession {
    pub(crate) id: String,
    pub(crate) logged_in_at: String,
    pub(crate) last_seen_at: String,
    pub(crate) status: &'static str,
    pub(crate) user_agent_summary: String,
    pub(crate) current: bool,
}

#[derive(Clone)]
pub(crate) struct SecurityEventRow {
    pub(crate) occurred_at: String,
    pub(crate) label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PasswordPolicyError {
    TooShort,
    TooLong,
    ControlCharacter,
    MissingUppercase,
    MissingLowercase,
    MissingNumber,
    MissingSymbol,
}

impl std::fmt::Display for PasswordPolicyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TooShort => "password must contain at least 7 characters",
            Self::TooLong => "password is too long",
            Self::ControlCharacter => "password must not contain control characters",
            Self::MissingUppercase => "password must contain an uppercase character",
            Self::MissingLowercase => "password must contain a lowercase character",
            Self::MissingNumber => "password must contain a numeric character",
            Self::MissingSymbol => "password must contain a symbol character",
        })
    }
}

impl std::error::Error for PasswordPolicyError {}

impl StateStore {
    pub fn enable_local_mode(&mut self) {
        self.auth.local_mode = true;
    }

    pub async fn open(path: &Path) -> Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .with_context(|| format!("open persistent application state {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .context("restrict persistent application state permissions")?;
        }
        let has_metadata: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_state_metadata')")
            .fetch_one(&pool)
            .await
            .context("inspect application state schema")?;
        if has_metadata {
            let schema_version: String = sqlx::query_scalar(
                "SELECT value FROM app_state_metadata WHERE key='schema_version'",
            )
            .fetch_one(&pool)
            .await
            .context("read application state schema version")?;
            let schema_version = schema_version
                .parse::<u32>()
                .context("invalid application state schema version")?;
            ensure!(
                schema_version <= 5,
                "application state schema is newer than this Agent Lens binary"
            );
        }
        sqlx::raw_sql(STATE_SCHEMA)
            .execute(&pool)
            .await
            .context("migrate persistent application state")?;
        let account_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('accounts')")
                .fetch_all(&pool)
                .await
                .context("inspect application account schema")?;
        if !account_columns.iter().any(|column| column == "role") {
            sqlx::query("ALTER TABLE accounts ADD COLUMN role TEXT NOT NULL DEFAULT 'admin' CHECK(role IN ('admin','viewer'))")
                .execute(&pool)
                .await
                .context("add application account roles")?;
        }
        if !account_columns
            .iter()
            .any(|column| column == "display_name")
        {
            sqlx::query("ALTER TABLE accounts ADD COLUMN display_name TEXT")
                .execute(&pool)
                .await
                .context("add application account display names")?;
        }
        migrate_token_events(&pool).await?;
        sqlx::raw_sql(crate::tokens::SCHEMA)
            .execute(&pool)
            .await
            .context("migrate token storage")?;
        let mut transaction = pool.begin().await.context("begin token retention")?;
        crate::tokens::prune(&mut transaction).await?;
        transaction
            .commit()
            .await
            .context("commit token retention")?;
        sqlx::query("UPDATE app_state_metadata SET value='5' WHERE key='schema_version'")
            .execute(&pool)
            .await
            .context("record application state schema version")?;
        let sessions = SqliteStore::new(pool.clone());
        sessions
            .migrate()
            .await
            .context("migrate persistent session store")?;
        sessions
            .delete_expired()
            .await
            .context("delete expired application sessions")?;
        sqlx::query("DELETE FROM account_sessions WHERE (revoked_at IS NOT NULL OR absolute_expires_at<=unixepoch() OR last_seen_at<unixepoch()-?) AND last_seen_at<unixepoch()-?")
            .bind(INACTIVITY_MINUTES * 60)
            .bind(SESSION_HISTORY_RETENTION_DAYS * 86_400)
            .execute(&pool)
            .await
            .context("expire retained application session history")?;
        sqlx::query("DELETE FROM security_events WHERE occurred_at<unixepoch()-?")
            .bind(SECURITY_RETENTION_DAYS * 86_400)
            .execute(&pool)
            .await
            .context("expire retained security events")?;
        let dummy_hash = hash_password("synthetic-dummy-password")?;
        Ok(Self {
            auth: AuthState {
                pool,
                local_mode: false,
                dummy_hash,
                absolute_lifetime_seconds: ABSOLUTE_HOURS * 60 * 60,
                token_attempts: crate::tokens::AttemptBudget::default(),
                #[cfg(test)]
                test_digest: None,
            },
            sessions,
        })
    }

    pub async fn require_active_account(&self) -> Result<()> {
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts WHERE enabled=1")
            .fetch_one(&self.auth.pool)
            .await
            .context("check active application accounts")?;
        ensure!(
            count > 0,
            "no active application account; create one with `agent-lens account create`"
        );
        Ok(())
    }
}

async fn migrate_token_events(pool: &SqlitePool) -> Result<()> {
    let schema: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='security_events'",
    )
    .fetch_one(pool)
    .await
    .context("inspect security event vocabulary")?;
    if schema.contains("'token_created'") {
        return Ok(());
    }
    let mut transaction = pool
        .begin()
        .await
        .context("begin security event migration")?;
    sqlx::raw_sql("DROP TRIGGER security_events_no_update; DROP INDEX idx_security_events_account_time; ALTER TABLE security_events RENAME TO security_events_v4;")
        .execute(&mut *transaction).await.context("prepare security event migration")?;
    sqlx::raw_sql(STATE_SCHEMA)
        .execute(&mut *transaction)
        .await
        .context("create updated security event vocabulary")?;
    sqlx::raw_sql("INSERT INTO security_events(id,account_id,event_type,occurred_at) SELECT id,account_id,event_type,occurred_at FROM security_events_v4; DROP TABLE security_events_v4;")
        .execute(&mut *transaction).await.context("preserve security event history")?;
    transaction
        .commit()
        .await
        .context("commit security event migration")
}

pub async fn create_account(
    state: &StateStore,
    username: &str,
    password_file: &Path,
    requested_role: Option<AccountRole>,
) -> Result<()> {
    let username = validate_username(username)?;
    let password = read_password(password_file)?;
    let hash = hash_password(&password)?;
    let mut transaction = state
        .auth
        .pool
        .begin()
        .await
        .context("begin account creation")?;
    let account_count: i64 = sqlx::query_scalar("SELECT count(*) FROM accounts")
        .fetch_one(&mut *transaction)
        .await
        .context("count application accounts")?;
    let role = requested_role.unwrap_or(if account_count == 0 {
        AccountRole::Admin
    } else {
        AccountRole::Viewer
    });
    let account_id = random_token();
    sqlx::query("INSERT INTO accounts(id,username,password_hash,role) VALUES(?,?,?,?)")
        .bind(&account_id)
        .bind(username)
        .bind(hash)
        .bind(role.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_account_write_error("create application account", error))?;
    insert_security_event(
        &mut transaction,
        Some(&account_id),
        SecurityEvent::AccountCreated,
    )
    .await?;
    transaction
        .commit()
        .await
        .context("commit account creation")?;
    Ok(())
}

pub async fn set_role(state: &StateStore, username: &str, role: AccountRole) -> Result<()> {
    let username = validate_username(username)?;
    let mut transaction = state.auth.pool.begin().await.context("begin role change")?;
    let account_id: Option<String> = sqlx::query_scalar(
        "UPDATE accounts SET role=? WHERE username=? COLLATE NOCASE RETURNING id",
    )
    .bind(role.as_str())
    .bind(username)
    .fetch_optional(&mut *transaction)
    .await
    .context("change application account role")?;
    let account_id = account_id.context("application account not found")?;
    insert_security_event(
        &mut transaction,
        Some(&account_id),
        SecurityEvent::RoleChanged,
    )
    .await?;
    transaction.commit().await.context("commit role change")?;
    Ok(())
}

pub async fn grant_source(state: &StateStore, username: &str, source_key: &str) -> Result<()> {
    let account_id = account_id(state, username).await?;
    validate_source_key(source_key)?;
    let mut transaction = state
        .auth
        .pool
        .begin()
        .await
        .context("begin source grant")?;
    let result = sqlx::query(
        "INSERT OR IGNORE INTO account_source_grants(account_id,source_key) VALUES(?,?)",
    )
    .bind(&account_id)
    .bind(source_key)
    .execute(&mut *transaction)
    .await
    .context("grant source access")?;
    if result.rows_affected() == 1 {
        insert_security_event(
            &mut transaction,
            Some(&account_id),
            SecurityEvent::SourceGranted,
        )
        .await?;
    }
    transaction.commit().await.context("commit source grant")?;
    Ok(())
}

pub async fn revoke_source(state: &StateStore, username: &str, source_key: &str) -> Result<()> {
    let account_id = account_id(state, username).await?;
    validate_source_key(source_key)?;
    let mut transaction = state
        .auth
        .pool
        .begin()
        .await
        .context("begin source revocation")?;
    let result =
        sqlx::query("DELETE FROM account_source_grants WHERE account_id=? AND source_key=?")
            .bind(&account_id)
            .bind(source_key)
            .execute(&mut *transaction)
            .await
            .context("revoke source access")?;
    if result.rows_affected() == 1 {
        insert_security_event(
            &mut transaction,
            Some(&account_id),
            SecurityEvent::SourceRevoked,
        )
        .await?;
    }
    transaction
        .commit()
        .await
        .context("commit source revocation")?;
    Ok(())
}

pub async fn account_permissions(state: &StateStore, username: &str) -> Result<AccountPermissions> {
    let username = validate_username(username)?;
    let account: Option<(String, String, String)> =
        sqlx::query_as("SELECT id,username,role FROM accounts WHERE username=? COLLATE NOCASE")
            .bind(username)
            .fetch_optional(&state.auth.pool)
            .await
            .context("load application account permissions")?;
    let Some((id, username, role)) = account else {
        anyhow::bail!("application account not found");
    };
    let role = parse_role(&role)?;
    let source_keys = source_grants(&state.auth.pool, &id).await?;
    Ok(AccountPermissions {
        username,
        role: role.as_str(),
        source_keys,
    })
}

pub async fn reset_password(
    state: &StateStore,
    username: &str,
    password_file: &Path,
) -> Result<()> {
    let username = validate_username(username)?;
    let password = read_password(password_file)?;
    let hash = hash_password(&password)?;
    let mut transaction = state
        .auth
        .pool
        .begin()
        .await
        .context("begin password reset")?;
    let account_id: Option<String> = sqlx::query_scalar("UPDATE accounts SET password_hash=?,password_changed_at=CURRENT_TIMESTAMP,session_generation=session_generation+1 WHERE username=? COLLATE NOCASE RETURNING id")
        .bind(hash).bind(username).fetch_optional(&mut *transaction).await.context("reset application account password")?;
    let account_id = account_id.context("application account not found")?;
    revoke_inventory_in_transaction(&mut transaction, &account_id).await?;
    insert_security_event(
        &mut transaction,
        Some(&account_id),
        SecurityEvent::PasswordReset,
    )
    .await?;
    transaction
        .commit()
        .await
        .context("commit password reset")?;
    Ok(())
}

pub async fn change_password(
    state: &AuthState,
    account_id: &str,
    current_password: &str,
    new_password: &str,
) -> Result<ChangePasswordResult> {
    if current_password.len() > MAX_PASSWORD_BYTES {
        return Ok(ChangePasswordResult::InvalidCurrentPassword);
    }
    let Some(current_hash) = sqlx::query_scalar::<_, String>(
        "SELECT password_hash FROM accounts WHERE id=? AND enabled=1",
    )
    .bind(account_id)
    .fetch_optional(&state.pool)
    .await
    .context("load account for password change")?
    else {
        return Ok(ChangePasswordResult::InvalidCurrentPassword);
    };
    let verification_hash = current_hash.clone();
    let supplied_password = current_password.to_owned();
    let current_matches = tokio::task::spawn_blocking(move || {
        verify_password(&verification_hash, &supplied_password)
    })
    .await
    .context("join current password verification")?;
    if !current_matches {
        return Ok(ChangePasswordResult::InvalidCurrentPassword);
    }
    if let Err(error) = validate_password(new_password) {
        return Ok(ChangePasswordResult::InvalidNewPassword(error));
    }
    let new_password = new_password.to_owned();
    let new_hash = tokio::task::spawn_blocking(move || hash_password(&new_password))
        .await
        .context("join password hashing")??;
    let mut transaction = state.pool.begin().await.context("begin password change")?;
    let generation = sqlx::query_scalar::<_, i64>(
        "UPDATE accounts SET password_hash=?,password_changed_at=CURRENT_TIMESTAMP,session_generation=session_generation+1 WHERE id=? AND password_hash=? AND enabled=1 RETURNING session_generation",
    )
    .bind(new_hash)
    .bind(account_id)
    .bind(current_hash)
    .fetch_optional(&mut *transaction)
    .await
    .context("change application account password")?;
    if generation.is_some() {
        insert_security_event(
            &mut transaction,
            Some(account_id),
            SecurityEvent::PasswordChanged,
        )
        .await?;
    }
    transaction
        .commit()
        .await
        .context("commit password change")?;
    Ok(
        generation.map_or(ChangePasswordResult::InvalidCurrentPassword, |generation| {
            ChangePasswordResult::Changed { generation }
        }),
    )
}

pub async fn update_preferences(
    state: &AuthState,
    account_id: &str,
    preferences: &crate::preferences::UserPreferences,
) -> Result<()> {
    preferences
        .validate()
        .map_err(|message| anyhow::anyhow!(message))?;
    let result = sqlx::query(
        "INSERT INTO account_preferences(account_id,timezone_mode,timezone_name,date_time_format,appearance,font_scale,page_size,live_interval_seconds,live_paused,transcript_detail,code_wrap,default_sessions_view,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,CURRENT_TIMESTAMP) ON CONFLICT(account_id) DO UPDATE SET timezone_mode=excluded.timezone_mode,timezone_name=excluded.timezone_name,date_time_format=excluded.date_time_format,appearance=excluded.appearance,font_scale=excluded.font_scale,page_size=excluded.page_size,live_interval_seconds=excluded.live_interval_seconds,live_paused=excluded.live_paused,transcript_detail=excluded.transcript_detail,code_wrap=excluded.code_wrap,default_sessions_view=excluded.default_sessions_view,updated_at=CURRENT_TIMESTAMP",
    )
    .bind(account_id)
    .bind(&preferences.timezone_mode)
    .bind(&preferences.timezone_name)
    .bind(&preferences.date_time_format)
    .bind(&preferences.appearance)
    .bind(i64::from(preferences.font_scale))
    .bind(preferences.page_size as i64)
    .bind(preferences.live_interval_seconds as i64)
    .bind(preferences.live_paused)
    .bind(&preferences.transcript_detail)
    .bind(preferences.code_wrap)
    .bind(&preferences.default_sessions_view)
    .execute(&state.pool)
    .await
    .context("save account preferences")?;
    ensure!(result.rows_affected() == 1, "application account not found");
    Ok(())
}

pub async fn update_profile(state: &AuthState, account_id: &str, display_name: &str) -> Result<()> {
    let display_name = display_name.trim();
    ensure!(
        display_name.chars().count() <= 80 && !display_name.chars().any(char::is_control),
        "display name must be at most 80 characters without control characters"
    );
    let value = (!display_name.is_empty()).then_some(display_name);
    let mut transaction = state.pool.begin().await.context("begin profile update")?;
    let result = sqlx::query("UPDATE accounts SET display_name=? WHERE id=?")
        .bind(value)
        .bind(account_id)
        .execute(&mut *transaction)
        .await
        .context("update account profile")?;
    ensure!(result.rows_affected() == 1, "application account not found");
    insert_security_event(
        &mut transaction,
        Some(account_id),
        SecurityEvent::ProfileChanged,
    )
    .await?;
    transaction
        .commit()
        .await
        .context("commit profile update")?;
    Ok(())
}

pub async fn record_failed_login(state: &AuthState, username: &str) -> Result<()> {
    let account_id: Option<String> = if validate_username(username).is_ok() {
        sqlx::query_scalar::<_, String>("SELECT id FROM accounts WHERE username=? COLLATE NOCASE")
            .bind(username)
            .fetch_optional(&state.pool)
            .await
            .context("resolve failed login account")?
    } else {
        None
    };
    insert_security_event_pool(state, account_id.as_deref(), SecurityEvent::LoginFailed).await
}

pub(crate) async fn account_sessions(
    state: &AuthState,
    account_id: &str,
    current_id: Option<&str>,
    offset: i64,
    limit: usize,
) -> Result<Vec<AccountSession>> {
    type Row = (String, i64, i64, i64, Option<i64>, String);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id,logged_in_at,last_seen_at,absolute_expires_at,revoked_at,user_agent_summary FROM account_sessions WHERE account_id=? ORDER BY last_seen_at DESC,id DESC LIMIT ? OFFSET ?",
    )
    .bind(account_id)
    .bind(limit as i64)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    .context("list account sessions")?;
    let now = unix_now();
    Ok(rows
        .into_iter()
        .map(|row| {
            let current = current_id.is_some_and(|id| id == row.0);
            let status = if row.4.is_some() {
                "revoked"
            } else if row.3 <= now || row.2 < now - INACTIVITY_MINUTES * 60 {
                "expired"
            } else if current {
                "current"
            } else {
                "active"
            };
            AccountSession {
                id: row.0,
                logged_in_at: unix_timestamp(row.1),
                last_seen_at: unix_timestamp(row.2),
                status,
                user_agent_summary: row.5,
                current,
            }
        })
        .collect())
}

pub(crate) async fn security_events(
    state: &AuthState,
    account_id: &str,
    offset: i64,
    limit: usize,
) -> Result<Vec<SecurityEventRow>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT event_type,occurred_at FROM security_events WHERE account_id=? ORDER BY occurred_at DESC,id DESC LIMIT ? OFFSET ?",
    )
    .bind(account_id)
    .bind(limit as i64)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    .context("list account security events")?;
    Ok(rows
        .into_iter()
        .filter_map(|(kind, occurred_at)| {
            security_event_label(&kind).map(|label| SecurityEventRow {
                occurred_at: unix_timestamp(occurred_at),
                label,
            })
        })
        .collect())
}

pub(crate) async fn revoke_account_session(
    state: &AuthState,
    account_id: &str,
    current_id: &str,
    target_id: &str,
) -> Result<bool> {
    let mut transaction = state
        .pool
        .begin()
        .await
        .context("begin session revocation")?;
    let tower_id: Option<String> = sqlx::query_scalar(
        "UPDATE account_sessions SET revoked_at=unixepoch() WHERE id=? AND account_id=? AND id<>? AND revoked_at IS NULL RETURNING tower_session_id",
    )
    .bind(target_id)
    .bind(account_id)
    .bind(current_id)
    .fetch_optional(&mut *transaction)
    .await
    .context("revoke account session")?;
    let Some(tower_id) = tower_id else {
        return Ok(false);
    };
    sqlx::query("DELETE FROM tower_sessions WHERE id=?")
        .bind(tower_id)
        .execute(&mut *transaction)
        .await
        .context("delete revoked session payload")?;
    insert_security_event(
        &mut transaction,
        Some(account_id),
        SecurityEvent::SessionRevoked,
    )
    .await?;
    transaction
        .commit()
        .await
        .context("commit session revocation")?;
    Ok(true)
}

pub(crate) async fn revoke_other_account_sessions(
    state: &AuthState,
    account_id: &str,
    current_id: &str,
) -> Result<u64> {
    let mut transaction = state
        .pool
        .begin()
        .await
        .context("begin other-session revocation")?;
    sqlx::query("DELETE FROM tower_sessions WHERE id IN (SELECT tower_session_id FROM account_sessions WHERE account_id=? AND id<>? AND revoked_at IS NULL)")
        .bind(account_id)
        .bind(current_id)
        .execute(&mut *transaction)
        .await
        .context("delete other session payloads")?;
    let result = sqlx::query("UPDATE account_sessions SET revoked_at=unixepoch() WHERE account_id=? AND id<>? AND revoked_at IS NULL")
        .bind(account_id)
        .bind(current_id)
        .execute(&mut *transaction)
        .await
        .context("revoke other account sessions")?;
    insert_security_event(
        &mut transaction,
        Some(account_id),
        SecurityEvent::SessionsRevoked,
    )
    .await?;
    transaction
        .commit()
        .await
        .context("commit other-session revocation")?;
    Ok(result.rows_affected())
}

pub(crate) async fn current_inventory_id(session: &Session) -> Result<Option<String>> {
    session
        .get(INVENTORY_ID)
        .await
        .context("load session inventory ID")
}

pub(crate) fn summarize_user_agent(value: Option<&str>) -> String {
    let value = value.unwrap_or_default();
    let client = if value.contains("Edg/") {
        "Edge"
    } else if value.contains("Firefox/") {
        "Firefox"
    } else if value.contains("Chrome/") || value.contains("Chromium/") {
        "Chrome"
    } else if value.contains("Safari/") {
        "Safari"
    } else if value.starts_with("curl/") {
        "curl"
    } else if value.starts_with("Wget/") {
        "Wget"
    } else if value.is_empty() {
        "Unknown client"
    } else {
        "Other client"
    };
    let platform = if value.contains("Android") {
        Some("Android")
    } else if value.contains("iPhone") || value.contains("iPad") {
        Some("iOS")
    } else if value.contains("Windows") {
        Some("Windows")
    } else if value.contains("Macintosh") || value.contains("Mac OS X") {
        Some("macOS")
    } else if value.contains("Linux") {
        Some("Linux")
    } else {
        None
    };
    platform.map_or_else(
        || client.to_owned(),
        |platform| format!("{client} on {platform}"),
    )
}

pub async fn set_enabled(state: &StateStore, username: &str, enabled: bool) -> Result<()> {
    let username = validate_username(username)?;
    let mut transaction = state
        .auth
        .pool
        .begin()
        .await
        .context("begin account state change")?;
    let account_id: Option<String> = if enabled {
        sqlx::query_scalar("UPDATE accounts SET enabled=1 WHERE username=? COLLATE NOCASE RETURNING id").bind(username).fetch_optional(&mut *transaction).await
    } else {
        sqlx::query_scalar("UPDATE accounts SET enabled=0,session_generation=session_generation+1 WHERE username=? COLLATE NOCASE RETURNING id").bind(username).fetch_optional(&mut *transaction).await
    }.context("change application account state")?;
    let account_id = account_id.context("application account not found")?;
    if !enabled {
        revoke_inventory_in_transaction(&mut transaction, &account_id).await?;
    }
    insert_security_event(
        &mut transaction,
        Some(&account_id),
        if enabled {
            SecurityEvent::AccountEnabled
        } else {
            SecurityEvent::AccountDisabled
        },
    )
    .await?;
    transaction
        .commit()
        .await
        .context("commit account state change")?;
    Ok(())
}

pub async fn revoke_sessions(state: &StateStore, username: &str) -> Result<()> {
    let username = validate_username(username)?;
    let mut transaction = state
        .auth
        .pool
        .begin()
        .await
        .context("begin account-wide session revocation")?;
    let account_id: Option<String> = sqlx::query_scalar("UPDATE accounts SET session_generation=session_generation+1 WHERE username=? COLLATE NOCASE RETURNING id")
        .bind(username).fetch_optional(&mut *transaction).await.context("revoke application account sessions")?;
    let account_id = account_id.context("application account not found")?;
    revoke_inventory_in_transaction(&mut transaction, &account_id).await?;
    insert_security_event(
        &mut transaction,
        Some(&account_id),
        SecurityEvent::SessionsRevoked,
    )
    .await?;
    transaction
        .commit()
        .await
        .context("commit account-wide session revocation")?;
    Ok(())
}

impl AuthState {
    pub(crate) async fn token_account(
        &self,
        account_id: &str,
        generation: i64,
    ) -> Result<Option<CurrentUser>> {
        let account: Option<AccountRow> = sqlx::query_as("SELECT id,username,password_hash,enabled,role,session_generation,display_name,password_changed_at FROM accounts WHERE id=? AND enabled=1 AND session_generation=?")
            .bind(account_id).bind(generation).fetch_optional(&self.pool).await.context("load token account")?;
        match account {
            Some(account) => Ok(Some(self.current_account(account).await?)),
            None => Ok(None),
        }
    }
    pub fn is_local_mode(&self) -> bool {
        self.local_mode
    }

    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Option<(CurrentUser, i64)>> {
        if password.len() > MAX_PASSWORD_BYTES {
            return Ok(None);
        }
        let account = if validate_username(username).is_ok() {
            sqlx::query_as::<_, AccountRow>("SELECT id,username,password_hash,enabled,role,session_generation,display_name,password_changed_at FROM accounts WHERE username=? COLLATE NOCASE")
                .bind(username).fetch_optional(&self.pool).await.context("load application account")?
        } else {
            None
        };
        let hash = account
            .as_ref()
            .map(|account| account.password_hash.clone())
            .unwrap_or_else(|| self.dummy_hash.clone());
        let password = password.to_owned();
        let valid = tokio::task::spawn_blocking(move || verify_password(&hash, &password))
            .await
            .context("join password verification")?;
        let Some(account) = account.filter(|account| valid && account.enabled) else {
            return Ok(None);
        };
        let generation = account.session_generation;
        Ok(Some((self.current_account(account).await?, generation)))
    }

    async fn current_user(&self, session: &Session) -> Result<Option<CurrentUser>> {
        let Some(account_id) = session.get::<String>(ACCOUNT_ID).await? else {
            return Ok(None);
        };
        let Some(generation) = session.get::<i64>(SESSION_GENERATION).await? else {
            return Ok(None);
        };
        let Some(issued_at) = session.get::<i64>(ISSUED_AT).await? else {
            return Ok(None);
        };
        if unix_now().saturating_sub(issued_at) > self.absolute_lifetime_seconds {
            session.flush().await?;
            return Ok(None);
        }
        let account = sqlx::query_as::<_, AccountRow>(
            "SELECT id,username,password_hash,enabled,role,session_generation,display_name,password_changed_at FROM accounts WHERE id=?",
        )
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
        .context("load authenticated application account")?;
        let Some(account) = account else {
            session.flush().await?;
            return Ok(None);
        };
        if !account.enabled || account.session_generation != generation {
            session.flush().await?;
            return Ok(None);
        }
        if !self
            .touch_or_adopt_session(session, &account.id, generation, issued_at)
            .await?
        {
            session.flush().await?;
            return Ok(None);
        }
        Ok(Some(self.current_account(account).await?))
    }

    async fn touch_or_adopt_session(
        &self,
        session: &Session,
        account_id: &str,
        generation: i64,
        issued_at: i64,
    ) -> Result<bool> {
        let Some(tower_id) = session.id().map(|id| id.to_string()) else {
            return Ok(false);
        };
        if let Some(inventory_id) = session.get::<String>(INVENTORY_ID).await? {
            let touched: Option<String> = sqlx::query_scalar(
                "UPDATE account_sessions SET last_seen_at=unixepoch() WHERE id=? AND account_id=? AND tower_session_id=? AND session_generation=? AND revoked_at IS NULL AND absolute_expires_at>unixepoch() AND last_seen_at>=unixepoch()-? RETURNING id",
            )
            .bind(inventory_id)
            .bind(account_id)
            .bind(tower_id)
            .bind(generation)
            .bind(INACTIVITY_MINUTES * 60)
            .fetch_optional(&self.pool)
            .await
            .context("touch account session inventory")?;
            return Ok(touched.is_some());
        }
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT id FROM account_sessions WHERE tower_session_id=? AND account_id=? AND revoked_at IS NULL",
        )
        .bind(&tower_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await
        .context("find legacy account session inventory")?;
        let inventory_id = existing.unwrap_or_else(random_token);
        sqlx::query("INSERT OR IGNORE INTO account_sessions(id,account_id,tower_session_id,session_generation,logged_in_at,last_seen_at,absolute_expires_at,user_agent_summary) VALUES(?,?,?,?,?,unixepoch(),?,'Unknown client (upgraded session)')")
            .bind(&inventory_id)
            .bind(account_id)
            .bind(tower_id)
            .bind(generation)
            .bind(issued_at)
            .bind(issued_at.saturating_add(self.absolute_lifetime_seconds))
            .execute(&self.pool)
            .await
            .context("adopt legacy account session")?;
        session.insert(INVENTORY_ID, &inventory_id).await?;
        Ok(true)
    }

    async fn current_account(&self, account: AccountRow) -> Result<CurrentUser> {
        let role = parse_role(&account.role)?;
        let source_keys = source_grants(&self.pool, &account.id).await?;
        let source_count = source_keys.len();
        let source_keys_json = serde_json::to_string(&source_keys)
            .context("encode application account source grants")?;
        let preferences = load_preferences(&self.pool, &account.id).await?;
        Ok(CurrentUser {
            id: account.id,
            credential_generation: account.session_generation,
            username: account.username,
            display_name: account.display_name,
            password_changed_at: account.password_changed_at,
            preferences,
            access: AccessScope {
                role,
                capabilities: crate::authorization::Capabilities::for_role(
                    role == AccountRole::Admin,
                ),
                source_keys_json,
                source_count,
            },
        })
    }

    #[cfg(test)]
    fn accepts_test_basic(&self, request: &Request) -> bool {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use sha2::{Digest, Sha256};
        let Some(expected) = self.test_digest else {
            return false;
        };
        let mut values = request.headers().get_all(header::AUTHORIZATION).iter();
        let Some(value) = values.next().and_then(|value| value.to_str().ok()) else {
            return false;
        };
        if values.next().is_some() {
            return false;
        }
        let Some((scheme, encoded)) = value.split_once(' ') else {
            return false;
        };
        if !scheme.eq_ignore_ascii_case("Basic") {
            return false;
        }
        let Ok(decoded) = STANDARD.decode(encoded) else {
            return false;
        };
        let digest: [u8; 32] = Sha256::digest(decoded).into();
        bool::from(expected.ct_eq(&digest))
    }
}

pub async fn establish_session(
    state: &AuthState,
    session: &Session,
    user: &CurrentUser,
    generation: i64,
    user_agent: Option<&str>,
) -> Result<()> {
    let existing_inventory = session.get::<String>(INVENTORY_ID).await?;
    session.cycle_id().await?;
    let issued_at = unix_now();
    session.insert(ACCOUNT_ID, &user.id).await?;
    session.insert(SESSION_GENERATION, generation).await?;
    session.insert(ISSUED_AT, issued_at).await?;
    let rotating_existing = existing_inventory.is_some();
    let inventory_id = existing_inventory.unwrap_or_else(random_token);
    session.insert(INVENTORY_ID, &inventory_id).await?;
    session.save().await?;
    let tower_id = session
        .id()
        .context("rotated application session has no identifier")?
        .to_string();
    let mut transaction = state
        .pool
        .begin()
        .await
        .context("begin session inventory update")?;
    if rotating_existing {
        sqlx::query("DELETE FROM tower_sessions WHERE id IN (SELECT tower_session_id FROM account_sessions WHERE account_id=? AND id<>? AND revoked_at IS NULL)")
            .bind(&user.id)
            .bind(&inventory_id)
            .execute(&mut *transaction)
            .await
            .context("delete sessions superseded by password change")?;
        sqlx::query("UPDATE account_sessions SET revoked_at=unixepoch() WHERE account_id=? AND id<>? AND revoked_at IS NULL")
            .bind(&user.id)
            .bind(&inventory_id)
            .execute(&mut *transaction)
            .await
            .context("revoke sessions superseded by password change")?;
        sqlx::query("UPDATE account_sessions SET tower_session_id=?,session_generation=?,last_seen_at=?,absolute_expires_at=?,revoked_at=NULL WHERE id=? AND account_id=?")
            .bind(&tower_id)
            .bind(generation)
            .bind(issued_at)
            .bind(issued_at.saturating_add(state.absolute_lifetime_seconds))
            .bind(&inventory_id)
            .bind(&user.id)
            .execute(&mut *transaction)
            .await
            .context("rotate current account session inventory")?;
        insert_security_event(
            &mut transaction,
            Some(&user.id),
            SecurityEvent::SessionsRevoked,
        )
        .await?;
    } else {
        sqlx::query("INSERT INTO account_sessions(id,account_id,tower_session_id,session_generation,logged_in_at,last_seen_at,absolute_expires_at,user_agent_summary) VALUES(?,?,?,?,?,?,?,?)")
            .bind(&inventory_id)
            .bind(&user.id)
            .bind(tower_id)
            .bind(generation)
            .bind(issued_at)
            .bind(issued_at)
            .bind(issued_at.saturating_add(state.absolute_lifetime_seconds))
            .bind(summarize_user_agent(user_agent))
            .execute(&mut *transaction)
            .await
            .context("create account session inventory")?;
        insert_security_event(
            &mut transaction,
            Some(&user.id),
            SecurityEvent::LoginSucceeded,
        )
        .await?;
    }
    transaction
        .commit()
        .await
        .context("commit session inventory update")?;
    Ok(())
}

pub async fn end_session(state: &AuthState, session: &Session) -> Result<()> {
    if let Some(inventory_id) = session.get::<String>(INVENTORY_ID).await? {
        let account_id: Option<String> = sqlx::query_scalar(
            "UPDATE account_sessions SET revoked_at=unixepoch() WHERE id=? AND revoked_at IS NULL RETURNING account_id",
        )
        .bind(inventory_id)
        .fetch_optional(&state.pool)
        .await
        .context("close account session inventory")?;
        if let Some(account_id) = account_id {
            insert_security_event_pool(state, Some(&account_id), SecurityEvent::SessionRevoked)
                .await?;
        }
    }
    session.flush().await?;
    Ok(())
}

pub async fn csrf_token(session: &Session) -> Result<String> {
    if let Some(token) = session.get::<String>(CSRF_TOKEN).await? {
        return Ok(token);
    }
    let token = random_token();
    session.insert(CSRF_TOKEN, &token).await?;
    Ok(token)
}

pub async fn verify_csrf(session: &Session, supplied: &str) -> Result<bool> {
    let Some(expected) = session.get::<String>(CSRF_TOKEN).await? else {
        return Ok(false);
    };
    Ok(expected.len() == supplied.len()
        && bool::from(expected.as_bytes().ct_eq(supplied.as_bytes())))
}

pub fn safe_return_to(value: &str) -> &str {
    if value.starts_with('/')
        && !value.starts_with("//")
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
    {
        value
    } else {
        "/sessions"
    }
}

pub async fn require_auth(
    State(auth): State<AuthState>,
    session: Session,
    mut request: Request,
    next: Next,
) -> Response {
    if auth.local_mode {
        request.extensions_mut().insert(CurrentUser {
            id: "local".into(),
            credential_generation: 0,
            username: "Local developer".into(),
            display_name: None,
            password_changed_at: "Unavailable".into(),
            access: AccessScope::all(),
            preferences: crate::preferences::UserPreferences::default(),
        });
        return next.run(request).await;
    }
    let has_bearer = request
        .headers()
        .get_all(header::AUTHORIZATION)
        .iter()
        .any(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.split_whitespace().next())
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("Bearer"))
        });
    if has_bearer || request.uri().path().starts_with("/api/") {
        if !auth.token_attempts.accept() {
            return token_rate_limited();
        }
        let mut values = request.headers().get_all(header::AUTHORIZATION).iter();
        let bearer = values
            .next()
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
            .map(|(_, bearer)| bearer);
        let Some(bearer) = bearer.filter(|_| values.next().is_none()) else {
            return token_unauthenticated();
        };
        let Some(required) =
            crate::authorization::token_required_capability(request.method(), request.uri().path())
        else {
            return (StatusCode::FORBIDDEN, JsonError("access_denied")).into_response();
        };
        match crate::tokens::authenticate(&auth, bearer, required).await {
            Ok(crate::tokens::Authentication::Authorized(user)) => {
                request.headers_mut().remove(header::AUTHORIZATION);
                request.extensions_mut().insert(*user);
                return next.run(request).await;
            }
            Ok(crate::tokens::Authentication::Invalid) => return token_unauthenticated(),
            Ok(crate::tokens::Authentication::Denied) => {
                return (StatusCode::FORBIDDEN, JsonError("access_denied")).into_response();
            }
            Ok(crate::tokens::Authentication::RateLimited) => return token_rate_limited(),
            Err(_) => {
                tracing::error!(error_kind = "authentication", "token authentication failed");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    JsonError("internal_error"),
                )
                    .into_response();
            }
        }
    }
    if matches!(
        request.uri().path(),
        "/login" | "/favicon.ico" | "/assets/favicon-32.png" | "/assets/agent-lens-logo.png"
    ) {
        return next.run(request).await;
    }
    match auth.current_user(&session).await {
        Ok(Some(user)) => {
            if !user
                .access
                .permits_route(request.method(), request.uri().path())
            {
                return (StatusCode::FORBIDDEN, "Access denied").into_response();
            }
            request.extensions_mut().insert(user);
            next.run(request).await
        }
        Ok(None) => {
            #[cfg(test)]
            if auth.accepts_test_basic(&request) {
                request.headers_mut().remove(header::AUTHORIZATION);
                request.extensions_mut().insert(CurrentUser {
                    id: "test-account".into(),
                    credential_generation: 0,
                    username: "synthetic".into(),
                    display_name: None,
                    password_changed_at: "Unavailable".into(),
                    access: AccessScope::all(),
                    preferences: crate::preferences::UserPreferences::default(),
                });
                return next.run(request).await;
            }
            if request.uri().path() == "/metrics"
                && let Some((username, password)) = basic_credentials(&request)
            {
                match auth.authenticate(&username, &password).await {
                    Ok(Some((user, _))) => {
                        if !user
                            .access
                            .permits_route(request.method(), request.uri().path())
                        {
                            return (StatusCode::FORBIDDEN, "Access denied").into_response();
                        }
                        request.headers_mut().remove(header::AUTHORIZATION);
                        request.extensions_mut().insert(user);
                        return next.run(request).await;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::error!(
                            error_kind = "authentication",
                            "metrics authentication failed"
                        );
                        let _ = error;
                        return (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
                            .into_response();
                    }
                }
            }
            unauthenticated(&request)
        }
        Err(error) => {
            tracing::error!(
                error_kind = "authentication",
                "authentication state lookup failed"
            );
            let _ = error;
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response()
        }
    }
}

struct JsonError(&'static str);

impl IntoResponse for JsonError {
    fn into_response(self) -> Response {
        axum::Json(serde_json::json!({"error": self.0})).into_response()
    }
}

fn token_unauthenticated() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer realm=\"Agent Lens API\"")],
        JsonError("authentication_required"),
    )
        .into_response()
}

fn token_rate_limited() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, "60")],
        JsonError("rate_limited"),
    )
        .into_response()
}

pub async fn security_headers(request: Request, next: Next) -> Response {
    let api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    if api && (response.status().is_client_error() || response.status().is_server_error()) {
        let code = match response.status() {
            StatusCode::NOT_FOUND => "not_found",
            StatusCode::FORBIDDEN => "access_denied",
            StatusCode::UNAUTHORIZED => "authentication_required",
            StatusCode::TOO_MANY_REQUESTS => "rate_limited",
            status if status.is_client_error() => "invalid_request",
            _ => "internal_error",
        };
        *response.body_mut() =
            axum::body::Body::from(serde_json::json!({"error": code}).to_string());
        response.headers_mut().remove(header::CONTENT_LENGTH);
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; script-src 'self'; connect-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn unauthenticated(request: &Request) -> Response {
    let wants_html = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().starts_with("text/html"))
        });
    if wants_html && request.method() == axum::http::Method::GET {
        let target = request
            .uri()
            .path_and_query()
            .map_or("/sessions", |value| value.as_str());
        (
            StatusCode::SEE_OTHER,
            [(
                header::LOCATION,
                format!("/login?return_to={}", percent_encode(target)),
            )],
            "",
        )
            .into_response()
    } else if request.uri().path() == "/metrics" {
        (
            StatusCode::UNAUTHORIZED,
            [(
                header::WWW_AUTHENTICATE,
                "Basic realm=\"Agent Lens metrics\", charset=\"UTF-8\"",
            )],
            "Authentication required",
        )
            .into_response()
    } else {
        (StatusCode::UNAUTHORIZED, "Authentication required").into_response()
    }
}

fn basic_credentials(request: &Request) -> Option<(String, String)> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let mut values = request.headers().get_all(header::AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Basic") {
        return None;
    }
    let decoded = String::from_utf8(STANDARD.decode(encoded).ok()?).ok()?;
    let (username, password) = decoded.split_once(':')?;
    Some((username.to_owned(), password.to_owned()))
}

fn validate_username(username: &str) -> Result<&str> {
    ensure!(
        (1..=64).contains(&username.len()),
        "username must contain 1 to 64 characters"
    );
    ensure!(
        username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)),
        "username may contain only ASCII letters, digits, '.', '_', and '-'"
    );
    Ok(username)
}

fn validate_source_key(source_key: &str) -> Result<()> {
    ensure!(
        source_key.len() == 68
            && source_key.starts_with("src_")
            && source_key[4..].bytes().all(|byte| byte.is_ascii_hexdigit()),
        "source ID must be a stable Agent Lens source ID"
    );
    Ok(())
}

fn parse_role(role: &str) -> Result<AccountRole> {
    match role {
        "admin" => Ok(AccountRole::Admin),
        "viewer" => Ok(AccountRole::Viewer),
        _ => anyhow::bail!("invalid application account role"),
    }
}

pub(crate) async fn insert_security_event(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    account_id: Option<&str>,
    event: SecurityEvent,
) -> Result<()> {
    sqlx::query("DELETE FROM security_events WHERE occurred_at<unixepoch()-?")
        .bind(SECURITY_RETENTION_DAYS * 86_400)
        .execute(&mut **transaction)
        .await
        .context("expire retained security events")?;
    sqlx::query("DELETE FROM account_sessions WHERE (revoked_at IS NOT NULL OR absolute_expires_at<=unixepoch() OR last_seen_at<unixepoch()-?) AND last_seen_at<unixepoch()-?")
        .bind(INACTIVITY_MINUTES * 60)
        .bind(SESSION_HISTORY_RETENTION_DAYS * 86_400)
        .execute(&mut **transaction)
        .await
        .context("expire retained account sessions")?;
    sqlx::query("INSERT INTO security_events(account_id,event_type) VALUES(?,?)")
        .bind(account_id)
        .bind(event.as_str())
        .execute(&mut **transaction)
        .await
        .context("record security event")?;
    Ok(())
}

async fn insert_security_event_pool(
    state: &AuthState,
    account_id: Option<&str>,
    event: SecurityEvent,
) -> Result<()> {
    let mut transaction = state.pool.begin().await.context("begin security event")?;
    insert_security_event(&mut transaction, account_id, event).await?;
    transaction.commit().await.context("commit security event")
}

async fn revoke_inventory_in_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    account_id: &str,
) -> Result<()> {
    sqlx::query("DELETE FROM tower_sessions WHERE id IN (SELECT tower_session_id FROM account_sessions WHERE account_id=? AND revoked_at IS NULL)")
        .bind(account_id)
        .execute(&mut **transaction)
        .await
        .context("delete account session payloads")?;
    sqlx::query("UPDATE account_sessions SET revoked_at=unixepoch() WHERE account_id=? AND revoked_at IS NULL")
        .bind(account_id)
        .execute(&mut **transaction)
        .await
        .context("revoke account session inventory")?;
    Ok(())
}

fn security_event_label(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "login_succeeded" => "Signed in",
        "login_failed" => "Failed sign-in",
        "password_changed" => "Password changed",
        "password_reset" => "Password reset by local administrator",
        "session_revoked" => "Session revoked",
        "sessions_revoked" => "Other sessions revoked",
        "account_created" => "Account created",
        "account_enabled" => "Account enabled",
        "account_disabled" => "Account disabled",
        "role_changed" => "Role changed",
        "source_granted" => "Source access granted",
        "source_revoked" => "Source access revoked",
        "profile_changed" => "Profile changed",
        "token_created" => "Personal access token created",
        "token_revoked" => "Personal access token revoked",
        _ => return None,
    })
}

fn unix_timestamp(value: i64) -> String {
    chrono::DateTime::from_timestamp(value, 0)
        .map(|timestamp| timestamp.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| "Unavailable".into())
}

async fn account_id(state: &StateStore, username: &str) -> Result<String> {
    let username = validate_username(username)?;
    sqlx::query_scalar("SELECT id FROM accounts WHERE username=? COLLATE NOCASE")
        .bind(username)
        .fetch_optional(&state.auth.pool)
        .await
        .context("load application account")?
        .ok_or_else(|| anyhow::anyhow!("application account not found"))
}

async fn source_grants(pool: &SqlitePool, account_id: &str) -> Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT source_key FROM account_source_grants WHERE account_id=? ORDER BY source_key",
    )
    .bind(account_id)
    .fetch_all(pool)
    .await
    .context("load application account source grants")
}

async fn load_preferences(
    pool: &SqlitePool,
    account_id: &str,
) -> Result<crate::preferences::UserPreferences> {
    type PreferenceRow = (
        String,
        String,
        String,
        String,
        i64,
        i64,
        i64,
        bool,
        String,
        bool,
        String,
    );
    let row: Option<PreferenceRow> = sqlx::query_as(
        "SELECT timezone_mode,timezone_name,date_time_format,appearance,font_scale,page_size,live_interval_seconds,live_paused,transcript_detail,code_wrap,default_sessions_view FROM account_preferences WHERE account_id=?",
    )
    .bind(account_id)
    .fetch_optional(pool)
    .await
    .context("load account preferences")?;
    let Some(row) = row else {
        return Ok(crate::preferences::UserPreferences::default());
    };
    Ok(crate::preferences::UserPreferences {
        timezone_mode: row.0,
        timezone_name: row.1,
        date_time_format: row.2,
        appearance: row.3,
        font_scale: u16::try_from(row.4).unwrap_or_default(),
        page_size: usize::try_from(row.5).unwrap_or_default(),
        live_interval_seconds: u64::try_from(row.6).unwrap_or_default(),
        live_paused: row.7,
        transcript_detail: row.8,
        code_wrap: row.9,
        default_sessions_view: row.10,
    }
    .sanitized())
}

fn read_password(path: &Path) -> Result<String> {
    let metadata = std::fs::metadata(path).context("inspect password file")?;
    ensure!(metadata.is_file(), "password path must be a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "password file must not be accessible by group or other users"
        );
    }
    let bytes = std::fs::read(path).context("read password file")?;
    ensure!(
        bytes.len() <= MAX_PASSWORD_BYTES + 2,
        "password is too long"
    );
    let mut password =
        String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("password file must be UTF-8"))?;
    if password.ends_with("\r\n") {
        password.truncate(password.len() - 2);
    } else if password.ends_with('\n') {
        password.pop();
    }
    validate_password(&password)?;
    Ok(password)
}

fn validate_password(password: &str) -> std::result::Result<(), PasswordPolicyError> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(PasswordPolicyError::TooShort);
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(PasswordPolicyError::TooLong);
    }
    if password.chars().any(char::is_control) {
        return Err(PasswordPolicyError::ControlCharacter);
    }
    if !password.chars().any(char::is_uppercase) {
        return Err(PasswordPolicyError::MissingUppercase);
    }
    if !password.chars().any(char::is_lowercase) {
        return Err(PasswordPolicyError::MissingLowercase);
    }
    if !password.chars().any(char::is_numeric) {
        return Err(PasswordPolicyError::MissingNumber);
    }
    if !password
        .chars()
        .any(|character| !character.is_alphanumeric() && !character.is_whitespace())
    {
        return Err(PasswordPolicyError::MissingSymbol);
    }
    Ok(())
}

fn password_hasher() -> Result<Argon2<'static>> {
    let params = Params::new(19_456, 2, 1, None)
        .map_err(|_| anyhow::anyhow!("configure password hashing"))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    password_hasher()?
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| anyhow::anyhow!("hash password"))
}

fn verify_password(hash: &str, password: &str) -> bool {
    let Ok(hasher) = password_hasher() else {
        return false;
    };
    PasswordHash::new(hash)
        .ok()
        .is_some_and(|hash| hasher.verify_password(password.as_bytes(), &hash).is_ok())
}

fn random_token() -> String {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn map_account_write_error(operation: &'static str, error: sqlx::Error) -> anyhow::Error {
    if matches!(&error, sqlx::Error::Database(database) if database.is_unique_violation()) {
        anyhow::anyhow!("application account already exists")
    } else {
        anyhow::Error::new(error).context(operation)
    }
}

#[cfg(test)]
#[derive(Clone)]
pub struct Credentials {
    digest: [u8; 32],
}

#[cfg(test)]
impl Credentials {
    pub fn load(path: &Path) -> Result<Self> {
        use sha2::{Digest, Sha256};
        #[derive(Deserialize)]
        struct Config {
            username: String,
            password: String,
        }
        let bytes = std::fs::read(path).context("read test authentication file")?;
        let config: Config = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid test authentication"))?;
        if config.username.is_empty() || config.password.is_empty() {
            anyhow::bail!("invalid test authentication");
        }
        Ok(Self {
            digest: Sha256::digest(format!("{}:{}", config.username, config.password)).into(),
        })
    }
}

#[cfg(test)]
pub fn test_state(credentials: Credentials) -> AuthState {
    let pool = SqlitePoolOptions::new()
        .connect_lazy_with(SqliteConnectOptions::new().filename(":memory:"));
    AuthState {
        pool,
        local_mode: false,
        dummy_hash: String::new(),
        absolute_lifetime_seconds: ABSOLUTE_HOURS * 60 * 60,
        token_attempts: crate::tokens::AttemptBudget::default(),
        test_digest: Some(credentials.digest),
    }
}

#[cfg(test)]
pub fn expire_immediately(state: &mut StateStore) {
    state.auth.absolute_lifetime_seconds = -1;
}

#[cfg(test)]
pub fn state_pool(state: &StateStore) -> &SqlitePool {
    &state.auth.pool
}

#[cfg(test)]
mod tests {
    use super::*;

    fn password_file(dir: &Path, value: &str) -> std::path::PathBuf {
        let path = dir.join("password");
        std::fs::write(&path, value).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        path
    }

    #[tokio::test]
    async fn accounts_hash_passwords_and_reset_disable_and_revoke_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
        assert_eq!(journal_mode, "wal");
        let first = password_file(dir.path(), "First1! synthetic passphrase\n");
        create_account(&state, "Synthetic.User", &first, None)
            .await
            .unwrap();
        let permissions = account_permissions(&state, "synthetic.user").await.unwrap();
        assert_eq!(permissions.role, "admin");
        assert!(permissions.source_keys.is_empty());
        create_account(&state, "viewer", &first, None)
            .await
            .unwrap();
        let source_key = format!("src_{}", "a".repeat(64));
        grant_source(&state, "viewer", &source_key).await.unwrap();
        let permissions = account_permissions(&state, "viewer").await.unwrap();
        assert_eq!(permissions.role, "viewer");
        assert_eq!(permissions.source_keys.len(), 1);
        assert_eq!(permissions.source_keys[0], source_key);
        revoke_source(&state, "viewer", &source_key).await.unwrap();
        set_role(&state, "viewer", AccountRole::Admin)
            .await
            .unwrap();
        assert_eq!(
            account_permissions(&state, "viewer").await.unwrap().role,
            "admin"
        );
        let stored: String = sqlx::query_scalar("SELECT password_hash FROM accounts")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
        assert!(stored.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(!stored.contains("First1! synthetic passphrase"));
        assert!(
            state
                .auth
                .authenticate("synthetic.user", "First1! synthetic passphrase")
                .await
                .unwrap()
                .is_some()
        );
        revoke_sessions(&state, "Synthetic.User").await.unwrap();
        let generation: i64 = sqlx::query_scalar("SELECT session_generation FROM accounts")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
        assert_eq!(generation, 1);
        let second = password_file(dir.path(), "Second2! synthetic passphrase");
        reset_password(&state, "Synthetic.User", &second)
            .await
            .unwrap();
        assert!(
            state
                .auth
                .authenticate("Synthetic.User", "First1! synthetic passphrase")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            state
                .auth
                .authenticate("Synthetic.User", "Second2! synthetic passphrase")
                .await
                .unwrap()
                .is_some()
        );
        set_enabled(&state, "Synthetic.User", false).await.unwrap();
        assert!(
            state
                .auth
                .authenticate("Synthetic.User", "Second2! synthetic passphrase")
                .await
                .unwrap()
                .is_none()
        );
        set_enabled(&state, "Synthetic.User", true).await.unwrap();
        assert!(
            state
                .auth
                .authenticate("Synthetic.User", "Second2! synthetic passphrase")
                .await
                .unwrap()
                .is_some()
        );
        let events: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT event_type FROM security_events ORDER BY event_type",
        )
        .fetch_all(&state.auth.pool)
        .await
        .unwrap();
        for expected in [
            "account_created",
            "account_disabled",
            "account_enabled",
            "password_reset",
            "role_changed",
            "sessions_revoked",
            "source_granted",
            "source_revoked",
        ] {
            assert!(events.iter().any(|event| event == expected), "{events:?}");
        }
    }

    #[tokio::test]
    async fn typed_preferences_are_isolated_and_unknown_values_fall_back() {
        let dir = tempfile::tempdir().unwrap();
        let state = StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        let password = password_file(dir.path(), "Synthetic1! password");
        create_account(&state, "alice", &password, None)
            .await
            .unwrap();
        create_account(&state, "bob", &password, None)
            .await
            .unwrap();
        let alice_id = account_id(&state, "alice").await.unwrap();
        let bob_id = account_id(&state, "bob").await.unwrap();
        let alice_preferences = crate::preferences::UserPreferences {
            timezone_mode: "iana".into(),
            timezone_name: "Europe/London".into(),
            appearance: "high-contrast".into(),
            page_size: 25,
            live_interval_seconds: 30,
            live_paused: false,
            transcript_detail: "expanded".into(),
            code_wrap: true,
            default_sessions_view: "active".into(),
            ..crate::preferences::UserPreferences::default()
        };
        update_preferences(&state.auth, &alice_id, &alice_preferences)
            .await
            .unwrap();
        assert_eq!(
            load_preferences(&state.auth.pool, &alice_id).await.unwrap(),
            alice_preferences
        );
        assert_eq!(
            load_preferences(&state.auth.pool, &bob_id).await.unwrap(),
            crate::preferences::UserPreferences::default()
        );

        sqlx::query("UPDATE account_preferences SET appearance='future-theme',page_size=999 WHERE account_id=?")
            .bind(&alice_id)
            .execute(&state.auth.pool)
            .await
            .unwrap();
        let sanitized = load_preferences(&state.auth.pool, &alice_id).await.unwrap();
        assert_eq!(sanitized.appearance, "system");
        assert_eq!(sanitized.page_size, 50);
        assert_eq!(sanitized.timezone_name, "Europe/London");
    }

    #[test]
    fn return_targets_are_same_origin_paths_only() {
        assert_eq!(safe_return_to("/sessions?model=x"), "/sessions?model=x");
        for unsafe_value in [
            "https://evil.invalid",
            "//evil.invalid",
            "/\\evil.invalid",
            "sessions",
            "/ok\r\nLocation: evil",
        ] {
            assert_eq!(safe_return_to(unsafe_value), "/sessions");
        }
    }

    #[test]
    fn password_input_requires_a_regular_file() {
        let dir = tempfile::tempdir().unwrap();
        let error = read_password(dir.path()).unwrap_err().to_string();
        assert!(error.contains("regular file"), "{error}");
    }

    #[test]
    fn password_policy_requires_seven_complex_characters() {
        assert_eq!(validate_password("Aa1!xyz"), Ok(()));
        for (password, expected) in [
            ("Aa1!xy", PasswordPolicyError::TooShort),
            ("aa1!xyz", PasswordPolicyError::MissingUppercase),
            ("AA1!XYZ", PasswordPolicyError::MissingLowercase),
            ("Aa!!xyz", PasswordPolicyError::MissingNumber),
            ("Aa12xyz", PasswordPolicyError::MissingSymbol),
            ("Aa1 xyz", PasswordPolicyError::MissingSymbol),
            ("Aa1!xy\n", PasswordPolicyError::ControlCharacter),
        ] {
            assert_eq!(validate_password(password), Err(expected), "{password:?}");
        }
    }

    #[test]
    fn user_agents_are_reduced_to_bounded_allowlisted_summaries() {
        assert_eq!(
            summarize_user_agent(Some(
                "Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/140.0"
            )),
            "Firefox on Linux"
        );
        assert_eq!(summarize_user_agent(Some("curl/8.12.1")), "curl");
        assert_eq!(
            summarize_user_agent(Some("<script>alert('private')</script>")),
            "Other client"
        );
        assert_eq!(summarize_user_agent(None), "Unknown client");
    }

    #[tokio::test]
    async fn security_history_is_allowlisted_append_only_and_retained_for_180_days() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let state = StateStore::open(&path).await.unwrap();
        let password = password_file(dir.path(), "Synthetic1! password");
        create_account(&state, "alice", &password, None)
            .await
            .unwrap();
        let alice_id = account_id(&state, "alice").await.unwrap();
        record_failed_login(&state.auth, "alice").await.unwrap();
        record_failed_login(&state.auth, "unknown-account")
            .await
            .unwrap();
        let linked_failures: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM security_events WHERE event_type='login_failed' AND account_id=?",
        )
        .bind(&alice_id)
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
        let unlinked_failures: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM security_events WHERE event_type='login_failed' AND account_id IS NULL",
        )
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
        assert_eq!((linked_failures, unlinked_failures), (1, 1));
        sqlx::query("INSERT INTO security_events(account_id,event_type,occurred_at) VALUES(?,'login_failed',unixepoch()-181*86400)")
            .bind(&alice_id)
            .execute(&state.auth.pool)
            .await
            .unwrap();
        let event_id: i64 = sqlx::query_scalar("INSERT INTO security_events(account_id,event_type) VALUES(?,'login_succeeded') RETURNING id")
            .bind(&alice_id)
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
        assert!(
            sqlx::query("UPDATE security_events SET event_type='login_failed' WHERE id=?")
                .bind(event_id)
                .execute(&state.auth.pool)
                .await
                .is_err()
        );
        assert!(sqlx::query("INSERT INTO security_events(account_id,event_type) VALUES(?,'future_arbitrary_event')")
            .bind(&alice_id)
            .execute(&state.auth.pool)
            .await
            .is_err());
        sqlx::query("INSERT INTO account_sessions(id,account_id,tower_session_id,session_generation,logged_in_at,last_seen_at,absolute_expires_at,user_agent_summary,revoked_at) VALUES('old-session',?,'old-tower',0,unixepoch()-40*86400,unixepoch()-31*86400,unixepoch()-30*86400,'Other client',unixepoch()-31*86400),('recent-session',?,'recent-tower',0,unixepoch()-100,unixepoch()-100,unixepoch()+100,'Other client',unixepoch()-50)")
            .bind(&alice_id)
            .bind(&alice_id)
            .execute(&state.auth.pool)
            .await
            .unwrap();
        state.auth.pool.close().await;

        let reopened = StateStore::open(&path).await.unwrap();
        let events: Vec<String> = sqlx::query_scalar(
            "SELECT event_type FROM security_events WHERE account_id=? ORDER BY id",
        )
        .bind(&alice_id)
        .fetch_all(&reopened.auth.pool)
        .await
        .unwrap();
        assert!(events.contains(&"login_succeeded".to_owned()));
        assert_eq!(
            events
                .iter()
                .filter(|event| *event == "login_failed")
                .count(),
            1
        );
        let retained_sessions: Vec<String> =
            sqlx::query_scalar("SELECT id FROM account_sessions WHERE account_id=? ORDER BY id")
                .bind(&alice_id)
                .fetch_all(&reopened.auth.pool)
                .await
                .unwrap();
        assert_eq!(retained_sessions, vec!["recent-session"]);
    }

    #[tokio::test]
    async fn valid_pre_inventory_sessions_are_adopted_without_signing_out() {
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let state = StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        let password = password_file(dir.path(), "Synthetic1! password");
        create_account(&state, "alice", &password, None)
            .await
            .unwrap();
        let (user, generation) = state
            .auth
            .authenticate("alice", "Synthetic1! password")
            .await
            .unwrap()
            .unwrap();
        let session = Session::new(None, Arc::new(state.sessions.clone()), None);
        session.insert(ACCOUNT_ID, &user.id).await.unwrap();
        session
            .insert(SESSION_GENERATION, generation)
            .await
            .unwrap();
        session.insert(ISSUED_AT, unix_now()).await.unwrap();
        session.save().await.unwrap();
        assert!(session.get::<String>(INVENTORY_ID).await.unwrap().is_none());

        let adopted = state.auth.current_user(&session).await.unwrap().unwrap();
        assert_eq!(adopted.id, user.id);
        let inventory_id = session.get::<String>(INVENTORY_ID).await.unwrap().unwrap();
        let summary: String =
            sqlx::query_scalar("SELECT user_agent_summary FROM account_sessions WHERE id=?")
                .bind(inventory_id)
                .fetch_one(&state.auth.pool)
                .await
                .unwrap();
        assert_eq!(summary, "Unknown client (upgraded session)");
    }

    #[tokio::test]
    async fn v4_security_events_migrate_without_losing_history_or_append_only_enforcement() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let pool = SqlitePoolOptions::new()
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        let old_schema = STATE_SCHEMA
            .replace("'schema_version','5'", "'schema_version','4'")
            .replace(",'token_created','token_revoked'", "");
        sqlx::raw_sql(&old_schema).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO security_events(id,event_type,occurred_at) VALUES(42,'login_failed',unixepoch())")
            .execute(&pool).await.unwrap();
        pool.close().await;
        let state = StateStore::open(&path).await.unwrap();
        let kind: String = sqlx::query_scalar("SELECT event_type FROM security_events WHERE id=42")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
        assert_eq!(kind, "login_failed");
        sqlx::query("INSERT INTO security_events(event_type) VALUES('token_created')")
            .execute(&state.auth.pool)
            .await
            .unwrap();
        assert!(
            sqlx::query("UPDATE security_events SET event_type='token_revoked' WHERE id=42")
                .execute(&state.auth.pool)
                .await
                .is_err()
        );
        let version: String =
            sqlx::query_scalar("SELECT value FROM app_state_metadata WHERE key='schema_version'")
                .fetch_one(&state.auth.pool)
                .await
                .unwrap();
        assert_eq!(version, "5");
        state.auth.pool.close().await;
        StateStore::open(&path).await.unwrap();
    }

    #[tokio::test]
    async fn pre_permission_accounts_migrate_to_administrators() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy-state.sqlite");
        let pool = SqlitePoolOptions::new()
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE app_state_metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL); INSERT INTO app_state_metadata VALUES('schema_version','1'); CREATE TABLE accounts(id TEXT PRIMARY KEY,username TEXT NOT NULL COLLATE NOCASE UNIQUE,password_hash TEXT NOT NULL,enabled INTEGER NOT NULL DEFAULT 1,session_generation INTEGER NOT NULL DEFAULT 0,created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,password_changed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP); INSERT INTO accounts(id,username,password_hash) VALUES('legacy-id','legacy','$argon2id$invalid');",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool.close().await;

        let state = StateStore::open(&path).await.unwrap();
        let permissions = account_permissions(&state, "legacy").await.unwrap();
        assert_eq!(permissions.role, "admin");
        let version: String =
            sqlx::query_scalar("SELECT value FROM app_state_metadata WHERE key='schema_version'")
                .fetch_one(&state.auth.pool)
                .await
                .unwrap();
        assert_eq!(version, "5");
        let preferences_table: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='account_preferences')",
        )
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
        assert!(preferences_table);
        let session_table: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='account_sessions')",
        )
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
        assert!(session_table);
        let event_table: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='security_events')",
        )
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
        assert!(event_table);
    }

    #[tokio::test]
    async fn newer_state_schema_fails_without_downgrading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future-state.sqlite");
        let pool = SqlitePoolOptions::new()
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::query("CREATE TABLE app_state_metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO app_state_metadata VALUES('schema_version','99')")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let error = match StateStore::open(&path).await {
            Ok(_) => panic!("newer state schema must be rejected"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("newer"), "{error}");
        let pool = SqlitePoolOptions::new()
            .connect_with(SqliteConnectOptions::new().filename(&path))
            .await
            .unwrap();
        let version: String =
            sqlx::query_scalar("SELECT value FROM app_state_metadata WHERE key='schema_version'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(version, "99");
    }
}
