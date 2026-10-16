//! Strict discovery gate for the initial code-flow/logout policy, not an authentication proof.
use super::{ClientAuthentication, Configuration, https_url};
use anyhow::{Result, ensure};
use reqwest::{Client, StatusCode, header};
use serde::Deserialize;
use std::time::Duration;

const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

// Unknown extension metadata is deliberately tolerated. Known malformed fields fail closed.
#[derive(Deserialize)]
struct Metadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    end_session_endpoint: String,
    response_types_supported: Vec<String>,
    subject_types_supported: Vec<String>,
    id_token_signing_alg_values_supported: Vec<String>,
    code_challenge_methods_supported: Vec<String>,
    #[serde(default, deserialize_with = "present_list")]
    grant_types_supported: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present_list")]
    response_modes_supported: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present_list")]
    scopes_supported: Option<Vec<String>>,
    #[serde(default, deserialize_with = "present_list")]
    token_endpoint_auth_methods_supported: Option<Vec<String>>,
    #[serde(default)]
    backchannel_logout_supported: bool,
}

fn present_list<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Vec<String>>, D::Error> {
    // Missing optional fields have specification defaults; explicit null is not an array.
    Vec::<String>::deserialize(deserializer).map(Some)
}

pub(super) fn client_builder(timeout: Duration) -> reqwest::ClientBuilder {
    Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
}

pub(super) async fn discover(config: &Configuration) -> Result<()> {
    let client = client_builder(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| anyhow::anyhow!("initialize OIDC discovery HTTPS client"))?;
    fetch(&client, config).await
}

fn discovery_url(issuer: &str) -> String {
    // OIDC Discovery appends beneath the issuer path, unlike OAuth metadata discovery.
    // Trim exactly one terminating slash; preserve the configured issuer for exact matching.
    format!(
        "{}/.well-known/openid-configuration",
        issuer.strip_suffix('/').unwrap_or(issuer)
    )
}

async fn fetch(client: &Client, config: &Configuration) -> Result<()> {
    // Errors intentionally omit remote URLs, response bodies, and serde diagnostics.
    // No client secret, authorization, cookies, or local account data are sent.
    let mut response = client
        .get(discovery_url(&config.issuer))
        .header(header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("OIDC discovery HTTPS request failed"))?;
    ensure!(
        response.status() == StatusCode::OK,
        "OIDC discovery requires HTTP 200; redirects are not followed"
    );
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .unwrap_or("")
        .trim();
    ensure!(
        content_type.eq_ignore_ascii_case("application/json"),
        "OIDC discovery requires application/json"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= MAX_DOCUMENT_BYTES as u64),
        "OIDC discovery document exceeds size limit"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| anyhow::anyhow!("read OIDC discovery response failed"))?
    {
        ensure!(
            chunk.len() <= MAX_DOCUMENT_BYTES - bytes.len(),
            "OIDC discovery document exceeds size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    validate_metadata(&bytes, config)
}

fn contains(values: &[String], value: &str) -> bool {
    values.iter().any(|entry| entry == value)
}

pub(super) fn validate_metadata(bytes: &[u8], config: &Configuration) -> Result<()> {
    let metadata: Metadata = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("invalid OIDC discovery metadata structure"))?;
    ensure!(
        metadata.issuer == config.issuer,
        "OIDC discovery issuer must exactly match configured issuer"
    );
    for endpoint in [
        &metadata.authorization_endpoint,
        &metadata.token_endpoint,
        &metadata.jwks_uri,
        &metadata.end_session_endpoint,
    ] {
        // Initial policy intentionally rejects endpoint queries as well as fragments/credentials.
        https_url(endpoint, false)?;
    }
    ensure!(
        contains(&metadata.response_types_supported, "code"),
        "OIDC provider must support code response type"
    );
    ensure!(
        metadata
            .grant_types_supported
            .as_ref()
            .is_none_or(|values| contains(values, "authorization_code")),
        "OIDC provider must support authorization_code grant"
    );
    ensure!(
        metadata
            .response_modes_supported
            .as_ref()
            .is_none_or(|values| contains(values, "query")),
        "OIDC provider must support query response mode"
    );
    ensure!(
        metadata
            .scopes_supported
            .as_ref()
            .is_none_or(|values| contains(values, "openid")),
        "OIDC provider must support openid scope"
    );
    ensure!(
        metadata
            .subject_types_supported
            .iter()
            .any(|value| matches!(value.as_str(), "public" | "pairwise")),
        "OIDC provider must support a standard subject type"
    );
    ensure!(
        contains(&metadata.id_token_signing_alg_values_supported, "RS256"),
        "OIDC provider must support RS256 ID tokens"
    );
    ensure!(
        contains(&metadata.code_challenge_methods_supported, "S256"),
        "OIDC provider must explicitly advertise S256 PKCE"
    );
    let required_auth = match config.client_authentication {
        ClientAuthentication::None => "none",
        ClientAuthentication::ClientSecretBasic { .. } => "client_secret_basic",
    };
    let supported_auth = metadata
        .token_endpoint_auth_methods_supported
        .unwrap_or_else(|| vec!["client_secret_basic".into()]);
    ensure!(
        contains(&supported_auth, required_auth),
        "OIDC provider does not support configured client authentication method"
    );
    ensure!(
        metadata.backchannel_logout_supported,
        "OIDC provider must advertise back-channel logout support"
    );
    Ok(())
}

#[cfg(test)]
pub(super) mod tests;
