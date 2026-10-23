//! OIDC configuration and optional provider discovery; neither enables authentication.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::path::Path;
use webauthn_rs::prelude::Url;

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "back-channel verifier is gated pending HTTP/runtime integration"
    )
)]
mod backchannel;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "browser bridge is gated pending local MFA/session/logout integration"
    )
)]
mod browser;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "runtime contexts remain gated pending production OIDC routing"
    )
)]
mod context;
mod deployment;
mod discovery;
pub(crate) mod links;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "RP logout is gated pending browser routing and back-channel logout"
    )
)]
mod logout;
mod preparation;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "durable OIDC logout remains gated pending HTTP/runtime integration"
    )
)]
pub(crate) mod sessions;
// Kept unreachable from production sign-in until session/MFA/logout integration is complete.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "OIDC protocol engine is gated pending browser/session/logout integration"
    )
)]
mod protocol;

pub(crate) async fn discover_file(path: &Path) -> Result<()> {
    let config = load_file(path)?;
    discovery::discover(&config).await
}

pub(crate) async fn prepare_file(path: &Path) -> Result<()> {
    let config = load_file(path)?;
    preparation::prepare(&config).await.map(|_| ())
}

pub(crate) async fn prepare_deployment_file(
    path: &Path,
    public_url: Option<&str>,
    secure_cookies: bool,
    state_database: &Path,
) -> Result<()> {
    let config = load_file(path)?;
    deployment::check(&config, public_url, secure_cookies, false, state_database).await?;
    preparation::prepare(&config).await.map(|_| ())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    issuer: String,
    public_url: String,
    client_id: String,
    client_authentication: ClientAuthentication,
    account_linking: AccountLinking,
    permissions: Permissions,
    logout: Logout,
    local_break_glass: bool,
}

#[derive(Deserialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
enum ClientAuthentication {
    None,
    ClientSecretBasic { secret_file: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum AccountLinking {
    AdministratorProvisionedIssuerSubject,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Permissions {
    LocalOnly,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Logout {
    RpInitiatedAndBackChannelRequired,
}

pub(crate) fn check_file(path: &Path) -> Result<()> {
    load_file(path).map(|_| ())
}

fn load_file(path: &Path) -> Result<Configuration> {
    let bytes = std::fs::read(path).context("read OIDC configuration")?;
    // Never include supplied values or serde errors (which can echo secrets) in diagnostics.
    let config: Configuration = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid OIDC configuration structure; see docs/oidc.md"))?;
    validate(&config)?;
    Ok(config)
}

fn https_url(value: &str, origin_only: bool) -> Result<()> {
    let url = Url::parse(value).map_err(|_| anyhow::anyhow!("invalid OIDC HTTPS URL"))?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && (!origin_only || url.path() == "/"),
        "OIDC URLs require HTTPS without credentials, query, or fragment; public_url must be an origin"
    );
    Ok(())
}

fn validate(config: &Configuration) -> Result<()> {
    https_url(&config.issuer, false)?;
    https_url(&config.public_url, true)?;
    ensure!(
        !config.client_id.trim().is_empty()
            && config.client_id.len() <= 1024
            && !config.client_id.chars().any(char::is_control),
        "OIDC client_id must be nonempty, bounded, and contain no control characters"
    );
    if let ClientAuthentication::ClientSecretBasic { secret_file } = &config.client_authentication {
        ensure!(
            Path::new(secret_file).is_absolute(),
            "OIDC client secret file must be an absolute path"
        );
    }
    ensure!(
        config.local_break_glass,
        "OIDC requires local break-glass access"
    );
    let (
        AccountLinking::AdministratorProvisionedIssuerSubject,
        Permissions::LocalOnly,
        Logout::RpInitiatedAndBackChannelRequired,
    ) = (&config.account_linking, &config.permissions, &config.logout);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn configuration() -> serde_json::Value {
        serde_json::json!({
            "issuer": "https://identity.example.test/tenant",
            "public_url": "https://lens.example.test",
            "client_id": "synthetic-client",
            "client_authentication": {"method": "none"},
            "account_linking": "administrator_provisioned_issuer_subject",
            "permissions": "local_only",
            "logout": "rp_initiated_and_back_channel_required",
            "local_break_glass": true
        })
    }

    fn check(value: &serde_json::Value) -> Result<()> {
        let file = tempfile::NamedTempFile::new()?;
        std::fs::write(file.path(), serde_json::to_vec(value)?)?;
        check_file(file.path())
    }

    #[test]
    fn offline_configuration_accepts_explicit_safe_policies() -> Result<()> {
        check(&configuration())?;
        let mut value = configuration();
        value["client_authentication"] = serde_json::json!({
            "method": "client_secret_basic", "secret_file": "/nonexistent/secret"
        });
        // Offline validation does not read secrets or contact the provider.
        check(&value)
    }

    #[test]
    fn configuration_rejects_missing_unknown_and_unsafe_settings() {
        let base = configuration();
        for key in base.as_object().unwrap().keys() {
            let mut value = base.clone();
            value.as_object_mut().unwrap().remove(key);
            assert!(check(&value).is_err(), "missing {key} must fail closed");
        }
        for (key, replacement) in [
            ("issuer", serde_json::json!("http://identity.example.test")),
            (
                "issuer",
                serde_json::json!("https://user:secret@identity.example.test"),
            ),
            (
                "issuer",
                serde_json::json!("https://identity.example.test/?query=1"),
            ),
            (
                "public_url",
                serde_json::json!("https://lens.example.test/path"),
            ),
            ("client_id", serde_json::json!(" ")),
            ("account_linking", serde_json::json!("email")),
            ("permissions", serde_json::json!("trust_claims")),
            ("logout", serde_json::json!("local_only")),
            ("local_break_glass", serde_json::json!(false)),
            (
                "client_authentication",
                serde_json::json!({"method":"client_secret_basic", "secret_file":"relative"}),
            ),
        ] {
            let mut value = base.clone();
            value[key] = replacement;
            assert!(check(&value).is_err(), "unsafe {key} must fail closed");
        }
        let mut value = base;
        value["client_secret"] = serde_json::json!("synthetic-secret-marker");
        let error = check(&value).unwrap_err().to_string();
        assert!(!error.contains("synthetic-secret-marker"));
    }
}
