//! Gated RP-initiated form-POST preparation. Local revocation is the caller's first step.
use super::{Configuration, protocol::VerifiedIdentity};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;

const MAX_PENDING: usize = 1024;
const LIFETIME: Duration = Duration::from_secs(300);

// No Debug/Serialize: the hint is a provider credential, not a loggable redirect URL.
pub(super) struct LogoutRequest {
    pub(super) endpoint: String,
    pub(super) id_token_hint: String,
    pub(super) client_id: String,
    pub(super) post_logout_redirect_uri: String,
    pub(super) state: String,
}

impl LogoutRequest {
    pub(super) fn response(&self) -> Result<axum::response::Response> {
        use askama::Template;
        use axum::{
            http::header,
            response::{Html, IntoResponse},
        };
        #[derive(Template)]
        #[template(
            source = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Signed out locally</title></head><body><h1>Signed out locally</h1><p>Continue to sign out at your identity provider.</p><form method=\"post\" action=\"{{ request.endpoint }}\"><input type=\"hidden\" name=\"id_token_hint\" value=\"{{ request.id_token_hint }}\"><input type=\"hidden\" name=\"client_id\" value=\"{{ request.client_id }}\"><input type=\"hidden\" name=\"post_logout_redirect_uri\" value=\"{{ request.post_logout_redirect_uri }}\"><input type=\"hidden\" name=\"state\" value=\"{{ request.state }}\"><button type=\"submit\">Continue provider sign-out</button></form><a href=\"/login\">Return to local sign-in</a></body></html>",
            ext = "html"
        )]
        struct Form<'a> {
            request: &'a LogoutRequest,
        }
        let origin = webauthn_rs::prelude::Url::parse(&self.endpoint)
            .map_err(|_| anyhow::anyhow!("invalid OIDC logout endpoint"))?
            .origin()
            .ascii_serialization();
        let csp = format!(
            "default-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action {origin}"
        )
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid OIDC logout security policy"))?;
        let body = Form { request: self }
            .render()
            .map_err(|_| anyhow::anyhow!("render OIDC logout form"))?;
        let mut response = Html(body).into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-store".parse()?);
        response
            .headers_mut()
            .insert(header::REFERRER_POLICY, "no-referrer".parse()?);
        response
            .headers_mut()
            .insert(header::CONTENT_SECURITY_POLICY, csp);
        Ok(response)
    }
}

struct Pending {
    state: String,
    expires: Instant,
}

pub(super) struct Logout {
    issuer: String,
    client_id: String,
    endpoint: String,
    callback: String,
    pending: Mutex<HashMap<[u8; 32], Pending>>,
}

fn binding_key(binding: &str) -> Result<[u8; 32]> {
    ensure!(
        !binding.is_empty() && binding.len() <= 256 && !binding.chars().any(char::is_control),
        "invalid OIDC logout binding"
    );
    Ok(blake3::derive_key(
        "agent-lens-oidc-logout-binding-v1",
        binding.as_bytes(),
    ))
}

impl Logout {
    pub(super) fn new(config: &Configuration, endpoint: &str) -> Result<Self> {
        super::validate(config)?;
        super::https_url(endpoint, false)?;
        Ok(Self {
            issuer: config.issuer.clone(),
            client_id: config.client_id.clone(),
            endpoint: endpoint.into(),
            callback: format!(
                "{}/login/oidc/logout/callback",
                config.public_url.trim_end_matches('/')
            ),
            pending: Mutex::new(HashMap::new()),
        })
    }

    pub(super) fn begin(&self, binding: &str, identity: VerifiedIdentity) -> Result<LogoutRequest> {
        let key = binding_key(binding)?;
        ensure!(
            identity.issuer == self.issuer,
            "OIDC logout issuer mismatch"
        );
        ensure!(
            identity.client_id == self.client_id,
            "OIDC logout client mismatch"
        );
        ensure!(
            !identity.id_token_hint.is_empty()
                && identity.id_token_hint.len() <= 256 * 1024
                && !identity.id_token_hint.chars().any(char::is_control),
            "invalid OIDC logout hint"
        );
        let state = crate::auth::random_token();
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("OIDC logout transaction store unavailable"))?;
        pending.retain(|_, value| value.expires > Instant::now());
        ensure!(
            pending.contains_key(&key) || pending.len() < MAX_PENDING,
            "OIDC logout transaction limit reached"
        );
        pending.insert(
            key,
            Pending {
                state: state.clone(),
                expires: Instant::now() + LIFETIME,
            },
        );
        Ok(LogoutRequest {
            endpoint: self.endpoint.clone(),
            id_token_hint: identity.id_token_hint,
            client_id: self.client_id.clone(),
            post_logout_redirect_uri: self.callback.clone(),
            state,
        })
    }

    pub(super) fn finish(&self, binding: &str, state: &str) -> Result<()> {
        let key = binding_key(binding)?;
        let pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("OIDC logout transaction store unavailable"))?
            .remove(&key)
            .context("OIDC logout transaction unavailable")?;
        ensure!(
            pending.expires > Instant::now(),
            "OIDC logout transaction expired"
        );
        ensure!(
            state.len() <= 128 && bool::from(pending.state.as_bytes().ct_eq(state.as_bytes())),
            "OIDC logout state validation failed"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Configuration {
        serde_json::from_value(crate::oidc::tests::configuration()).unwrap()
    }
    fn identity() -> VerifiedIdentity {
        VerifiedIdentity {
            issuer: config().issuer,
            client_id: config().client_id,
            subject: "synthetic-subject".into(),
            id_token_hint: "synthetic-private-hint".into(),
            issued_at: crate::auth::unix_now(),
            session_id: None,
        }
    }

    #[test]
    fn logout_endpoints_hints_and_identity_configuration_fail_closed() {
        for endpoint in [
            "http://identity.example.test/logout",
            "https://user@identity.example.test/logout",
            "https://identity.example.test/logout?q=1",
        ] {
            assert!(Logout::new(&config(), endpoint).is_err());
        }
        let logout = Logout::new(&config(), "https://identity.example.test/logout").unwrap();
        for field in ["issuer", "client", "empty", "oversized", "control"] {
            let mut identity = identity();
            match field {
                "issuer" => identity.issuer.push('/'),
                "client" => identity.client_id.push('-'),
                "empty" => identity.id_token_hint.clear(),
                "oversized" => identity.id_token_hint = "a".repeat(256 * 1024 + 1),
                _ => identity.id_token_hint.push('\n'),
            }
            assert!(
                logout.begin("browser", identity).is_err(),
                "{field} must reject logout preparation"
            );
        }
        assert!(logout.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn logout_form_escapes_hidden_values_instead_of_creating_active_markup() {
        let logout = Logout::new(&config(), "https://identity.example.test/logout").unwrap();
        let mut identity = identity();
        identity.id_token_hint = "\"><script>synthetic-private-marker</script>".into();
        let response = logout
            .begin("browser", identity)
            .unwrap()
            .response()
            .unwrap();
        let html = String::from_utf8(
            axum::body::to_bytes(response.into_body(), 512 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("synthetic-private-marker"));
        assert!(
            !html.contains("<script>"),
            "hidden provider values must be HTML-escaped"
        );
        assert!(!html.contains("value=\"\"><script>"));
    }

    #[test]
    fn logout_state_is_browser_bound_one_use_expiring_bounded_and_lost_on_restart() {
        let logout = Logout::new(&config(), "https://identity.example.test/logout").unwrap();
        let request = logout.begin("browser", identity()).unwrap();
        assert_eq!(request.state.len(), 43);
        assert!(logout.finish("other-browser", &request.state).is_err());
        logout.finish("browser", &request.state).unwrap();
        assert!(logout.finish("browser", &request.state).is_err());
        let request = logout.begin("browser", identity()).unwrap();
        assert!(logout.finish("browser", "wrong-state").is_err());
        assert!(logout.finish("browser", &request.state).is_err());
        let old = logout.begin("browser", identity()).unwrap();
        let replacement = logout.begin("browser", identity()).unwrap();
        assert_ne!(old.state, replacement.state);
        assert!(logout.finish("browser", &old.state).is_err());
        let request = logout.begin("browser", identity()).unwrap();
        logout
            .pending
            .lock()
            .unwrap()
            .get_mut(&binding_key("browser").unwrap())
            .unwrap()
            .expires = Instant::now();
        assert!(logout.finish("browser", &request.state).is_err());
        let request = logout.begin("browser", identity()).unwrap();
        let restarted = Logout::new(&config(), "https://identity.example.test/logout").unwrap();
        assert!(restarted.finish("browser", &request.state).is_err());
        logout.pending.lock().unwrap().clear();
        for i in 0..MAX_PENDING {
            logout.begin(&format!("browser-{i}"), identity()).unwrap();
        }
        assert!(logout.begin("overflow", identity()).is_err());
        logout.begin("browser-0", identity()).unwrap();
        logout
            .pending
            .lock()
            .unwrap()
            .values_mut()
            .for_each(|entry| entry.expires = Instant::now());
        logout.begin("after-expiry", identity()).unwrap();
        assert_eq!(logout.pending.lock().unwrap().len(), 1);
        for binding in ["", "\n", &"a".repeat(257)] {
            assert!(logout.begin(binding, identity()).is_err());
        }
    }
}
