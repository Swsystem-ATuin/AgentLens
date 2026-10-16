//! Gated OIDC completion: no provider claim substitutes for local identity/MFA policy.
use super::*;

pub(crate) struct Identity<'a> {
    pub(crate) account_id: &'a str,
    pub(crate) generation: i64,
    pub(crate) link_id: &'a str,
    pub(crate) issuer: &'a str,
    pub(crate) subject: &'a str,
}

pub(crate) async fn complete(
    state: &AuthState,
    session: &Session,
    identity: Identity<'_>,
    factor: &str,
    user_agent: Option<&str>,
) -> Result<Option<(CurrentUser, bool)>> {
    ensure!(!state.is_local_mode(), "OIDC requires authenticated mode");
    ensure!(
        state.current_user(session).await?.is_none(),
        "OIDC requires an anonymous session"
    );
    let proof_binding = format!(
        "s:{}",
        session.id().context("OIDC browser session unavailable")?
    );
    let mut tx = state
        .pool
        .begin()
        .await
        .context("begin OIDC session completion")?;
    let locked = sqlx::query("UPDATE accounts SET session_generation=session_generation WHERE id=? AND enabled=1 AND session_generation=? AND EXISTS(SELECT 1 FROM account_oidc_links WHERE id=? AND account_id=accounts.id AND issuer=? COLLATE BINARY AND subject=? COLLATE BINARY)")
        .bind(identity.account_id).bind(identity.generation).bind(identity.link_id)
        .bind(identity.issuer).bind(identity.subject).execute(&mut *tx).await.context("recheck OIDC identity and credentials")?;
    ensure!(
        locked.rows_affected() == 1,
        "OIDC local identity changed; restart sign-in"
    );
    // The SQLite writer prevents permission/identity mutations while local policy is resolved.
    let (user, _) = state
        .oidc_account(identity.issuer, identity.subject)
        .await?
        .context("OIDC local identity unavailable")?;
    if !crate::mfa::verify_in_transaction(&mut tx, state, &user, factor, Some(&proof_binding))
        .await?
    {
        // Invalid-factor budgets and fixed MFA audit facts must survive rejection.
        tx.commit()
            .await
            .context("commit rejected OIDC factor attempt")?;
        return Ok(None);
    }
    let (required, enrolled): (bool, bool) = sqlx::query_as("SELECT required,(secret IS NOT NULL OR EXISTS(SELECT 1 FROM account_passkeys WHERE account_id=account_mfa.account_id)) FROM account_mfa WHERE account_id=?")
        .bind(&user.id).fetch_optional(&mut *tx).await.context("check OIDC enrollment policy")?.unwrap_or((false,false));
    let enrollment_required = required && !enrolled;
    let inventory_id = random_token();
    let issued_at = unix_now();
    // Rotating an anonymous session uses the session store, which needs its own SQLite writer.
    // Do not call the store while holding this transaction. Commit the factor+inventory first
    // using an intentionally invalid tower identifier, then save the session payload.
    // No actual browser session ID can match this pending inventory, including after a crash.
    let tower_id = format!("oidc_pending:{}", random_token());
    sqlx::query("INSERT INTO account_sessions(id,account_id,tower_session_id,session_generation,logged_in_at,last_seen_at,absolute_expires_at,user_agent_summary) VALUES(?,?,?,?,?,?,?,?)")
        .bind(&inventory_id).bind(&user.id).bind(tower_id).bind(identity.generation)
        .bind(issued_at).bind(issued_at).bind(issued_at.saturating_add(state.absolute_lifetime_seconds))
        .bind(summarize_user_agent(user_agent)).execute(&mut *tx).await.context("create OIDC session inventory")?;
    tx.commit()
        .await
        .context("commit OIDC factor and session inventory")?;
    // Session APIs allocate their own rotated ID; bind that ID with a conditional inventory
    // update after saving. Until then the payload cannot pass inventory validation.
    let saved: Result<()> = async {
        session.flush().await?;
        // flush clears the stored ID but retains the in-memory record's ID; explicitly rotate.
        session.cycle_id().await?;
        session.insert(ACCOUNT_ID, &user.id).await?;
        session.insert(SESSION_GENERATION, identity.generation).await?;
        session.insert(ISSUED_AT, issued_at).await?;
        session.insert(INVENTORY_ID, &inventory_id).await?;
        session.insert("auth.mfa_verified", enrolled).await?;
        session.save().await?;
        let actual_id = session.id().context("OIDC rotated session unavailable")?.to_string();
        let mut binding = state.pool.begin().await.context("begin rotated OIDC session binding")?;
        let bound = sqlx::query("UPDATE account_sessions SET tower_session_id=? WHERE id=? AND account_id=? AND session_generation=? AND revoked_at IS NULL AND EXISTS(SELECT 1 FROM accounts a JOIN account_oidc_links l ON l.account_id=a.id WHERE a.id=? AND a.enabled=1 AND a.session_generation=? AND l.id=? AND l.issuer=? COLLATE BINARY AND l.subject=? COLLATE BINARY)")
            .bind(actual_id).bind(&inventory_id).bind(&user.id).bind(identity.generation)
            .bind(&user.id).bind(identity.generation).bind(identity.link_id).bind(identity.issuer).bind(identity.subject)
            .execute(&mut *binding).await.context("bind rotated OIDC session")?;
        ensure!(bound.rows_affected() == 1, "OIDC identity revoked during session rotation");
        insert_security_event(&mut binding, Some(&user.id), SecurityEvent::LoginSucceeded).await?;
        binding.commit().await.context("commit rotated OIDC session binding")?;
        Ok(())
    }.await;
    if let Err(error) = saved {
        sqlx::query("UPDATE account_sessions SET revoked_at=unixepoch() WHERE id=?")
            .bind(&inventory_id)
            .execute(&state.pool)
            .await
            .context("revoke failed OIDC session")?;
        session
            .flush()
            .await
            .context("discard failed OIDC session")?;
        return Err(error);
    }
    Ok(Some((user, enrollment_required)))
}
