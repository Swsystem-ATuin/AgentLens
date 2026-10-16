//! Session-bound browser bridge. No production routes or authentication activation yet.
use super::{
    Configuration,
    protocol::{Protocol, VerifiedIdentity},
};
use crate::auth::{self, AuthState, CurrentUser};
use anyhow::{Result, ensure};
use axum::http::{HeaderMap, header};
use openidconnect::AsyncHttpClient;
use std::time::{Duration, Instant};
use tower_sessions::Session;
use webauthn_rs::prelude::Url;

const BINDING: &str = "oidc.browser_binding";

pub(super) struct BrowserFlow {
    protocol: Protocol,
    origin: String,
}

// Deliberately neither Debug nor serializable. This is NOT an authenticated user/session.
pub(super) struct Candidate {
    pub(super) user: CurrentUser,
    pub(super) link_id: String,
    pub(super) identity: VerifiedIdentity,
    browser_id: String,
    origin: String,
    pub(super) expires: Instant,
}

pub(super) struct Completion {
    pub(super) user: CurrentUser,
    pub(super) enrollment_required: bool,
    pub(super) identity: VerifiedIdentity,
}

async fn request(origin: &str, session: &Session, headers: &HeaderMap, csrf: &str) -> Result<()> {
    let mut origins = headers.get_all(header::ORIGIN).iter();
    ensure!(
        origins.next().and_then(|value| value.to_str().ok()) == Some(origin)
            && origins.next().is_none(),
        "invalid OIDC login request"
    );
    ensure!(
        auth::verify_csrf(session, csrf).await?,
        "invalid OIDC login request"
    );
    Ok(())
}

impl Candidate {
    // Consumed by value: one completion attempt, never a reusable authenticated capability.
    pub(super) async fn complete(
        self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
        factor: &str,
    ) -> Result<Option<Completion>> {
        request(&self.origin, session, headers, csrf).await?;
        ensure!(
            Instant::now() < self.expires
                && session
                    .id()
                    .is_some_and(|id| id.to_string() == self.browser_id),
            "OIDC completion unavailable; restart sign-in"
        );
        let identity = auth::oidc::Identity {
            account_id: &self.user.id,
            generation: self.user.credential_generation,
            link_id: &self.link_id,
            issuer: &self.identity.issuer,
            subject: &self.identity.subject,
        };
        let result = auth::oidc::complete(
            auth,
            session,
            identity,
            factor,
            headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok()),
        )
        .await?;
        Ok(result.map(|(user, enrollment_required)| Completion {
            user,
            enrollment_required,
            identity: self.identity,
        }))
    }
}

impl BrowserFlow {
    pub(super) fn new(config: &Configuration, protocol: Protocol) -> Result<Self> {
        super::validate(config)?;
        ensure!(
            protocol.matches_configuration(config),
            "OIDC browser and protocol configuration must match"
        );
        let origin = Url::parse(&config.public_url)
            .map_err(|_| anyhow::anyhow!("invalid OIDC browser origin"))?
            .origin()
            .ascii_serialization();
        Ok(Self { protocol, origin })
    }

    async fn anonymous(auth: &AuthState, session: &Session) -> Result<()> {
        ensure!(
            !auth.is_local_mode(),
            "OIDC requires authenticated deployment mode"
        );
        ensure!(
            auth.current_user(session).await?.is_none(),
            "OIDC account switching requires local logout first"
        );
        Ok(())
    }

    pub(super) async fn begin(
        &self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
    ) -> Result<String> {
        request(&self.origin, session, headers, csrf).await?;
        Self::anonymous(auth, session).await?;
        // The binding is generated locally, never accepted from a form/query/provider claim.
        // Reuse within this anonymous session so starting again replaces its pending transaction.
        let binding = session
            .get::<String>(BINDING)
            .await?
            .unwrap_or_else(auth::random_token);
        session.insert(BINDING, &binding).await?;
        session.save().await?;
        self.protocol.begin(&binding)
    }

    pub(super) async fn finish<'c, C: AsyncHttpClient<'c>>(
        &'c self,
        auth: &AuthState,
        session: &Session,
        state: &str,
        code: &str,
        http: &'c C,
    ) -> Result<Candidate> {
        Self::anonymous(auth, session).await?;
        let binding = session
            .remove::<String>(BINDING)
            .await?
            .ok_or_else(|| anyhow::anyhow!("OIDC browser transaction unavailable"))?;
        // Persist removal before awaiting the provider; the engine also consumes under its mutex.
        session.save().await?;
        let identity = self.protocol.finish(&binding, state, code, http).await?;
        let (user, link_id) = auth
            .oidc_account(&identity.issuer, &identity.subject)
            .await?
            .ok_or_else(|| anyhow::anyhow!("OIDC local identity unavailable"))?;
        Ok(Candidate {
            user,
            link_id,
            identity,
            browser_id: session
                .id()
                .ok_or_else(|| anyhow::anyhow!("OIDC browser session unavailable"))?
                .to_string(),
            origin: self.origin.clone(),
            expires: Instant::now() + Duration::from_secs(300),
        })
    }
}
