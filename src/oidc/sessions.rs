//! Durable provider-session binding and atomic logout replay/revocation. No HTTP activation.
use crate::auth::{self, AuthState, SecurityEvent, oidc::Identity};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sqlx::{Sqlite, Transaction};

pub(crate) const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS oidc_session_bindings (
 inventory_id TEXT PRIMARY KEY REFERENCES account_sessions(id) ON DELETE CASCADE,
 provider_key BLOB NOT NULL CHECK(length(provider_key)=32),
 subject_key BLOB NOT NULL CHECK(length(subject_key)=32),
 session_key BLOB CHECK(session_key IS NULL OR length(session_key)=32),
 issued_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS oidc_session_provider ON oidc_session_bindings(provider_key,subject_key,session_key);
CREATE TABLE IF NOT EXISTS oidc_logout_events (
 replay_key BLOB PRIMARY KEY CHECK(length(replay_key)=32),
 provider_key BLOB NOT NULL CHECK(length(provider_key)=32),
 subject_key BLOB CHECK(subject_key IS NULL OR length(subject_key)=32),
 session_key BLOB CHECK(session_key IS NULL OR length(session_key)=32),
 issued_at INTEGER NOT NULL,
 retain_until INTEGER NOT NULL,
 CHECK(subject_key IS NOT NULL OR session_key IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS oidc_logout_provider ON oidc_logout_events(provider_key,issued_at);
CREATE INDEX IF NOT EXISTS oidc_logout_retention ON oidc_logout_events(retain_until);
"#;

fn digest(kind: &str, values: &[&str]) -> Vec<u8> {
    let mut hash = blake3::Hasher::new_derive_key("agent-lens-oidc-session-selector-v1");
    for value in std::iter::once(kind).chain(values.iter().copied()) {
        hash.update(&(value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    }
    hash.finalize().as_bytes().to_vec()
}
fn selectors(
    issuer: &str,
    client: &str,
    subject: Option<&str>,
    sid: Option<&str>,
) -> (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>) {
    (
        digest("provider", &[issuer, client]),
        subject.map(|value| digest("subject", &[issuer, client, value])),
        sid.map(|value| digest("session", &[issuer, client, value])),
    )
}

pub(super) fn session_id(token: &str) -> Result<Option<String>> {
    #[derive(serde::Deserialize)]
    struct Extra {
        #[serde(default, deserialize_with = "present")]
        sid: Option<String>,
    }
    fn present<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<Option<String>, D::Error> {
        String::deserialize(d).map(Some)
    }
    use serde::Deserialize;
    let payload = token
        .split('.')
        .nth(1)
        .context("invalid verified OIDC token encoding")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| anyhow::anyhow!("invalid verified OIDC token encoding"))?;
    let extra: Extra = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid OIDC provider session claim"))?;
    if let Some(sid) = &extra.sid {
        ensure!(
            !sid.is_empty() && sid.len() <= 1024 && sid.bytes().all(|b| (b' '..=b'~').contains(&b)),
            "invalid OIDC provider session claim"
        );
    }
    Ok(extra.sid)
}

pub(crate) async fn check_login(
    tx: &mut Transaction<'_, Sqlite>,
    identity: &Identity<'_>,
) -> Result<()> {
    let (provider, subject, sid) = selectors(
        identity.issuer,
        identity.client_id,
        Some(identity.subject),
        identity.session_id,
    );
    let logged_out:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oidc_logout_events WHERE provider_key=? AND issued_at>=? AND retain_until>unixepoch() AND (subject_key IS NULL OR subject_key=?) AND (session_key IS NULL OR session_key=?))")
        .bind(provider).bind(identity.issued_at).bind(subject).bind(sid).fetch_one(&mut **tx).await.context("check pending OIDC logout tombstones")?;
    ensure!(
        !logged_out,
        "OIDC provider session logged out; restart provider sign-in"
    );
    Ok(())
}

pub(crate) async fn bind(
    tx: &mut Transaction<'_, Sqlite>,
    inventory: &str,
    identity: &Identity<'_>,
) -> Result<()> {
    let (provider, subject, sid) = selectors(
        identity.issuer,
        identity.client_id,
        Some(identity.subject),
        identity.session_id,
    );
    sqlx::query("INSERT INTO oidc_session_bindings(inventory_id,provider_key,subject_key,session_key,issued_at) VALUES(?,?,?,?,?)")
        .bind(inventory).bind(provider).bind(subject).bind(sid).bind(identity.issued_at).execute(&mut **tx).await.context("bind OIDC provider session inventory")?;
    Ok(())
}

pub(super) async fn apply(
    auth: &AuthState,
    event: &super::backchannel::VerifiedLogout,
) -> Result<u64> {
    let now = auth::unix_now();
    ensure!(
        event.expires > now,
        "OIDC logout token expired before revocation"
    );
    let (provider, subject, sid) = selectors(
        &event.issuer,
        &event.client_id,
        event.subject.as_deref(),
        event.session_id.as_deref(),
    );
    let mut tx = auth
        .pool
        .begin()
        .await
        .context("begin durable OIDC logout")?;
    // A write acquires the SQLite writer before checking replay/capacity or selecting sessions.
    sqlx::query("DELETE FROM oidc_logout_events WHERE retain_until<=?")
        .bind(now)
        .execute(&mut *tx)
        .await
        .context("prune OIDC logout tombstones")?;
    let seen: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oidc_logout_events WHERE replay_key=?)")
            .bind(event.replay_key.as_slice())
            .fetch_one(&mut *tx)
            .await
            .context("check durable OIDC logout replay")?;
    if seen {
        tx.commit().await.context("commit duplicate OIDC logout")?;
        return Ok(0);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM oidc_logout_events")
        .fetch_one(&mut *tx)
        .await
        .context("count retained OIDC logouts")?;
    ensure!(count < 4096, "OIDC logout ledger capacity reached");
    sqlx::query("INSERT INTO oidc_logout_events(replay_key,provider_key,subject_key,session_key,issued_at,retain_until) VALUES(?,?,?,?,?,?)")
        .bind(event.replay_key.as_slice()).bind(&provider).bind(&subject).bind(&sid).bind(event.issued_at).bind(event.expires.max(now.saturating_add(1200))).execute(&mut *tx).await.context("record durable OIDC logout")?;
    let targets:Vec<(String,String,String)>=sqlx::query_as("SELECT s.id,s.tower_session_id,s.account_id FROM account_sessions s JOIN oidc_session_bindings b ON b.inventory_id=s.id WHERE s.revoked_at IS NULL AND b.provider_key=? AND b.issued_at<=? AND (? IS NULL OR b.subject_key=?) AND (? IS NULL OR b.session_key=?) LIMIT 10001")
        .bind(&provider).bind(event.issued_at).bind(&subject).bind(&subject).bind(&sid).bind(&sid).fetch_all(&mut *tx).await.context("select exact OIDC logout sessions")?;
    ensure!(targets.len() <= 10000, "OIDC logout session limit reached");
    let mut accounts = std::collections::BTreeSet::new();
    for (id, tower, account) in &targets {
        sqlx::query("DELETE FROM tower_sessions WHERE id=?")
            .bind(tower)
            .execute(&mut *tx)
            .await
            .context("delete OIDC logout session payload")?;
        sqlx::query("UPDATE account_sessions SET revoked_at=? WHERE id=?")
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await
            .context("revoke exact OIDC logout inventory")?;
        accounts.insert(account);
    }
    for account in accounts {
        auth::insert_security_event(&mut tx, Some(account), SecurityEvent::SessionRevoked).await?;
    }
    tx.commit()
        .await
        .context("commit OIDC replay and session revocation")?;
    Ok(targets.len() as u64)
}
