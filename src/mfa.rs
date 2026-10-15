//! Opt-in TOTP, mandatory enrollment gates, encrypted secrets, and one-use recovery.
use crate::auth::{self, AuthState, CurrentUser, SecurityEvent};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use password_hash::rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sqlx::{Sqlite, Transaction};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use subtle::ConstantTimeEq;
use totp_rs::{Algorithm, Secret, TOTP};

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS account_mfa (
    account_id TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    required INTEGER NOT NULL DEFAULT 0 CHECK(required IN (0,1)),
    secret TEXT,
    last_step INTEGER NOT NULL DEFAULT -1,
    pending_secret TEXT,
    pending_session TEXT,
    pending_generation INTEGER,
    pending_expires_at INTEGER,
    attempt_window INTEGER NOT NULL DEFAULT 0,
    attempt_count INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS mfa_recovery_codes (
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    digest BLOB NOT NULL,
    PRIMARY KEY(account_id,digest)
);
"#;

#[derive(Default, sqlx::FromRow)]
pub(crate) struct Status {
    pub(crate) required: bool,
    pub(crate) enrolled: bool,
    pub(crate) recovery_count: i64,
    pub(crate) totp_enrolled: bool,
}

pub(crate) async fn status(auth: &AuthState, id: &str) -> Result<Status> {
    Ok(sqlx::query_as("SELECT required,(secret IS NOT NULL OR EXISTS(SELECT 1 FROM account_passkeys WHERE account_id=account_mfa.account_id)) AS enrolled,secret IS NOT NULL AS totp_enrolled,(SELECT count(*) FROM mfa_recovery_codes WHERE account_id=?) AS recovery_count FROM account_mfa WHERE account_id=?")
        .bind(id).bind(id).fetch_optional(&auth.pool).await.context("load MFA status")?.unwrap_or_default())
}

#[derive(Serialize, Deserialize)]
struct Keyring {
    version: u32,
    active: String,
    keys: BTreeMap<String, String>,
}

fn key_file(path: &Path) -> Result<Keyring> {
    let metadata = std::fs::symlink_metadata(path).context("inspect MFA key file")?;
    ensure!(
        metadata.is_file() && metadata.len() <= 65536,
        "invalid MFA key file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "MFA key file must be owner-only"
        );
    }
    let mut bytes = vec![];
    std::fs::File::open(path)
        .context("open MFA key file")?
        .take(65537)
        .read_to_end(&mut bytes)
        .context("read MFA key file")?;
    let ring: Keyring = serde_json::from_slice(&bytes).context("decode MFA key file")?;
    ensure!(
        ring.version == 1
            && !ring.keys.is_empty()
            && ring.keys.len() <= 128
            && ring.keys.contains_key(&ring.active),
        "unsupported MFA key ring"
    );
    for value in ring.keys.values() {
        ensure!(
            STANDARD
                .decode(value)
                .context("decode MFA encryption key")?
                .len()
                == 32,
            "invalid MFA encryption key length"
        );
    }
    Ok(ring)
}
fn fresh_key() -> (String, String) {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    (auth::random_token(), STANDARD.encode(bytes))
}
fn write_keys(path: &Path, ring: &Keyring, replace: bool) -> Result<()> {
    let target = if replace {
        path.with_extension(format!("json.{}", auth::random_token()))
    } else {
        path.to_owned()
    };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&target)
        .context("create owner-only MFA key file")?;
    let bytes = serde_json::to_vec(ring).context("encode MFA key ring")?;
    file.write_all(&bytes).context("write MFA key ring")?;
    file.sync_all().context("sync MFA key ring")?;
    if replace {
        std::fs::rename(&target, path).context("publish rotated MFA key ring")?;
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::File::open(parent)
        .context("open MFA key directory")?
        .sync_all()
        .context("sync MFA key directory")?;
    Ok(())
}

pub(crate) async fn check_keys(auth: &AuthState) -> Result<()> {
    let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM account_mfa WHERE secret IS NOT NULL OR pending_secret IS NOT NULL)").fetch_one(&auth.pool).await.context("inspect encrypted MFA state")?;
    if exists || auth.mfa_key_file.exists() {
        key_file(&auth.mfa_key_file)?;
    }
    Ok(())
}

pub(crate) async fn prune_pending(pool: &sqlx::SqlitePool) -> Result<()> {
    sqlx::query("UPDATE account_mfa SET pending_secret=NULL,pending_session=NULL,pending_generation=NULL,pending_expires_at=NULL WHERE pending_expires_at<=unixepoch()")
        .execute(pool).await.context("expire unconfirmed MFA enrollment secrets")?;
    Ok(())
}
fn load_or_create(path: &Path) -> Result<Keyring> {
    if path.exists() {
        return key_file(path);
    }
    let (id, key) = fresh_key();
    let ring = Keyring {
        version: 1,
        active: id.clone(),
        keys: BTreeMap::from([(id, key)]),
    };
    match write_keys(path, &ring, false) {
        Ok(()) => Ok(ring),
        Err(_) if path.exists() => key_file(path),
        Err(e) => Err(e),
    }
}

pub(crate) async fn prepare_key(auth: &AuthState, tx: &mut Transaction<'_, Sqlite>) -> Result<()> {
    let encrypted:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM account_mfa WHERE secret IS NOT NULL OR pending_secret IS NOT NULL)").fetch_one(&mut **tx).await.context("check existing encrypted MFA secrets")?;
    ensure!(
        !encrypted || auth.mfa_key_file.exists(),
        "MFA encryption key unavailable; restore it or use local MFA recovery"
    );
    load_or_create(&auth.mfa_key_file)?;
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    key: String,
    nonce: String,
    ciphertext: String,
}
fn seal(ring: &Keyring, id: &str, purpose: &str, secret: &[u8]) -> Result<String> {
    let key = STANDARD
        .decode(
            ring.keys
                .get(&ring.active)
                .context("active MFA key missing")?,
        )
        .context("decode active MFA key")?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(&key).map_err(|_| anyhow::anyhow!("invalid MFA key"))?;
    let mut nonce = [0u8; 24];
    OsRng.fill_bytes(&mut nonce);
    let aad = format!("agent-lens:mfa:v1:{id}:{purpose}");
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: secret,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| anyhow::anyhow!("MFA encryption failed"))?;
    serde_json::to_string(&Envelope {
        version: 1,
        key: ring.active.clone(),
        nonce: STANDARD.encode(nonce),
        ciphertext: STANDARD.encode(ciphertext),
    })
    .context("encode encrypted MFA secret")
}
fn open(ring: &Keyring, id: &str, purpose: &str, value: &str) -> Result<Vec<u8>> {
    let envelope: Envelope = serde_json::from_str(value).context("decode encrypted MFA secret")?;
    ensure!(envelope.version == 1, "unsupported MFA encryption version");
    let key = STANDARD
        .decode(
            ring.keys
                .get(&envelope.key)
                .context("MFA encryption key unavailable")?,
        )
        .context("decode MFA key")?;
    let nonce = STANDARD
        .decode(envelope.nonce)
        .context("decode MFA nonce")?;
    ensure!(nonce.len() == 24, "invalid MFA nonce length");
    let bytes = STANDARD
        .decode(envelope.ciphertext)
        .context("decode MFA ciphertext")?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(&key).map_err(|_| anyhow::anyhow!("invalid MFA key"))?;
    let aad = format!("agent-lens:mfa:v1:{id}:{purpose}");
    cipher
        .decrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &bytes,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| anyhow::anyhow!("MFA secret authentication failed"))
}
fn totp(secret: Vec<u8>, name: &str) -> Result<TOTP> {
    TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        secret,
        Some("Agent Lens".into()),
        name.into(),
    )
    .map_err(|_| anyhow::anyhow!("invalid TOTP configuration"))
}
fn matched_step(secret: Vec<u8>, code: &str, now: i64, last: i64) -> Result<Option<i64>> {
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(None);
    }
    let generator = totp(secret, "account")?;
    let current = now / 30;
    for step in [current + 1, current, current - 1] {
        if step > last
            && step >= 0
            && bool::from(
                generator
                    .generate(step as u64 * 30)
                    .as_bytes()
                    .ct_eq(code.as_bytes()),
            )
        {
            return Ok(Some(step));
        }
    }
    Ok(None)
}

pub(crate) async fn lock_identity<'a>(
    auth: &'a AuthState,
    user: &CurrentUser,
    inventory: &str,
) -> Result<Transaction<'a, Sqlite>> {
    let mut tx = auth.pool.begin().await.context("begin MFA change")?;
    let count=sqlx::query("UPDATE accounts SET session_generation=session_generation WHERE id=? AND enabled=1 AND session_generation=? AND EXISTS(SELECT 1 FROM account_sessions s WHERE s.id=? AND s.account_id=accounts.id AND s.revoked_at IS NULL AND s.session_generation=accounts.session_generation AND s.absolute_expires_at>unixepoch() AND s.last_seen_at>=unixepoch()-1800)")
        .bind(&user.id).bind(user.credential_generation).bind(inventory).execute(&mut *tx).await.context("lock current MFA identity and session")?;
    ensure!(count.rows_affected() == 1, "MFA identity no longer valid");
    Ok(tx)
}

pub(crate) struct Enrollment {
    pub(crate) secret: String,
    pub(crate) uri: String,
}
pub(crate) async fn begin(
    auth: &AuthState,
    user: &CurrentUser,
    inventory: &str,
) -> Result<Enrollment> {
    let mut tx = lock_identity(auth, user, inventory).await?;
    let enrolled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM account_mfa WHERE account_id=? AND secret IS NOT NULL)",
    )
    .bind(&user.id)
    .fetch_one(&mut *tx)
    .await
    .context("check existing MFA enrollment")?;
    ensure!(!enrolled, "MFA already enrolled");
    prepare_key(auth, &mut tx).await?;
    let ring = key_file(&auth.mfa_key_file)?;
    let mut bytes = vec![0u8; 20];
    OsRng.fill_bytes(&mut bytes);
    let generator = totp(bytes.clone(), &user.username)?;
    let encrypted = seal(&ring, &user.id, "pending", &bytes)?;
    sqlx::query("INSERT INTO account_mfa(account_id,pending_secret,pending_session,pending_generation,pending_expires_at) VALUES(?,?,?,?,unixepoch()+600) ON CONFLICT(account_id) DO UPDATE SET pending_secret=excluded.pending_secret,pending_session=excluded.pending_session,pending_generation=excluded.pending_generation,pending_expires_at=excluded.pending_expires_at")
        .bind(&user.id).bind(encrypted).bind(inventory).bind(user.credential_generation).execute(&mut *tx).await.context("stage encrypted MFA enrollment")?;
    auth::insert_security_event(&mut tx, Some(&user.id), SecurityEvent::MfaEnrollmentStarted)
        .await?;
    tx.commit().await.context("commit pending MFA enrollment")?;
    Ok(Enrollment {
        secret: Secret::Raw(bytes).to_encoded().to_string(),
        uri: generator.get_url(),
    })
}

pub(crate) async fn budget(tx: &mut Transaction<'_, Sqlite>, id: &str, now: i64) -> Result<bool> {
    let accepted=sqlx::query("UPDATE account_mfa SET attempt_count=CASE WHEN attempt_window<=?-300 THEN 1 ELSE attempt_count+1 END,attempt_window=CASE WHEN attempt_window<=?-300 THEN ? ELSE attempt_window END WHERE account_id=? AND (attempt_window<=?-300 OR attempt_count<10)")
        .bind(now).bind(now).bind(now).bind(id).bind(now).execute(&mut **tx).await.context("apply MFA verification budget")?;
    Ok(accepted.rows_affected() == 1)
}

pub(crate) async fn finish(
    auth: &AuthState,
    user: &CurrentUser,
    inventory: &str,
    code: &str,
) -> Result<Option<(i64, Vec<String>)>> {
    let mut tx = lock_identity(auth, user, inventory).await?;
    let now = auth::unix_now();
    let pending:Option<String>=sqlx::query_scalar("SELECT pending_secret FROM account_mfa WHERE account_id=? AND secret IS NULL AND pending_session=? AND pending_generation=? AND pending_expires_at>?")
        .bind(&user.id).bind(inventory).bind(user.credential_generation).bind(now).fetch_optional(&mut *tx).await.context("load current pending MFA enrollment")?;
    let Some(pending) = pending else {
        return Ok(None);
    };
    if !budget(&mut tx, &user.id, now).await? {
        tx.commit().await.context("commit MFA budget")?;
        return Ok(None);
    }
    let ring = key_file(&auth.mfa_key_file)?;
    let secret = open(&ring, &user.id, "pending", &pending)?;
    let Some(step) = matched_step(secret.clone(), code, now, -1)? else {
        auth::insert_security_event(&mut tx, Some(&user.id), SecurityEvent::MfaFailed).await?;
        tx.commit()
            .await
            .context("record enrollment verification failure")?;
        return Ok(None);
    };
    sqlx::query("UPDATE account_mfa SET secret=?,last_step=?,pending_secret=NULL,pending_session=NULL,pending_generation=NULL,pending_expires_at=NULL,attempt_count=0 WHERE account_id=?")
        .bind(seal(&ring,&user.id,"totp",&secret)?).bind(step).bind(&user.id).execute(&mut *tx).await.context("activate MFA enrollment")?;
    let codes = recovery(&mut tx, &user.id).await?;
    let generation = invalidate(&mut tx, &user.id).await?;
    auth::insert_security_event(&mut tx, Some(&user.id), SecurityEvent::MfaEnabled).await?;
    tx.commit().await.context("commit MFA enrollment")?;
    Ok(Some((generation, codes)))
}

fn recovery_digest(id: &str, code: &str) -> [u8; 32] {
    blake3::derive_key(
        "Agent Lens MFA recovery v1",
        format!("{id}:{code}").as_bytes(),
    )
}
pub(crate) async fn recovery(tx: &mut Transaction<'_, Sqlite>, id: &str) -> Result<Vec<String>> {
    sqlx::query("DELETE FROM mfa_recovery_codes WHERE account_id=?")
        .bind(id)
        .execute(&mut **tx)
        .await
        .context("replace MFA recovery codes")?;
    let mut codes = vec![];
    for _ in 0..10 {
        let mut bytes = [0u8; 16];
        OsRng.fill_bytes(&mut bytes);
        let code = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        sqlx::query("INSERT INTO mfa_recovery_codes(account_id,digest) VALUES(?,?)")
            .bind(id)
            .bind(recovery_digest(id, &code).as_slice())
            .execute(&mut **tx)
            .await
            .context("store one-way MFA recovery digest")?;
        codes.push(code);
    }
    Ok(codes)
}

async fn consume(
    tx: &mut Transaction<'_, Sqlite>,
    auth: &AuthState,
    id: &str,
    code: &str,
    now: i64,
) -> Result<bool> {
    let value: Option<(String, i64)> = sqlx::query_as(
        "SELECT secret,last_step FROM account_mfa WHERE account_id=? AND secret IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await
    .context("load enrolled MFA secret")?;
    if !budget(tx, id, now).await? {
        return Ok(false);
    }
    if code.len() == 32 && code.bytes().all(|b| b.is_ascii_hexdigit()) {
        let used = sqlx::query("DELETE FROM mfa_recovery_codes WHERE account_id=? AND digest=?")
            .bind(id)
            .bind(recovery_digest(id, code).as_slice())
            .execute(&mut **tx)
            .await
            .context("consume MFA recovery code")?;
        if used.rows_affected() == 1 {
            auth::insert_security_event(tx, Some(id), SecurityEvent::MfaRecoveryUsed).await?;
            return Ok(true);
        }
    }
    let Some((value, last)) = value else {
        return Ok(false);
    };
    let ring = key_file(&auth.mfa_key_file)?;
    if let Some(step) = matched_step(open(&ring, id, "totp", &value)?, code, now, last)? {
        sqlx::query("UPDATE account_mfa SET last_step=? WHERE account_id=?")
            .bind(step)
            .bind(id)
            .execute(&mut **tx)
            .await
            .context("consume TOTP step")?;
        return Ok(true);
    }
    auth::insert_security_event(tx, Some(id), SecurityEvent::MfaFailed).await?;
    Ok(false)
}

/// Password verification occurs first. Consume a factor only for the same live generation.
pub(crate) async fn verify(auth: &AuthState, user: &CurrentUser, code: &str) -> Result<bool> {
    verify_bound(auth, user, code, None).await
}
pub(crate) async fn verify_in_session(
    auth: &AuthState,
    user: &CurrentUser,
    code: &str,
    session: &tower_sessions::Session,
) -> Result<bool> {
    let inventory = auth::current_inventory_id(session).await?;
    let binding = inventory.as_ref().map(|id| format!("i:{id}"));
    verify_bound(auth, user, code, binding.as_deref()).await
}
async fn verify_bound(
    auth: &AuthState,
    user: &CurrentUser,
    code: &str,
    binding: Option<&str>,
) -> Result<bool> {
    let mut tx = auth.pool.begin().await.context("begin MFA verification")?;
    let locked=sqlx::query("UPDATE accounts SET session_generation=session_generation WHERE id=? AND enabled=1 AND session_generation=?")
        .bind(&user.id).bind(user.credential_generation).execute(&mut *tx).await.context("recheck MFA account")?;
    if locked.rows_affected() != 1 {
        return Ok(false);
    }
    if let Some(inventory) = binding.and_then(|value| value.strip_prefix("i:")) {
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM account_sessions WHERE id=? AND account_id=? AND session_generation=? AND revoked_at IS NULL AND absolute_expires_at>unixepoch() AND last_seen_at>=unixepoch()-1800)").bind(inventory).bind(&user.id).bind(user.credential_generation).fetch_one(&mut *tx).await.context("recheck second-factor session")?;
        if !active {
            return Ok(false);
        }
    }
    let enrolled: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM account_mfa WHERE account_id=? AND (secret IS NOT NULL OR EXISTS(SELECT 1 FROM account_passkeys WHERE account_id=account_mfa.account_id)))",
    )
    .bind(&user.id)
    .fetch_one(&mut *tx)
    .await
    .context("check factor requirement")?;
    let valid = if code.starts_with("pk_") {
        binding.is_some_and(|b| auth.passkeys.consume_proof(user, code, b))
    } else if enrolled {
        consume(&mut tx, auth, &user.id, code, auth::unix_now()).await?
    } else {
        true
    };
    tx.commit().await.context("commit MFA verification")?;
    Ok(valid)
}

pub(crate) async fn manage(
    auth: &AuthState,
    user: &CurrentUser,
    inventory: &str,
    code: &str,
    disable: bool,
) -> Result<Option<(i64, Vec<String>)>> {
    let mut tx = lock_identity(auth, user, inventory).await?;
    let required: bool = sqlx::query_scalar("SELECT required FROM account_mfa WHERE account_id=?")
        .bind(&user.id)
        .fetch_optional(&mut *tx)
        .await
        .context("inspect required MFA policy")?
        .unwrap_or(false);
    ensure!(
        !disable || !required,
        "required MFA cannot be disabled by the account"
    );
    let valid = if code.starts_with("pk_") {
        auth.passkeys
            .consume_proof(user, code, &format!("i:{inventory}"))
    } else {
        consume(&mut tx, auth, &user.id, code, auth::unix_now()).await?
    };
    if !valid {
        tx.commit()
            .await
            .context("commit unsuccessful MFA management")?;
        return Ok(None);
    }
    let codes = if disable {
        reset_in_transaction(&mut tx, &user.id).await?;
        vec![]
    } else {
        recovery(&mut tx, &user.id).await?
    };
    let generation = invalidate(&mut tx, &user.id).await?;
    auth::insert_security_event(
        &mut tx,
        Some(&user.id),
        if disable {
            SecurityEvent::MfaDisabled
        } else {
            SecurityEvent::MfaRecoveryRotated
        },
    )
    .await?;
    tx.commit()
        .await
        .context("commit self-service MFA management")?;
    Ok(Some((generation, codes)))
}

pub(crate) async fn invalidate(tx: &mut Transaction<'_, Sqlite>, id: &str) -> Result<i64> {
    let generation=sqlx::query_scalar("UPDATE accounts SET session_generation=session_generation+1 WHERE id=? RETURNING session_generation").bind(id).fetch_one(&mut **tx).await.context("invalidate credentials after MFA change")?;
    auth::revoke_inventory_in_transaction(tx, id).await?;
    Ok(generation)
}
pub(crate) async fn reset_in_transaction(tx: &mut Transaction<'_, Sqlite>, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM account_passkeys WHERE account_id=?")
        .bind(id)
        .execute(&mut **tx)
        .await
        .context("reset enrolled passkey second factors")?;
    sqlx::query("UPDATE account_mfa SET secret=NULL,last_step=-1,pending_secret=NULL,pending_session=NULL,pending_generation=NULL,pending_expires_at=NULL,attempt_count=0 WHERE account_id=?").bind(id).execute(&mut **tx).await.context("reset MFA enrollment without removing requirement")?;
    sqlx::query("DELETE FROM mfa_recovery_codes WHERE account_id=?")
        .bind(id)
        .execute(&mut **tx)
        .await
        .context("remove MFA recovery digests")?;
    Ok(())
}
pub(crate) async fn cli_reset(auth: &AuthState, username: &str) -> Result<()> {
    auth::validate_username(username)?;
    let mut tx = auth
        .pool
        .begin()
        .await
        .context("begin local MFA recovery")?;
    let id:String=sqlx::query_scalar("UPDATE accounts SET session_generation=session_generation WHERE username=? COLLATE NOCASE RETURNING id").bind(username).fetch_optional(&mut *tx).await.context("resolve local MFA recovery target")?.context("account not found")?;
    reset_in_transaction(&mut tx, &id).await?;
    invalidate(&mut tx, &id).await?;
    auth::insert_security_event(&mut tx, Some(&id), SecurityEvent::MfaReset).await?;
    tx.commit().await.context("commit local MFA recovery")
}

pub(crate) async fn rotate_key(auth: &AuthState) -> Result<()> {
    let mut tx = auth
        .pool
        .begin()
        .await
        .context("begin MFA encryption rotation")?;
    sqlx::query("UPDATE account_mfa SET required=required")
        .execute(&mut *tx)
        .await
        .context("lock encrypted MFA state")?;
    prepare_key(auth, &mut tx).await?;
    let mut ring = key_file(&auth.mfa_key_file)?;
    ensure!(ring.keys.len() < 128, "MFA keyring capacity reached");
    let (id, key) = fresh_key();
    ring.keys.insert(id.clone(), key);
    ring.active = id;
    // Publish a ring retaining old keys BEFORE rewrapping. An interrupted transaction stays decryptable.
    write_keys(&auth.mfa_key_file, &ring, true)?;
    let rows:Vec<(String,Option<String>,Option<String>)>=sqlx::query_as("SELECT account_id,secret,pending_secret FROM account_mfa WHERE secret IS NOT NULL OR pending_secret IS NOT NULL").fetch_all(&mut *tx).await.context("load MFA ciphertext for rotation")?;
    for (account, secret, pending) in rows {
        let secret = secret
            .map(|v| {
                open(&ring, &account, "totp", &v).and_then(|b| seal(&ring, &account, "totp", &b))
            })
            .transpose()?;
        let pending = pending
            .map(|v| {
                open(&ring, &account, "pending", &v)
                    .and_then(|b| seal(&ring, &account, "pending", &b))
            })
            .transpose()?;
        sqlx::query("UPDATE account_mfa SET secret=?,pending_secret=? WHERE account_id=?")
            .bind(secret)
            .bind(pending)
            .bind(account)
            .execute(&mut *tx)
            .await
            .context("rewrap MFA secret")?;
    }
    auth::insert_security_event(&mut tx, None, SecurityEvent::MfaKeyRotated).await?;
    tx.commit().await.context("commit MFA encryption rotation")
}

pub(crate) fn default_key_path(state: &Path) -> PathBuf {
    state.with_extension("mfa-keys.json")
}

#[cfg(test)]
mod tests;
