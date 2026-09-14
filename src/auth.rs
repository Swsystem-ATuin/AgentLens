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
const MIN_PASSWORD_CHARS: usize = 15;
const MAX_PASSWORD_BYTES: usize = 1_024;
const ACCOUNT_ID: &str = "auth.account_id";
const SESSION_GENERATION: &str = "auth.generation";
const ISSUED_AT: &str = "auth.issued_at";
const CSRF_TOKEN: &str = "auth.csrf";

const STATE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS app_state_metadata (key TEXT PRIMARY KEY,value TEXT NOT NULL);
INSERT OR IGNORE INTO app_state_metadata(key,value) VALUES('schema_version','2');
CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL COLLATE NOCASE UNIQUE,
    password_hash TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
    role TEXT NOT NULL DEFAULT 'viewer' CHECK(role IN ('admin','viewer')),
    session_generation INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    password_changed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS account_source_grants (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    source_key TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY(account_id,source_key)
);
"#;

#[derive(Clone)]
pub struct AuthState {
    pool: SqlitePool,
    dummy_hash: String,
    absolute_lifetime_seconds: i64,
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
    pub username: String,
    pub access: AccessScope,
}

#[derive(Clone, Debug)]
pub struct AccessScope {
    role: AccountRole,
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
    pub fn is_admin(&self) -> bool {
        self.role == AccountRole::Admin
    }

    pub fn role(&self) -> &'static str {
        self.role.as_str()
    }

    pub fn source_count(&self) -> usize {
        self.source_count
    }

    pub(crate) fn unrestricted(&self) -> bool {
        self.is_admin()
    }

    pub(crate) fn source_keys_json(&self) -> &str {
        &self.source_keys_json
    }

    fn permits_route(&self, path: &str) -> bool {
        if self.is_admin() {
            return true;
        }
        if matches!(
            path,
            "/" | "/account"
                | "/logout"
                | "/sessions"
                | "/usage"
                | "/diagnostics"
                | "/assets/live-reader.js"
        ) {
            return true;
        }
        let parts: Vec<_> = path.trim_start_matches('/').split('/').collect();
        matches!(
            parts.as_slice(),
            ["sessions", _]
                | ["sessions", _, "live-status" | "turns"]
                | ["sessions", _, "postmortem", "json" | "md"]
                | ["sessions", _, "messages" | "tools" | "results", _]
        )
    }

    #[cfg(test)]
    pub(crate) fn all() -> Self {
        Self {
            role: AccountRole::Admin,
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

impl StateStore {
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
                schema_version <= 2,
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
        sqlx::query("UPDATE app_state_metadata SET value='2' WHERE key='schema_version'")
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
        let dummy_hash = hash_password("synthetic-dummy-password")?;
        Ok(Self {
            auth: AuthState {
                pool,
                dummy_hash,
                absolute_lifetime_seconds: ABSOLUTE_HOURS * 60 * 60,
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
    sqlx::query("INSERT INTO accounts(id,username,password_hash,role) VALUES(?,?,?,?)")
        .bind(random_token())
        .bind(username)
        .bind(hash)
        .bind(role.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_account_write_error("create application account", error))?;
    transaction
        .commit()
        .await
        .context("commit account creation")?;
    Ok(())
}

pub async fn set_role(state: &StateStore, username: &str, role: AccountRole) -> Result<()> {
    let username = validate_username(username)?;
    let result = sqlx::query("UPDATE accounts SET role=? WHERE username=? COLLATE NOCASE")
        .bind(role.as_str())
        .bind(username)
        .execute(&state.auth.pool)
        .await
        .context("change application account role")?;
    ensure!(result.rows_affected() == 1, "application account not found");
    Ok(())
}

pub async fn grant_source(state: &StateStore, username: &str, source_key: &str) -> Result<()> {
    let account_id = account_id(state, username).await?;
    validate_source_key(source_key)?;
    sqlx::query("INSERT OR IGNORE INTO account_source_grants(account_id,source_key) VALUES(?,?)")
        .bind(account_id)
        .bind(source_key)
        .execute(&state.auth.pool)
        .await
        .context("grant source access")?;
    Ok(())
}

pub async fn revoke_source(state: &StateStore, username: &str, source_key: &str) -> Result<()> {
    let account_id = account_id(state, username).await?;
    validate_source_key(source_key)?;
    sqlx::query("DELETE FROM account_source_grants WHERE account_id=? AND source_key=?")
        .bind(account_id)
        .bind(source_key)
        .execute(&state.auth.pool)
        .await
        .context("revoke source access")?;
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
    let result = sqlx::query("UPDATE accounts SET password_hash=?,password_changed_at=CURRENT_TIMESTAMP,session_generation=session_generation+1 WHERE username=? COLLATE NOCASE")
        .bind(hash).bind(username).execute(&state.auth.pool).await.context("reset application account password")?;
    ensure!(result.rows_affected() == 1, "application account not found");
    Ok(())
}

pub async fn set_enabled(state: &StateStore, username: &str, enabled: bool) -> Result<()> {
    let username = validate_username(username)?;
    let result = if enabled {
        sqlx::query("UPDATE accounts SET enabled=1 WHERE username=? COLLATE NOCASE").bind(username).execute(&state.auth.pool).await
    } else {
        sqlx::query("UPDATE accounts SET enabled=0,session_generation=session_generation+1 WHERE username=? COLLATE NOCASE").bind(username).execute(&state.auth.pool).await
    }.context("change application account state")?;
    ensure!(result.rows_affected() == 1, "application account not found");
    Ok(())
}

pub async fn revoke_sessions(state: &StateStore, username: &str) -> Result<()> {
    let username = validate_username(username)?;
    let result = sqlx::query("UPDATE accounts SET session_generation=session_generation+1 WHERE username=? COLLATE NOCASE")
        .bind(username).execute(&state.auth.pool).await.context("revoke application account sessions")?;
    ensure!(result.rows_affected() == 1, "application account not found");
    Ok(())
}

impl AuthState {
    pub async fn authenticate(
        &self,
        username: &str,
        password: &str,
    ) -> Result<Option<(CurrentUser, i64)>> {
        if password.len() > MAX_PASSWORD_BYTES {
            return Ok(None);
        }
        let account = if validate_username(username).is_ok() {
            sqlx::query_as::<_, AccountRow>("SELECT id,username,password_hash,enabled,role,session_generation FROM accounts WHERE username=? COLLATE NOCASE")
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
            "SELECT id,username,password_hash,enabled,role,session_generation FROM accounts WHERE id=?",
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
        Ok(Some(self.current_account(account).await?))
    }

    async fn current_account(&self, account: AccountRow) -> Result<CurrentUser> {
        let role = parse_role(&account.role)?;
        let source_keys = source_grants(&self.pool, &account.id).await?;
        let source_count = source_keys.len();
        let source_keys_json = serde_json::to_string(&source_keys)
            .context("encode application account source grants")?;
        Ok(CurrentUser {
            id: account.id,
            username: account.username,
            access: AccessScope {
                role,
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
    session: &Session,
    user: &CurrentUser,
    generation: i64,
) -> Result<()> {
    session.cycle_id().await?;
    session.insert(ACCOUNT_ID, &user.id).await?;
    session.insert(SESSION_GENERATION, generation).await?;
    session.insert(ISSUED_AT, unix_now()).await?;
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
    if request.uri().path() == "/login" {
        return next.run(request).await;
    }
    match auth.current_user(&session).await {
        Ok(Some(user)) => {
            if !user.access.permits_route(request.uri().path()) {
                return (StatusCode::FORBIDDEN, "Administrator access required").into_response();
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
                    username: "synthetic".into(),
                    access: AccessScope::all(),
                });
                return next.run(request).await;
            }
            if request.uri().path() == "/metrics"
                && let Some((username, password)) = basic_credentials(&request)
            {
                match auth.authenticate(&username, &password).await {
                    Ok(Some((user, _))) => {
                        if !user.access.permits_route(request.uri().path()) {
                            return (StatusCode::FORBIDDEN, "Administrator access required")
                                .into_response();
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

pub async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'; script-src 'self'; connect-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"));
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

fn validate_password(password: &str) -> Result<()> {
    ensure!(
        password.chars().count() >= MIN_PASSWORD_CHARS,
        "password must contain at least {MIN_PASSWORD_CHARS} characters"
    );
    ensure!(password.len() <= MAX_PASSWORD_BYTES, "password is too long");
    ensure!(
        !password.chars().any(char::is_control),
        "password must not contain control characters"
    );
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
        dummy_hash: String::new(),
        absolute_lifetime_seconds: ABSOLUTE_HOURS * 60 * 60,
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
        let first = password_file(dir.path(), "first synthetic passphrase\n");
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
        assert!(!stored.contains("first synthetic passphrase"));
        assert!(
            state
                .auth
                .authenticate("synthetic.user", "first synthetic passphrase")
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
        let second = password_file(dir.path(), "second synthetic passphrase");
        reset_password(&state, "Synthetic.User", &second)
            .await
            .unwrap();
        assert!(
            state
                .auth
                .authenticate("Synthetic.User", "first synthetic passphrase")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            state
                .auth
                .authenticate("Synthetic.User", "second synthetic passphrase")
                .await
                .unwrap()
                .is_some()
        );
        set_enabled(&state, "Synthetic.User", false).await.unwrap();
        assert!(
            state
                .auth
                .authenticate("Synthetic.User", "second synthetic passphrase")
                .await
                .unwrap()
                .is_none()
        );
        set_enabled(&state, "Synthetic.User", true).await.unwrap();
        assert!(
            state
                .auth
                .authenticate("Synthetic.User", "second synthetic passphrase")
                .await
                .unwrap()
                .is_some()
        );
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
        assert_eq!(version, "2");
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
