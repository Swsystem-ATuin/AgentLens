//! Session-bound browser bridge. No production routes or authentication activation yet.
use super::{
    Configuration,
    protocol::{Protocol, VerifiedIdentity},
};
use crate::{
    auth::{self, AuthState, CurrentUser},
    passkeys,
};
use anyhow::{Result, ensure};
use axum::http::{HeaderMap, header};
use openidconnect::AsyncHttpClient;
use std::time::{Duration, Instant};
use tower_sessions::Session;
use webauthn_rs::prelude::{PublicKeyCredential, RequestChallengeResponse, Url};

const BINDING: &str = "oidc.browser_binding";
const LOGOUT_BINDING: &str = "oidc.logout_binding";

pub(super) struct BrowserFlow {
    protocol: Protocol,
    origin: String,
    logout: super::logout::Logout,
}

// Deliberately neither Debug nor serializable. This is NOT an authenticated user/session.
pub(super) struct Candidate {
    pub(super) user: CurrentUser,
    pub(super) link_id: String,
    pub(super) identity: VerifiedIdentity,
    browser_id: String,
    origin: String,
    pub(super) expires: Instant,
    factor_binding: String,
}

pub(super) struct PasskeyAssertion {
    pub(super) ticket: String,
    pub(super) credential: PublicKeyCredential,
}

pub(super) struct Completion {
    pub(super) user: CurrentUser,
    pub(super) enrollment_required: bool,
    pub(super) identity: VerifiedIdentity,
    inventory_id: String,
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
    async fn guard(
        &self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
    ) -> Result<()> {
        request(&self.origin, session, headers, csrf).await?;
        ensure!(
            Instant::now() < self.expires
                && session
                    .id()
                    .is_some_and(|id| id.to_string() == self.browser_id),
            "OIDC completion unavailable; restart sign-in"
        );
        BrowserFlow::anonymous(auth, session).await
    }

    async fn passkey_guard(
        &self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
    ) -> Result<()> {
        self.guard(auth, session, headers, csrf).await?;
        ensure!(
            auth.passkeys.origin() == Some(self.origin.as_str()),
            "OIDC passkeys require the same configured HTTPS origin"
        );
        Ok(())
    }

    pub(super) async fn start_passkey(
        &self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
        password: &str,
    ) -> Result<passkeys::Challenge<RequestChallengeResponse>> {
        self.passkey_guard(auth, session, headers, csrf).await?;
        ensure!(
            auth.accept_password_attempt(),
            "OIDC passkey password verification rate limited"
        );
        let verified = auth.authenticate(&self.user.username, password).await?;
        let Some((user, generation)) = verified.filter(|(user, generation)| {
            user.id == self.user.id && *generation == self.user.credential_generation
        }) else {
            auth::record_failed_login(auth, &self.user.username).await?;
            anyhow::bail!("OIDC passkey password verification failed");
        };
        let current = auth
            .oidc_account(&self.identity.issuer, &self.identity.subject)
            .await?;
        ensure!(
            current.is_some_and(|(current, link)| current.id == user.id
                && current.credential_generation == generation
                && link == self.link_id),
            "OIDC local identity changed; restart sign-in"
        );
        passkeys::start_authentication(auth, &user, &self.factor_binding, true).await
    }

    pub(super) async fn complete_passkey(
        self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
        assertion: PasskeyAssertion,
    ) -> Result<Option<Completion>> {
        self.passkey_guard(auth, session, headers, csrf).await?;
        let proof = passkeys::finish_oidc_authentication(
            auth,
            &assertion.ticket,
            &self.factor_binding,
            assertion.credential,
        )
        .await?;
        self.complete(auth, session, headers, csrf, &proof).await
    }

    // Consumed by value: one completion attempt, never a reusable authenticated capability.
    pub(super) async fn complete(
        self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
        factor: &str,
    ) -> Result<Option<Completion>> {
        self.guard(auth, session, headers, csrf).await?;
        let identity = auth::oidc::Identity {
            account_id: &self.user.id,
            generation: self.user.credential_generation,
            link_id: &self.link_id,
            issuer: &self.identity.issuer,
            subject: &self.identity.subject,
            factor_binding: &self.factor_binding,
            client_id: &self.identity.client_id,
            issued_at: self.identity.issued_at,
            session_id: self.identity.session_id.as_deref(),
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
        match result {
            Some((user, enrollment_required)) => {
                let inventory_id = auth::current_inventory_id(session).await?.ok_or_else(|| {
                    anyhow::anyhow!("OIDC completed session inventory unavailable")
                })?;
                Ok(Some(Completion {
                    user,
                    enrollment_required,
                    identity: self.identity,
                    inventory_id,
                }))
            }
            None => Ok(None),
        }
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
        let logout = super::logout::Logout::new(config, protocol.logout_endpoint())?;
        Ok(Self {
            protocol,
            origin,
            logout,
        })
    }

    pub(super) async fn logout(
        &self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
        completion: Completion,
    ) -> Result<super::logout::LogoutRequest> {
        request(&self.origin, session, headers, csrf).await?;
        ensure!(
            !auth.is_local_mode(),
            "OIDC requires authenticated deployment mode"
        );
        let current = auth.current_user(session).await?;
        let inventory = auth::current_inventory_id(session).await?;
        // Provider failure, cancellation, or a missing/stale context must never preserve local login.
        auth::end_session(auth, session).await?;
        ensure!(
            current.is_some_and(|user| user.id == completion.user.id
                && user.credential_generation == completion.user.credential_generation)
                && inventory.as_deref() == Some(completion.inventory_id.as_str()),
            "OIDC logout context unavailable; local session ended"
        );
        session.cycle_id().await?;
        let binding = auth::random_token();
        session.insert(LOGOUT_BINDING, &binding).await?;
        session.save().await?;
        self.logout.begin(&binding, completion.identity)
    }

    pub(super) async fn finish_logout(&self, session: &Session, state: &str) -> Result<()> {
        let binding = session
            .remove::<String>(LOGOUT_BINDING)
            .await?
            .ok_or_else(|| anyhow::anyhow!("OIDC logout browser transaction unavailable"))?;
        session.save().await?;
        // This callback never establishes or revokes any account session.
        self.logout.finish(&binding, state)
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
            factor_binding: format!("o:{}", auth::random_token()),
        })
    }
}
