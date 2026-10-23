//! Gated context-owning browser operations. Only opaque handles enter server session payloads.
use super::*;
use crate::oidc::context::{Entry, Store};
use std::sync::Arc;

const CANDIDATE: &str = "oidc.runtime_candidate";
const COMPLETED: &str = "oidc.runtime_logout_context";

pub(in crate::oidc) enum LogoutOutcome {
    Provider(super::super::logout::LogoutRequest),
    LocalOnly,
}

pub(in crate::oidc) struct Runtime {
    flow: BrowserFlow,
    candidates: Store<Candidate>,
    completed: Store<Completion>,
}

fn browser(session: &Session) -> Result<String> {
    session
        .id()
        .map(|id| id.to_string())
        .ok_or_else(|| anyhow::anyhow!("OIDC browser session unavailable"))
}

impl Runtime {
    pub(in crate::oidc) fn new(flow: BrowserFlow) -> Self {
        Self {
            flow,
            candidates: Store::new(),
            completed: Store::new(),
        }
    }

    async fn cancel_pending(&self, session: &Session) -> Result<()> {
        if let Some(handle) = session.remove::<String>(CANDIDATE).await? {
            let _ = self.candidates.cancel(&browser(session)?, &handle);
        }
        session.save().await?;
        Ok(())
    }

    pub(in crate::oidc) async fn begin(
        &self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
    ) -> Result<String> {
        request(&self.flow.origin, session, headers, csrf).await?;
        BrowserFlow::anonymous(auth, session).await?;
        self.cancel_pending(session).await?;
        self.flow.begin(auth, session, headers, csrf).await
    }

    pub(in crate::oidc) async fn callback<'c, C: AsyncHttpClient<'c>>(
        &'c self,
        auth: &AuthState,
        session: &Session,
        state: &str,
        code: &str,
        http: &'c C,
    ) -> Result<()> {
        let candidate = self.flow.finish(auth, session, state, code, http).await?;
        let binding = browser(session)?;
        let expires = candidate.expires;
        let handle = self.candidates.insert(&binding, candidate, expires)?;
        let saved: Result<()> = async {
            session.insert(CANDIDATE, &handle).await?;
            session.save().await?;
            Ok(())
        }
        .await;
        if let Err(error) = saved {
            let _ = self.candidates.cancel(&binding, &handle);
            return Err(error);
        }
        Ok(())
    }

    async fn pending(&self, session: &Session) -> Result<Arc<Entry<Candidate>>> {
        let handle = session
            .get::<String>(CANDIDATE)
            .await?
            .ok_or_else(|| anyhow::anyhow!("OIDC pending sign-in unavailable"))?;
        self.candidates.get(&browser(session)?, &handle)
    }

    pub(in crate::oidc) async fn start_passkey(
        &self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
        password: &str,
    ) -> Result<passkeys::Challenge<RequestChallengeResponse>> {
        let entry = self.pending(session).await?;
        let candidate = entry.value.lock().await;
        entry.check()?;
        let response = candidate
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("OIDC pending sign-in consumed"))?
            .start_passkey(auth, session, headers, csrf, password)
            .await?;
        entry.check()?;
        Ok(response)
    }

    async fn take_pending(
        &self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
    ) -> Result<Candidate> {
        request(&self.flow.origin, session, headers, csrf).await?;
        BrowserFlow::anonymous(auth, session).await?;
        let handle = session
            .remove::<String>(CANDIDATE)
            .await?
            .ok_or_else(|| anyhow::anyhow!("OIDC pending sign-in unavailable"))?;
        session.save().await?;
        self.candidates.take(&browser(session)?, &handle).await
    }

    async fn retain_completion(
        &self,
        auth: &AuthState,
        session: &Session,
        completion: Option<Completion>,
    ) -> Result<Option<(CurrentUser, bool)>> {
        let Some(completion) = completion else {
            return Ok(None);
        };
        let result = (completion.user.clone(), completion.enrollment_required);
        let retained: Result<()> = async {
            let binding = browser(session)?;
            let handle = self.completed.insert(
                &binding,
                completion,
                Instant::now() + Duration::from_secs(crate::auth::ABSOLUTE_HOURS as u64 * 3600),
            )?;
            if let Err(error) = async {
                session.insert(COMPLETED, &handle).await?;
                session.save().await
            }
            .await
            {
                let _ = self.completed.cancel(&binding, &handle);
                return Err(error.into());
            }
            Ok(())
        }
        .await;
        if let Err(error) = retained {
            // Required provider-logout context could not be retained: never hand out a live login.
            auth::end_session(auth, session).await?;
            return Err(error);
        }
        Ok(Some(result))
    }

    pub(in crate::oidc) async fn complete(
        &self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
        factor: &str,
    ) -> Result<Option<(CurrentUser, bool)>> {
        let candidate = self.take_pending(auth, session, headers, csrf).await?;
        let completed = candidate
            .complete(auth, session, headers, csrf, factor)
            .await?;
        self.retain_completion(auth, session, completed).await
    }

    pub(in crate::oidc) async fn complete_passkey(
        &self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
        assertion: PasskeyAssertion,
    ) -> Result<Option<(CurrentUser, bool)>> {
        let candidate = self.take_pending(auth, session, headers, csrf).await?;
        let completed = candidate
            .complete_passkey(auth, session, headers, csrf, assertion)
            .await?;
        self.retain_completion(auth, session, completed).await
    }

    pub(in crate::oidc) async fn logout(
        &self,
        auth: &AuthState,
        session: &Session,
        headers: &HeaderMap,
        csrf: &str,
    ) -> Result<LogoutOutcome> {
        request(&self.flow.origin, session, headers, csrf).await?;
        ensure!(
            !auth.is_local_mode(),
            "OIDC requires authenticated deployment mode"
        );
        let handle = session.remove::<String>(COMPLETED).await?;
        let context = match handle {
            Some(handle) => self.completed.take(&browser(session)?, &handle).await.ok(),
            None => None,
        };
        if let Some(completion) = context {
            return self
                .flow
                .logout(auth, session, headers, csrf, completion)
                .await
                .map(LogoutOutcome::Provider);
        }
        // Contexts intentionally do not survive restart/expiry. Still revoke locally, and never
        // invent a hint or claim provider sign-out when the verified context is unavailable.
        self.cancel_pending(session).await?;
        auth::end_session(auth, session).await?;
        Ok(LogoutOutcome::LocalOnly)
    }

    pub(in crate::oidc) async fn finish_logout(
        &self,
        session: &Session,
        state: &str,
    ) -> Result<()> {
        self.flow.finish_logout(session, state).await
    }
}
