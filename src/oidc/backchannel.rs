//! Gated signed-token verification only. No durable replay ledger, revocation, or HTTP route.
use super::Configuration;
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::{
    JsonWebKey,
    core::{CoreJsonWebKey, CoreJwsSigningAlgorithm},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Mutex,
};

const MAX_TOKEN_BYTES: usize = 64 * 1024;
const MAX_REPLAY: usize = 4096;
const EVENT: &str = "http://schemas.openid.net/event/backchannel-logout";

#[derive(Deserialize)]
struct Header {
    alg: String,
    typ: String,
    kid: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

fn present_string<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: Audience,
    iat: i64,
    exp: i64,
    jti: String,
    #[serde(default, deserialize_with = "present_string")]
    sub: Option<String>,
    #[serde(default, deserialize_with = "present_string")]
    sid: Option<String>,
    events: BTreeMap<String, Value>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

// Neither Debug nor serializable. This is not permission to revoke an entire local account.
pub(super) struct VerifiedLogout {
    pub(super) issuer: String,
    pub(super) client_id: String,
    pub(super) subject: Option<String>,
    pub(super) session_id: Option<String>,
    pub(super) replay_key: [u8; 32],
    pub(super) expires: i64,
}

pub(super) struct Verifier {
    issuer: String,
    client_id: String,
    keys: Vec<CoreJsonWebKey>,
    replay: Mutex<HashMap<[u8; 32], i64>>,
}

fn opaque(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.bytes().all(|byte| (b' '..=b'~').contains(&byte))
}

impl Verifier {
    // Only called with the protocol engine's prepared/filtered provider keys.
    pub(super) fn new(config: &Configuration, keys: Vec<CoreJsonWebKey>) -> Self {
        Self {
            issuer: config.issuer.clone(),
            client_id: config.client_id.clone(),
            keys,
            replay: Mutex::new(HashMap::new()),
        }
    }

    pub(super) fn verify(&self, token: &str) -> Result<VerifiedLogout> {
        // One fixed public diagnostic; neither serde/crypto errors nor supplied values escape.
        self.verify_inner(token)
            .map_err(|_| anyhow::anyhow!("OIDC back-channel logout token rejected"))
    }

    fn verify_inner(&self, token: &str) -> Result<VerifiedLogout> {
        ensure!(
            !token.is_empty() && token.len() <= MAX_TOKEN_BYTES,
            "token bound"
        );
        let parts: Vec<_> = token.splitn(4, '.').collect();
        ensure!(
            parts.len() == 3 && parts.iter().all(|part| !part.is_empty()),
            "compact signed JWT required"
        );
        let header: Header = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0])?)?;
        ensure!(
            header.alg == "RS256" && header.typ == "logout+jwt",
            "algorithm/type mismatch"
        );
        ensure!(
            !header.extra.contains_key("crit") && !header.extra.contains_key("b64"),
            "unsupported header semantics"
        );
        if let Some(kid) = &header.kid {
            ensure!(opaque(kid, 256), "key id bound");
        }
        let mut candidates = self.keys.iter().filter(|key| {
            header
                .kid
                .as_ref()
                .is_none_or(|kid| key.key_id().is_some_and(|id| id.as_str() == kid))
        });
        let key = candidates
            .next()
            .ok_or_else(|| anyhow::anyhow!("signing key unavailable"))?;
        ensure!(candidates.next().is_none(), "ambiguous signing key");
        let signature = URL_SAFE_NO_PAD.decode(parts[2])?;
        let signed = &token.as_bytes()[..parts[0].len() + 1 + parts[1].len()];
        key.verify_signature(
            &CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            signed,
            &signature,
        )?;
        let claims: Claims = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1])?)?;
        ensure!(claims.iss == self.issuer, "issuer mismatch");
        let audience = match &claims.aud {
            Audience::One(value) => value == &self.client_id,
            Audience::Many(values) => values.len() == 1 && values[0] == self.client_id,
        };
        ensure!(audience, "audience mismatch");
        ensure!(
            claims
                .extra
                .get("azp")
                .is_none_or(|value| value.as_str() == Some(self.client_id.as_str())),
            "authorized party mismatch"
        );
        let now = crate::auth::unix_now();
        ensure!(
            now.checked_sub(claims.iat)
                .is_some_and(|age| (-60..=600).contains(&age))
                && claims.exp > now
                && claims
                    .exp
                    .checked_sub(claims.iat)
                    .is_some_and(|lifetime| (1..=600).contains(&lifetime)),
            "time window"
        );
        ensure!(opaque(&claims.jti, 255), "token id bound");
        ensure!(
            claims.sub.is_some() || claims.sid.is_some(),
            "missing target"
        );
        if let Some(subject) = &claims.sub {
            super::links::validate_identity(&claims.iss, subject)?;
        }
        if let Some(sid) = &claims.sid {
            ensure!(opaque(sid, 1024), "session id bound");
        }
        ensure!(!claims.extra.contains_key("nonce"), "nonce prohibited");
        ensure!(
            claims.events.len() == 1
                && claims
                    .events
                    .get(EVENT)
                    .is_some_and(|value| value.as_object().is_some_and(|object| object.is_empty())),
            "invalid logout event"
        );
        let mut digest = blake3::Hasher::new_derive_key("agent-lens-oidc-backchannel-replay-v1");
        for value in [&claims.iss, &self.client_id, &claims.jti] {
            digest.update(&(value.len() as u64).to_le_bytes());
            digest.update(value.as_bytes());
        }
        let replay_key = *digest.finalize().as_bytes();
        self.remember(replay_key, claims.exp, now)?;
        Ok(VerifiedLogout {
            issuer: claims.iss,
            client_id: self.client_id.clone(),
            subject: claims.sub,
            session_id: claims.sid,
            replay_key,
            expires: claims.exp,
        })
    }

    fn remember(&self, replay_key: [u8; 32], expires: i64, now: i64) -> Result<()> {
        let mut replay = self
            .replay
            .lock()
            .map_err(|_| anyhow::anyhow!("replay guard unavailable"))?;
        replay.retain(|_, expires| *expires > now);
        ensure!(
            !replay.contains_key(&replay_key) && replay.len() < MAX_REPLAY,
            "replay or capacity limit"
        );
        replay.insert(replay_key, expires);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replay_cache_fails_closed_at_capacity_and_prunes_only_expired_entries() {
        let config = serde_json::from_value(crate::oidc::tests::configuration()).unwrap();
        let verifier = Verifier::new(&config, vec![]);
        for i in 0..MAX_REPLAY {
            let key = *blake3::hash(&i.to_le_bytes()).as_bytes();
            verifier.remember(key, 100, 0).unwrap();
        }
        assert!(verifier.remember([0; 32], 100, 0).is_err());
        let key = *blake3::hash(&0usize.to_le_bytes()).as_bytes();
        assert!(verifier.remember(key, 100, 99).is_err());
        verifier.remember(key, 200, 100).unwrap();
        assert_eq!(verifier.replay.lock().unwrap().len(), 1);
    }
}
