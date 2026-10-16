//! Local administrator-provisioned identity links, independent of the disposable index.
use crate::auth::{self, AuthState, SecurityEvent};
use anyhow::{Context, Result, ensure};
use sqlx::{Sqlite, Transaction};
use std::{io::Read, path::Path};

const MAX_LINKS: i64 = 16;
pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS account_oidc_links (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    issuer TEXT NOT NULL COLLATE BINARY CHECK(length(CAST(issuer AS BLOB)) BETWEEN 1 AND 4096),
    subject TEXT NOT NULL COLLATE BINARY CHECK(length(CAST(subject AS BLOB)) BETWEEN 1 AND 255 AND subject NOT GLOB '*[^ -~]*' AND instr(subject,char(0))=0),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(issuer,subject),
    UNIQUE(account_id,issuer)
);
"#;

#[derive(sqlx::FromRow)]
pub(crate) struct Link {
    pub(crate) id: String,
    pub(crate) issuer: String,
    pub(crate) created_at: String,
}

pub(super) fn validate_identity(issuer: &str, subject: &str) -> Result<()> {
    ensure!(
        issuer.len() <= 4096 && issuer.trim() == issuer && !issuer.chars().any(char::is_control),
        "OIDC issuer must be bounded and contain no surrounding whitespace or control characters"
    );
    super::https_url(issuer, false)?;
    ensure!(
        (1..=255).contains(&subject.len())
            && subject.bytes().all(|byte| (b' '..=b'~').contains(&byte)),
        "OIDC subject must contain 1–255 printable ASCII characters"
    );
    Ok(())
}

fn read_subject(path: &Path) -> Result<String> {
    let metadata = std::fs::symlink_metadata(path).context("inspect OIDC subject file")?;
    ensure!(
        metadata.is_file() && metadata.len() <= 257,
        "OIDC subject file must be a small regular file, not a symbolic link"
    );
    let file = std::fs::File::open(path).context("open OIDC subject file")?;
    let opened = file
        .metadata()
        .context("inspect opened OIDC subject file")?;
    ensure!(opened.is_file(), "OIDC subject file must be a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        ensure!(
            metadata.dev() == opened.dev() && metadata.ino() == opened.ino(),
            "OIDC subject file changed while opening"
        );
        ensure!(
            opened.permissions().mode() & 0o077 == 0,
            "OIDC subject file must be owner-only"
        );
    }
    let mut bytes = Vec::new();
    file.take(258)
        .read_to_end(&mut bytes)
        .context("read OIDC subject file")?;
    ensure!(bytes.len() <= 257, "OIDC subject file exceeds size limit");
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    String::from_utf8(bytes)
        .map_err(|_| anyhow::anyhow!("OIDC subject file must contain ASCII text"))
}

pub(crate) async fn link_file(
    auth: &AuthState,
    username: &str,
    config: &Path,
    subject: &Path,
) -> Result<String> {
    let configuration = super::load_file(config)?;
    let subject = read_subject(subject)?;
    link(auth, username, &configuration.issuer, &subject).await
}

async fn invalidate(
    tx: &mut Transaction<'_, Sqlite>,
    account_id: &str,
    event: SecurityEvent,
) -> Result<()> {
    sqlx::query("UPDATE accounts SET session_generation=session_generation+1 WHERE id=?")
        .bind(account_id)
        .execute(&mut **tx)
        .await
        .context("invalidate credentials after OIDC identity change")?;
    auth::revoke_inventory_in_transaction(tx, account_id).await?;
    auth::insert_security_event(tx, Some(account_id), event).await
}

async fn link(auth: &AuthState, username: &str, issuer: &str, subject: &str) -> Result<String> {
    let username = auth::validate_username(username)?;
    validate_identity(issuer, subject)?;
    let mut tx = auth
        .pool
        .begin()
        .await
        .context("begin OIDC identity linking")?;
    // Acquire the SQLite writer before reading policy/counts; serialize linking and account changes.
    let account: Option<String> = sqlx::query_scalar("UPDATE accounts SET session_generation=session_generation WHERE username=? COLLATE NOCASE AND enabled=1 RETURNING id")
        .bind(username).fetch_optional(&mut *tx).await.context("lock OIDC link target")?;
    let account = account.context("enabled application account not found")?;
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_oidc_links WHERE account_id=?")
            .bind(&account)
            .fetch_one(&mut *tx)
            .await
            .context("count OIDC identity links")?;
    ensure!(
        count < MAX_LINKS,
        "account OIDC identity link limit reached"
    );
    let id = format!("oidc_{}", auth::random_token());
    let inserted =
        sqlx::query("INSERT INTO account_oidc_links(id,account_id,issuer,subject) VALUES(?,?,?,?)")
            .bind(&id)
            .bind(&account)
            .bind(issuer)
            .bind(subject)
            .execute(&mut *tx)
            .await;
    if let Err(error) = inserted {
        if error
            .as_database_error()
            .is_some_and(|error| error.is_unique_violation())
        {
            anyhow::bail!(
                "OIDC identity or account/issuer already linked; unlink the existing link before replacement"
            );
        }
        return Err(error).context("store OIDC identity link");
    }
    invalidate(&mut tx, &account, SecurityEvent::OidcLinked).await?;
    tx.commit().await.context("commit OIDC identity link")?;
    Ok(id)
}

pub(crate) async fn unlink(auth: &AuthState, username: &str, id: &str) -> Result<()> {
    let username = auth::validate_username(username)?;
    ensure!(
        id.len() == 48
            && id.starts_with("oidc_")
            && id[5..]
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte)),
        "invalid OIDC link ID"
    );
    let mut tx = auth
        .pool
        .begin()
        .await
        .context("begin OIDC identity unlinking")?;
    let account: Option<String> = sqlx::query_scalar("UPDATE accounts SET session_generation=session_generation WHERE username=? COLLATE NOCASE RETURNING id")
        .bind(username).fetch_optional(&mut *tx).await.context("lock OIDC unlink target")?;
    let account = account.context("application account not found")?;
    let removed = sqlx::query("DELETE FROM account_oidc_links WHERE id=? AND account_id=?")
        .bind(id)
        .bind(&account)
        .execute(&mut *tx)
        .await
        .context("remove OIDC identity link")?;
    ensure!(
        removed.rows_affected() == 1,
        "OIDC identity link not found for account"
    );
    invalidate(&mut tx, &account, SecurityEvent::OidcUnlinked).await?;
    tx.commit().await.context("commit OIDC identity unlinking")
}

pub(crate) async fn list(auth: &AuthState, username: &str) -> Result<Vec<Link>> {
    let username = auth::validate_username(username)?;
    let account: Option<String> =
        sqlx::query_scalar("SELECT id FROM accounts WHERE username=? COLLATE NOCASE")
            .bind(username)
            .fetch_optional(&auth.pool)
            .await
            .context("load OIDC link target")?;
    let account = account.context("application account not found")?;
    sqlx::query_as("SELECT id,issuer,created_at FROM account_oidc_links WHERE account_id=? ORDER BY created_at,id LIMIT ?")
        .bind(account).bind(MAX_LINKS).fetch_all(&auth.pool).await.context("list account OIDC identity links")
}

#[cfg(test)]
mod tests;
