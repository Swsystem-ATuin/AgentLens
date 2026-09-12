use std::path::Path;

use anyhow::{Context, Result, bail};
use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Keep only a digest in application state; never expose credentials through Debug.
#[derive(Clone)]
pub struct Credentials {
    digest: [u8; 32],
}

#[derive(Deserialize)]
struct Config {
    username: String,
    password: String,
}

impl Credentials {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| {
            format!(
                "read authentication file {}; copy auth.example.json and set credentials",
                path.display()
            )
        })?;
        // Do not include serde's error: malformed values may contain secrets.
        let config: Config = serde_json::from_slice(&bytes).map_err(|_| {
            anyhow::anyhow!(
                "authentication file must be a JSON object with string username and password"
            )
        })?;
        if config.username.trim().is_empty()
            || config.password.trim().is_empty()
            || config.username.contains(':')
            || config.username.chars().any(char::is_control)
            || config.password.chars().any(char::is_control)
        {
            bail!(
                "authentication requires a nonempty username and password without control characters; username cannot contain ':'"
            );
        }
        Ok(Self {
            digest: Sha256::digest(format!("{}:{}", config.username, config.password).as_bytes())
                .into(),
        })
    }

    fn accepts(&self, request: &Request) -> bool {
        let mut headers = request.headers().get_all(header::AUTHORIZATION).iter();
        let Some(value) = headers.next().and_then(|value| value.to_str().ok()) else {
            return false;
        };
        if headers.next().is_some() {
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
        bool::from(self.digest.ct_eq(&digest))
    }
}

pub async fn require_auth(
    State(credentials): State<Credentials>,
    mut request: Request,
    next: Next,
) -> Response {
    let mut response = if credentials.accepts(&request) {
        request.headers_mut().remove(header::AUTHORIZATION);
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(
                header::WWW_AUTHENTICATE,
                "Basic realm=\"Agent Lens\", charset=\"UTF-8\"",
            )],
            "Authentication required",
        )
            .into_response()
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accepted_authorization_is_removed_before_downstream_handler() {
        use axum::{Router, body::Body, routing::get};
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        std::fs::write(&path, r#"{"username":"synthetic","password":"password"}"#).unwrap();
        let app = Router::new()
            .route(
                "/",
                get(|request: Request| async move {
                    assert!(!request.headers().contains_key(header::AUTHORIZATION));
                    StatusCode::NO_CONTENT
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                Credentials::load(&path).unwrap(),
                require_auth,
            ));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(
                        header::AUTHORIZATION,
                        format!("Basic {}", STANDARD.encode("synthetic:password")),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response.headers()[header::CONTENT_SECURITY_POLICY],
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
        );
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
    }

    #[test]
    fn configuration_fails_closed_without_disclosing_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.json");
        assert!(Credentials::load(&path).is_err());
        for invalid in [
            r#"{"username":"","password":""}"#,
            r#"{"username":"test","password":" "}"#,
            r#"{"username":"test:name","password":"synthetic"}"#,
            r#"{"username":"test","password":"synthetic\n"}"#,
            r#"{"username":"test"}"#,
            r#"{"username": "test", "password": ["synthetic-secret"]}"#,
            "{",
        ] {
            std::fs::write(&path, invalid).unwrap();
            let error = Credentials::load(&path).err().unwrap().to_string();
            assert!(!error.contains("synthetic"));
        }
        std::fs::write(
            &path,
            r#"{"username":"test","password":"synthetic:password"}"#,
        )
        .unwrap();
        assert!(Credentials::load(&path).is_ok());
    }
}
