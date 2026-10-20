//! Internal code-flow engine. No browser route or runtime activation is enabled here.
use super::{ClientAuthentication, Configuration};
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::{
    AccessTokenHash, AsyncHttpClient, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    EndpointNotSet, EndpointSet, HttpRequest, HttpResponse, IssuerUrl, Nonce, OAuth2TokenResponse,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, TokenResponse,
    core::{
        CoreAuthenticationFlow, CoreClient, CoreJsonWebKey, CoreJsonWebKeySet,
        CoreJwsSigningAlgorithm, CoreProviderMetadata,
    },
};
use std::{
    collections::HashMap,
    future::Future,
    io,
    pin::Pin,
    sync::Mutex,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;

const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
const MAX_PENDING: usize = 1024;
const CHALLENGE_LIFETIME: Duration = Duration::from_secs(300);
type Client = CoreClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

struct Pending {
    state: CsrfToken,
    nonce: Nonce,
    verifier: PkceCodeVerifier,
    expires: Instant,
}

pub(super) struct Protocol {
    client: Client,
    issuer: String,
    client_id: String,
    end_session_endpoint: String,
    backchannel: super::backchannel::Verifier,
    pending: Mutex<HashMap<[u8; 32], Pending>>,
}

// Deliberately no Debug/Serialize: callers must not log or persist callback credentials.
pub(super) struct VerifiedIdentity {
    pub(super) issuer: String,
    pub(super) client_id: String,
    pub(super) subject: String,
    pub(super) id_token_hint: String,
    pub(super) issued_at: i64,
    pub(super) session_id: Option<String>,
}

fn suitable_key(key: &CoreJsonWebKey) -> bool {
    let Ok(value) = serde_json::to_value(key) else {
        return false;
    };
    if value["kty"] != "RSA" {
        return false;
    }
    if value.get("alg").is_some_and(|alg| alg != "RS256") {
        return false;
    }
    if value.get("use").is_some_and(|usage| usage != "sig") {
        return false;
    }
    let Some(modulus) = value["n"]
        .as_str()
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
    else {
        return false;
    };
    let Some(first) = modulus.first().filter(|value| **value != 0) else {
        return false;
    };
    let bits = modulus.len() * 8 - first.leading_zeros() as usize;
    (2048..=8192).contains(&bits)
}

fn binding_key(binding: &str) -> Result<[u8; 32]> {
    ensure!(
        !binding.is_empty() && binding.len() <= 256 && !binding.chars().any(char::is_control),
        "invalid OIDC browser binding"
    );
    Ok(blake3::derive_key(
        "agent-lens-oidc-browser-binding-v1",
        binding.as_bytes(),
    ))
}

impl Protocol {
    pub(super) async fn apply_logout(
        &self,
        auth: &crate::auth::AuthState,
        token: &str,
    ) -> Result<u64> {
        let verified = self.verify_logout(token)?;
        super::sessions::apply(auth, &verified).await
    }
    pub(super) fn verify_logout(&self, token: &str) -> Result<super::backchannel::VerifiedLogout> {
        self.backchannel.verify(token)
    }
    pub(super) fn logout_endpoint(&self) -> &str {
        &self.end_session_endpoint
    }
    pub(super) fn matches_configuration(&self, config: &Configuration) -> bool {
        self.issuer == config.issuer
            && self.client_id == config.client_id
            && self.client.redirect_uri().is_some_and(|redirect| {
                redirect.as_str()
                    == format!(
                        "{}/login/oidc/callback",
                        config.public_url.trim_end_matches('/')
                    )
            })
    }

    /// Runtime activation must fetch both documents over bounded verified HTTPS and load
    /// any confidential secret from owner-only storage before constructing this engine.
    pub(super) fn from_documents(
        config: &Configuration,
        discovery: &[u8],
        jwks: &[u8],
        secret: Option<ClientSecret>,
    ) -> Result<Self> {
        super::validate(config)?;
        ensure!(
            discovery.len() <= MAX_DOCUMENT_BYTES && jwks.len() <= MAX_DOCUMENT_BYTES,
            "OIDC provider document exceeds size limit"
        );
        super::discovery::validate_metadata(discovery, config)?;
        #[derive(serde::Deserialize)]
        struct LogoutMetadata {
            end_session_endpoint: String,
        }
        let logout: LogoutMetadata = serde_json::from_slice(discovery)
            .map_err(|_| anyhow::anyhow!("invalid OIDC logout metadata"))?;
        match (&config.client_authentication, &secret) {
            (ClientAuthentication::None, None) => {}
            (ClientAuthentication::ClientSecretBasic { .. }, Some(secret)) => ensure!(
                !secret.secret().is_empty()
                    && secret.secret().len() <= 4096
                    && !secret.secret().chars().any(char::is_control),
                "invalid OIDC confidential client secret"
            ),
            _ => anyhow::bail!("OIDC client authentication configuration and secret do not match"),
        }
        let metadata: CoreProviderMetadata = serde_json::from_slice(discovery)
            .map_err(|_| anyhow::anyhow!("invalid OIDC provider document"))?;
        let keys: CoreJsonWebKeySet = serde_json::from_slice(jwks)
            .map_err(|_| anyhow::anyhow!("invalid OIDC public key set"))?;
        ensure!(
            keys.keys().len() <= 32,
            "OIDC public key count exceeds limit"
        );
        let keys: Vec<_> = keys
            .keys()
            .iter()
            .filter(|key| suitable_key(key))
            .cloned()
            .collect();
        ensure!(
            !keys.is_empty(),
            "OIDC provider has no suitable RS256 signing keys"
        );
        let backchannel = super::backchannel::Verifier::new(config, keys.clone());
        let mut client = CoreClient::new(
            ClientId::new(config.client_id.clone()),
            IssuerUrl::new(config.issuer.clone())
                .map_err(|_| anyhow::anyhow!("invalid OIDC issuer"))?,
            CoreJsonWebKeySet::new(keys),
        )
        .set_auth_uri(metadata.authorization_endpoint().clone())
        .set_token_uri(
            metadata
                .token_endpoint()
                .cloned()
                .context("OIDC token endpoint missing")?,
        )
        .set_redirect_uri(
            RedirectUrl::new(format!(
                "{}/login/oidc/callback",
                config.public_url.trim_end_matches('/')
            ))
            .map_err(|_| anyhow::anyhow!("invalid OIDC redirect URI"))?,
        );
        if let Some(secret) = secret {
            client = client.set_client_secret(secret);
        }
        Ok(Self {
            client,
            issuer: config.issuer.clone(),
            client_id: config.client_id.clone(),
            end_session_endpoint: logout.end_session_endpoint,
            backchannel,
            pending: Mutex::new(HashMap::new()),
        })
    }

    /// Binding must come from an opaque server-owned browser session, never a form/query value.
    pub(super) fn begin(&self, binding: &str) -> Result<String> {
        let binding = binding_key(binding)?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state, nonce) = self
            .client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                || CsrfToken::new_random_len(32),
                || Nonce::new_random_len(32),
            )
            .set_pkce_challenge(challenge)
            .add_extra_param("response_mode", "query")
            .url();
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("OIDC transaction store unavailable"))?;
        pending.retain(|_, value| value.expires > Instant::now());
        ensure!(
            pending.contains_key(&binding) || pending.len() < MAX_PENDING,
            "OIDC pending transaction limit reached"
        );
        pending.insert(
            binding,
            Pending {
                state,
                nonce,
                verifier,
                expires: Instant::now() + CHALLENGE_LIFETIME,
            },
        );
        Ok(url.to_string())
    }

    pub(super) async fn finish<'c, C: AsyncHttpClient<'c>>(
        &'c self,
        binding: &str,
        state: &str,
        code: &str,
        http: &'c C,
    ) -> Result<VerifiedIdentity> {
        let binding = binding_key(binding)?;
        // Remove under the mutex before any await: even failed exchange/validation consumes it.
        let pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("OIDC transaction store unavailable"))?
            .remove(&binding)
            .context("OIDC transaction unavailable")?;
        ensure!(pending.expires > Instant::now(), "OIDC transaction expired");
        ensure!(
            state.len() <= 128
                && bool::from(pending.state.secret().as_bytes().ct_eq(state.as_bytes())),
            "OIDC state validation failed"
        );
        ensure!(
            !code.is_empty() && code.len() <= 4096 && !code.chars().any(char::is_control),
            "invalid OIDC authorization code"
        );
        let response = self
            .client
            .exchange_code(AuthorizationCode::new(code.to_owned()))
            .set_pkce_verifier(pending.verifier)
            .request_async(http)
            .await
            .map_err(|_| anyhow::anyhow!("OIDC authorization code exchange failed"))?;
        let token = response
            .id_token()
            .context("OIDC response missing ID token")?;
        let verifier = self
            .client
            .id_token_verifier()
            .set_allowed_algs([CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256])
            .set_issue_time_verifier_fn(|issued| {
                let age = chrono::Utc::now()
                    .signed_duration_since(issued)
                    .num_seconds();
                if (-60..=600).contains(&age) {
                    Ok(())
                } else {
                    Err("ID token issue time outside allowed window".into())
                }
            });
        let claims = token
            .claims(&verifier, &pending.nonce)
            .map_err(|_| anyhow::anyhow!("OIDC ID token validation failed"))?;
        ensure!(
            claims.issuer().as_str() == self.issuer,
            "OIDC ID token issuer mismatch"
        );
        ensure!(
            claims.audiences().len() == 1 && claims.audiences()[0].as_str() == self.client_id,
            "OIDC ID token audience mismatch"
        );
        ensure!(
            claims
                .authorized_party()
                .is_none_or(|party| party.as_str() == self.client_id),
            "OIDC ID token authorized party mismatch"
        );
        if let Some(expected) = claims.access_token_hash() {
            let actual = AccessTokenHash::from_token(
                response.access_token(),
                token
                    .signing_alg()
                    .map_err(|_| anyhow::anyhow!("invalid OIDC signing algorithm"))?,
                token
                    .signing_key(&verifier)
                    .map_err(|_| anyhow::anyhow!("invalid OIDC signing key"))?,
            )
            .map_err(|_| anyhow::anyhow!("invalid OIDC access token hash"))?;
            ensure!(&actual == expected, "OIDC access token hash mismatch");
        }
        super::links::validate_identity(&self.issuer, claims.subject().as_str())?;
        let encoded = serde_json::to_value(token)
            .map_err(|_| anyhow::anyhow!("encode OIDC logout hint failed"))?;
        let id_token_hint = encoded
            .as_str()
            .context("invalid OIDC logout hint encoding")?
            .to_owned();
        let session_id = super::sessions::session_id(&id_token_hint)?;
        Ok(VerifiedIdentity {
            issuer: self.issuer.clone(),
            client_id: self.client_id.clone(),
            subject: claims.subject().as_str().to_owned(),
            id_token_hint,
            issued_at: claims.issue_time().timestamp(),
            session_id,
        })
    }
}

/// Maintained-library HTTP adapter with the same no-redirect/no-proxy TLS policy as discovery.
pub(super) struct BoundedHttp(reqwest::Client);
impl BoundedHttp {
    pub(super) fn new() -> Result<Self> {
        super::discovery::client_builder(Duration::from_secs(10))
            .build()
            .map(Self)
            .map_err(|_| anyhow::anyhow!("initialize OIDC protocol HTTPS client"))
    }
    async fn request(&self, request: HttpRequest) -> Result<HttpResponse> {
        let (parts, body) = request.into_parts();
        ensure!(
            body.len() <= 32 * 1024,
            "OIDC protocol request exceeds size limit"
        );
        let mut response = self
            .0
            .request(parts.method, parts.uri.to_string())
            .headers(parts.headers)
            .body(body)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("OIDC protocol HTTPS request failed"))?;
        ensure!(
            response.status() == reqwest::StatusCode::OK,
            "OIDC protocol requires HTTP 200"
        );
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .unwrap_or("")
            .trim();
        ensure!(
            content_type.eq_ignore_ascii_case("application/json"),
            "OIDC protocol requires application/json"
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|size| size <= MAX_DOCUMENT_BYTES as u64),
            "OIDC protocol response exceeds size limit"
        );
        let mut builder = axum::http::Response::builder().status(response.status());
        *builder
            .headers_mut()
            .context("build OIDC protocol response")? = response.headers().clone();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow::anyhow!("read OIDC protocol response failed"))?
        {
            ensure!(
                chunk.len() <= MAX_DOCUMENT_BYTES - bytes.len(),
                "OIDC protocol response exceeds size limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        builder.body(bytes).context("build OIDC protocol response")
    }
}
impl<'c> AsyncHttpClient<'c> for BoundedHttp {
    type Error = io::Error;
    type Future =
        Pin<Box<dyn Future<Output = std::result::Result<HttpResponse, io::Error>> + Send + 'c>>;
    fn call(&'c self, request: HttpRequest) -> Self::Future {
        Box::pin(async move {
            self.request(request)
                .await
                .map_err(|_| io::Error::other("OIDC protocol HTTPS request failed"))
        })
    }
}

#[cfg(test)]
mod tests;
