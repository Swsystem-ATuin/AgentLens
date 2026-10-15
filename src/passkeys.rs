//! Password-first WebAuthn second factors. Challenge and proof state never leaves this process.
use crate::{
    auth::{self, AuthState, CurrentUser, SecurityEvent},
    mfa,
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use sqlx::{Sqlite, Transaction};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use webauthn_rs::prelude::*;

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS account_passkeys (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    credential_id BLOB NOT NULL UNIQUE,
    credential TEXT NOT NULL,
    origin TEXT NOT NULL,
    name TEXT NOT NULL,
    sign_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_passkeys_account ON account_passkeys(account_id,created_at,id);
"#;

enum State {
    Register(PasskeyRegistration, String),
    Authenticate(PasskeyAuthentication, bool), // true: browser sign-in, false: one-action factor proof
    Proof,
}
struct Pending {
    user: String,
    generation: i64,
    binding: String,
    expires: Instant,
    state: State,
}
#[derive(Clone, Default)]
pub(crate) struct Runtime {
    config: Option<Arc<Config>>,
    pending: Arc<Mutex<HashMap<String, Pending>>>,
}
struct Config {
    webauthn: Webauthn,
    origin: String,
}
impl Runtime {
    pub(crate) fn configured(value: Option<&str>) -> Result<Self> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        let url = Url::parse(value).context("parse passkey public URL")?;
        ensure!(
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/",
            "public URL must be an exact HTTPS origin without credentials, path, query, or fragment"
        );
        let host = url.host_str().context("public URL hostname missing")?;
        ensure!(
            !host.ends_with('.') && host.parse::<std::net::IpAddr>().is_err(),
            "passkeys require a DNS relying-party hostname"
        );
        let webauthn = WebauthnBuilder::new(host, &url)
            .map_err(|_| anyhow::anyhow!("invalid passkey relying-party configuration"))?
            .rp_name("Agent Lens")
            .build()
            .map_err(|_| anyhow::anyhow!("build passkey verifier"))?;
        Ok(Self {
            config: Some(Arc::new(Config {
                webauthn,
                origin: url.origin().ascii_serialization(),
            })),
            ..Default::default()
        })
    }
    pub(crate) fn enabled(&self) -> bool {
        self.config.is_some()
    }
    pub(crate) fn origin(&self) -> Option<&str> {
        self.config.as_ref().map(|c| c.origin.as_str())
    }
    fn config(&self) -> Result<&Config> {
        self.config
            .as_deref()
            .context("passkeys disabled until public HTTPS URL is configured")
    }
    fn insert(
        &self,
        user: &CurrentUser,
        binding: &str,
        state: State,
        seconds: u64,
    ) -> Result<String> {
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, entry| entry.expires > Instant::now());
        // Only one outstanding challenge/proof per browser context; bounded across all users.
        map.retain(|_, entry| entry.binding != binding);
        ensure!(map.len() < 1024, "passkey challenge capacity reached");
        let id = format!("pk_{}", auth::random_token());
        map.insert(
            id.clone(),
            Pending {
                user: user.id.clone(),
                generation: user.credential_generation,
                binding: binding.into(),
                expires: Instant::now() + Duration::from_secs(seconds),
                state,
            },
        );
        Ok(id)
    }
    fn take(&self, id: &str, binding: &str) -> Result<Pending> {
        let mut map = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map.get(id).context("passkey challenge unavailable")?;
        ensure!(
            entry.binding == binding && entry.expires > Instant::now(),
            "passkey challenge expired or belongs to another session"
        );
        map.remove(id).context("consume passkey challenge")
    }
    pub(crate) fn consume_proof(&self, user: &CurrentUser, code: &str, binding: &str) -> bool {
        self.take(code, binding).is_ok_and(|p| {
            p.user == user.id
                && p.generation == user.credential_generation
                && matches!(p.state, State::Proof)
        })
    }
}

#[derive(Serialize)]
pub(crate) struct Challenge<T: Serialize> {
    pub(crate) ticket: String,
    pub(crate) options: T,
}
#[derive(sqlx::FromRow, Serialize)]
pub(crate) struct KeyView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) created_at: String,
}
pub(crate) async fn list(auth: &AuthState, id: &str) -> Result<Vec<KeyView>> {
    sqlx::query_as("SELECT id,name,created_at FROM account_passkeys WHERE account_id=? ORDER BY created_at,id LIMIT 10").bind(id).fetch_all(&auth.pool).await.context("list passkey devices")
}
async fn credentials(
    tx: &mut Transaction<'_, Sqlite>,
    auth: &AuthState,
    id: &str,
) -> Result<Vec<Passkey>> {
    let origin = &auth.passkeys.config()?.origin;
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT credential,origin FROM account_passkeys WHERE account_id=? ORDER BY id LIMIT 10",
    )
    .bind(id)
    .fetch_all(&mut **tx)
    .await
    .context("load passkey public credentials")?;
    rows.into_iter().map(|(value,stored)|{ensure!(&stored==origin,"stored passkeys belong to a different public origin; restore configuration or recover enrollment");serde_json::from_str(&value).context("decode stored passkey public credential")}).collect()
}
pub(crate) async fn start_registration(
    auth: &AuthState,
    user: &CurrentUser,
    inventory: &str,
    name: &str,
) -> Result<Challenge<CreationChallengeResponse>> {
    ensure!(
        !name.trim().is_empty()
            && name.chars().count() <= 80
            && !name.chars().any(char::is_control),
        "device name must contain 1–80 non-control characters"
    );
    let mut tx = mfa::lock_identity(auth, user, inventory).await?;
    let keys = credentials(&mut tx, auth, &user.id).await?;
    ensure!(keys.len() < 10, "at most ten passkeys per account");
    let exclude = Some(keys.iter().map(|key| key.cred_id().clone()).collect());
    let handle = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, user.id.as_bytes());
    let (options, state) = auth
        .passkeys
        .config()?
        .webauthn
        .start_passkey_registration(handle, &user.username, &user.username, exclude)
        .map_err(|_| anyhow::anyhow!("start passkey registration"))?;
    let ticket = auth.passkeys.insert(
        user,
        &format!("i:{inventory}"),
        State::Register(state, name.trim().into()),
        300,
    )?;
    tx.commit()
        .await
        .context("commit passkey registration challenge")?;
    Ok(Challenge { ticket, options })
}
pub(crate) async fn finish_registration(
    auth: &AuthState,
    user: &CurrentUser,
    inventory: &str,
    ticket: &str,
    response: RegisterPublicKeyCredential,
) -> Result<(i64, Vec<String>)> {
    let pending = auth.passkeys.take(ticket, &format!("i:{inventory}"))?;
    ensure!(
        pending.user == user.id && pending.generation == user.credential_generation,
        "stale passkey enrollment identity"
    );
    let State::Register(state, name) = pending.state else {
        anyhow::bail!("wrong passkey challenge purpose");
    };
    let key = auth
        .passkeys
        .config()?
        .webauthn
        .finish_passkey_registration(&response, &state)
        .map_err(|_| anyhow::anyhow!("passkey registration rejected"))?;
    let mut tx = mfa::lock_identity(auth, user, inventory).await?;
    let keys = credentials(&mut tx, auth, &user.id).await?;
    ensure!(keys.len() < 10, "at most ten passkeys per account");
    let totp: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM account_mfa WHERE account_id=? AND secret IS NOT NULL)",
    )
    .bind(&user.id)
    .fetch_one(&mut *tx)
    .await
    .context("inspect existing second factors")?;
    sqlx::query("INSERT INTO account_passkeys(id,account_id,credential_id,credential,origin,name) VALUES(?,?,?,?,?,?)")
        .bind(auth::random_token()).bind(&user.id).bind(key.cred_id().as_ref() as &[u8]).bind(serde_json::to_string(&key).context("encode public passkey credential")?).bind(&auth.passkeys.config()?.origin).bind(name)
        .execute(&mut *tx).await.context("store unique passkey credential")?;
    sqlx::query("INSERT OR IGNORE INTO account_mfa(account_id) VALUES(?)")
        .bind(&user.id)
        .execute(&mut *tx)
        .await
        .context("initialize passkey factor budget")?;
    let codes = if keys.is_empty() && !totp {
        mfa::recovery(&mut tx, &user.id).await?
    } else {
        vec![]
    };
    let generation = mfa::invalidate(&mut tx, &user.id).await?;
    auth::insert_security_event(&mut tx, Some(&user.id), SecurityEvent::PasskeyAdded).await?;
    tx.commit().await.context("commit passkey registration")?;
    Ok((generation, codes))
}
pub(crate) async fn start_authentication(
    auth: &AuthState,
    user: &CurrentUser,
    binding: &str,
    login: bool,
) -> Result<Challenge<RequestChallengeResponse>> {
    let mut tx = auth
        .pool
        .begin()
        .await
        .context("begin passkey authentication challenge")?;
    let valid=sqlx::query("UPDATE accounts SET session_generation=session_generation WHERE id=? AND enabled=1 AND session_generation=?")
        .bind(&user.id).bind(user.credential_generation).execute(&mut *tx).await.context("recheck password-confirmed passkey account")?;
    ensure!(valid.rows_affected() == 1, "passkey identity unavailable");
    let keys = credentials(&mut tx, auth, &user.id).await?;
    ensure!(!keys.is_empty(), "no passkey enrolled");
    ensure!(
        mfa::budget(&mut tx, &user.id, auth::unix_now()).await?,
        "passkey verification budget exhausted"
    );
    let (options, state) = auth
        .passkeys
        .config()?
        .webauthn
        .start_passkey_authentication(&keys)
        .map_err(|_| anyhow::anyhow!("start passkey assertion"))?;
    tx.commit()
        .await
        .context("commit passkey assertion budget")?;
    Ok(Challenge {
        ticket: auth
            .passkeys
            .insert(user, binding, State::Authenticate(state, login), 300)?,
        options,
    })
}
pub(crate) async fn finish_authentication(
    auth: &AuthState,
    ticket: &str,
    binding: &str,
    response: PublicKeyCredential,
    login: bool,
) -> Result<CurrentUser> {
    let pending = auth.passkeys.take(ticket, binding)?;
    let State::Authenticate(state, purpose) = pending.state else {
        anyhow::bail!("wrong passkey challenge purpose");
    };
    ensure!(purpose == login, "passkey challenge purpose mismatch");
    let verification = auth
        .passkeys
        .config()?
        .webauthn
        .finish_passkey_authentication(&response, &state);
    let verified = match verification {
        Ok(result) => result,
        Err(_) => {
            let mut tx = auth
                .pool
                .begin()
                .await
                .context("begin rejected passkey audit")?;
            auth::insert_security_event(&mut tx, Some(&pending.user), SecurityEvent::MfaFailed)
                .await?;
            tx.commit()
                .await
                .context("record rejected passkey assertion")?;
            anyhow::bail!("passkey assertion rejected");
        }
    };
    let mut tx = auth
        .pool
        .begin()
        .await
        .context("begin atomic passkey assertion")?;
    let valid=sqlx::query("UPDATE accounts SET session_generation=session_generation WHERE id=? AND enabled=1 AND session_generation=?")
        .bind(&pending.user).bind(pending.generation).execute(&mut *tx).await.context("recheck assertion identity")?;
    ensure!(
        valid.rows_affected() == 1,
        "stale or disabled passkey account"
    );
    if let Some(inventory) = pending.binding.strip_prefix("i:") {
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM account_sessions WHERE id=? AND account_id=? AND session_generation=? AND revoked_at IS NULL AND absolute_expires_at>unixepoch() AND last_seen_at>=unixepoch()-1800)").bind(inventory).bind(&pending.user).bind(pending.generation).fetch_one(&mut *tx).await.context("recheck passkey assertion session")?;
        ensure!(active, "passkey session revoked during assertion");
    }
    let row:Option<(String,String,i64)>=sqlx::query_as("SELECT id,credential,sign_count FROM account_passkeys WHERE account_id=? AND credential_id=? AND origin=?")
        .bind(&pending.user).bind(verified.cred_id().as_ref() as &[u8]).bind(&auth.passkeys.config()?.origin).fetch_optional(&mut *tx).await.context("recheck current passkey enrollment")?;
    let (id, value, counter) = row.context("passkey revoked during authentication")?;
    let next = i64::from(verified.counter());
    ensure!(
        (counter == 0 && next == 0) || next > counter,
        "passkey signature counter regressed"
    );
    let mut key: Passkey =
        serde_json::from_str(&value).context("decode current public credential")?;
    ensure!(
        key.update_credential(&verified).is_some(),
        "credential update mismatch"
    );
    sqlx::query("UPDATE account_passkeys SET credential=?,sign_count=? WHERE id=?")
        .bind(serde_json::to_string(&key).context("encode updated public credential")?)
        .bind(next)
        .bind(id)
        .execute(&mut *tx)
        .await
        .context("persist passkey counter and backup state")?;
    auth::insert_security_event(&mut tx, Some(&pending.user), SecurityEvent::PasskeyUsed).await?;
    tx.commit().await.context("commit passkey assertion")?;
    auth.token_account(&pending.user, pending.generation)
        .await?
        .context("passkey account no longer available")
}
pub(crate) fn factor_proof(
    auth: &AuthState,
    user: &CurrentUser,
    inventory: &str,
) -> Result<String> {
    auth.passkeys
        .insert(user, &format!("i:{inventory}"), State::Proof, 90)
}
pub(crate) async fn delete(
    auth: &AuthState,
    user: &CurrentUser,
    inventory: &str,
    id: &str,
) -> Result<i64> {
    let mut tx = mfa::lock_identity(auth, user, inventory).await?;
    let (required, totp): (bool, bool) =
        sqlx::query_as("SELECT required,secret IS NOT NULL FROM account_mfa WHERE account_id=?")
            .bind(&user.id)
            .fetch_one(&mut *tx)
            .await
            .context("inspect passkey removal policy")?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM account_passkeys WHERE account_id=?")
        .bind(&user.id)
        .fetch_one(&mut *tx)
        .await
        .context("count remaining passkeys")?;
    ensure!(
        !required || totp || count > 1,
        "cannot remove the last required second factor"
    );
    let removed = sqlx::query("DELETE FROM account_passkeys WHERE id=? AND account_id=?")
        .bind(id)
        .bind(&user.id)
        .execute(&mut *tx)
        .await
        .context("remove own passkey")?;
    ensure!(removed.rows_affected() == 1, "passkey not found");
    if !totp && count == 1 {
        sqlx::query("DELETE FROM mfa_recovery_codes WHERE account_id=?")
            .bind(&user.id)
            .execute(&mut *tx)
            .await
            .context("remove unused MFA recovery digests")?;
    }
    let generation = mfa::invalidate(&mut tx, &user.id).await?;
    auth::insert_security_event(&mut tx, Some(&user.id), SecurityEvent::PasskeyRemoved).await?;
    tx.commit().await.context("commit passkey removal")?;
    Ok(generation)
}

#[cfg(test)]
mod tests;
