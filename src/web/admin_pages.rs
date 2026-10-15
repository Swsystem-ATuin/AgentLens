//! Bounded administration pages; mutations delegate to the transactional domain module.
use super::*;
use crate::{admin, auth, authorization::Capability};

#[derive(Deserialize, Default)]
pub(super) struct PageQuery {
    page: Option<u32>,
}

#[derive(Deserialize)]
pub(super) struct ActionForm {
    csrf_token: String,
    current_password: String,
    #[serde(flatten)]
    command: admin::Command,
}

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminTemplate {
    section: &'static str,
    csrf_token: String,
    accounts: Vec<admin::Account>,
    teams: Vec<crate::access::Team>,
    account: Option<admin::Account>,
    team: Option<crate::access::Team>,
    grants: Vec<crate::access::GrantOrigin>,
    members: Vec<String>,
    sources: Vec<String>,
    tokens: Vec<crate::tokens::TokenView>,
    events: Vec<admin::AuditEntry>,
    pagination: Pagination,
}

fn guard(
    state: &AppState,
    user: &auth::CurrentUser,
    capability: Capability,
) -> Result<(), WebError> {
    if state.auth.is_local_mode() || !user.access.permits(capability) {
        return Err(WebError::forbidden());
    }
    Ok(())
}

pub(super) async fn accounts(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
    Query(query): Query<PageQuery>,
) -> Result<Response, WebError> {
    render(&state, &user, &session, "accounts", None, query.page).await
}
pub(super) async fn account(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Response, WebError> {
    render(&state, &user, &session, "account", Some(&id), query.page).await
}
pub(super) async fn teams(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
    Query(query): Query<PageQuery>,
) -> Result<Response, WebError> {
    render(&state, &user, &session, "teams", None, query.page).await
}
pub(super) async fn team(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Response, WebError> {
    render(&state, &user, &session, "team", Some(&id), query.page).await
}
pub(super) async fn audit(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
    Query(query): Query<PageQuery>,
) -> Result<Response, WebError> {
    render(&state, &user, &session, "audit", None, query.page).await
}

async fn render(
    state: &AppState,
    user: &auth::CurrentUser,
    session: &tower_sessions::Session,
    section: &'static str,
    id: Option<&str>,
    page: Option<u32>,
) -> Result<Response, WebError> {
    guard(
        state,
        user,
        if section == "audit" {
            Capability::AuditRead
        } else {
            Capability::AccountsAdminister
        },
    )?;
    let page = bounded_page(page, "validate administrative page")?;
    let size = user.preferences.page_size;
    let offset = i64::from(page - 1) * size as i64;
    let limit = size as i64 + 1;
    let mut view = AdminTemplate {
        section,
        csrf_token: auth::csrf_token(session)
            .await
            .map_err(|_| WebError::authentication("prepare administration page"))?,
        accounts: vec![],
        teams: vec![],
        account: None,
        team: None,
        grants: vec![],
        members: vec![],
        sources: vec![],
        tokens: vec![],
        events: vec![],
        pagination: Pagination {
            page,
            previous_href: None,
            next_href: None,
        },
    };
    let next = match section {
        "accounts" => {
            view.accounts = admin::accounts(&state.auth, offset, limit)
                .await
                .map_err(|_| WebError::authentication("list administrative accounts"))?;
            truncate(&mut view.accounts, size)
        }
        "teams" => {
            view.teams = crate::access::list(&state.auth.pool, offset, limit)
                .await
                .map_err(|_| WebError::authentication("list administrative teams"))?;
            truncate(&mut view.teams, size)
        }
        "account" => {
            let id = id.ok_or_else(WebError::not_found)?;
            view.account = Some(
                admin::account(&state.auth, id)
                    .await
                    .map_err(|_| WebError::authentication("load administrative account"))?
                    .ok_or_else(WebError::not_found)?,
            );
            view.grants = crate::access::grant_origins(&state.auth.pool, id, offset, Some(limit))
                .await
                .map_err(|_| WebError::authentication("inspect administrative grants"))?;
            view.tokens = crate::tokens::list(&state.auth.pool, id, offset, size + 1)
                .await
                .map_err(|_| WebError::authentication("inspect administrative tokens"))?;
            truncate(&mut view.grants, size) | truncate(&mut view.tokens, size)
        }
        "team" => {
            let id = id.ok_or_else(WebError::not_found)?;
            view.team = Some(
                sqlx::query_as("SELECT id,name FROM teams WHERE id=?")
                    .bind(id)
                    .fetch_optional(&state.auth.pool)
                    .await
                    .map_err(|_| WebError::authentication("inspect administrative team"))?
                    .ok_or_else(WebError::not_found)?,
            );
            view.members=sqlx::query_scalar("SELECT a.username FROM team_members m JOIN accounts a ON a.id=m.account_id WHERE m.team_id=? ORDER BY a.username COLLATE NOCASE,a.id LIMIT ? OFFSET ?").bind(id).bind(limit).bind(offset).fetch_all(&state.auth.pool).await.map_err(|_|WebError::authentication("inspect administrative membership"))?;
            view.sources=sqlx::query_scalar("SELECT source_key FROM team_source_grants WHERE team_id=? ORDER BY source_key LIMIT ? OFFSET ?").bind(id).bind(limit).bind(offset).fetch_all(&state.auth.pool).await.map_err(|_|WebError::authentication("inspect administrative team grants"))?;
            truncate(&mut view.members, size) | truncate(&mut view.sources, size)
        }
        "audit" => {
            view.events = admin::events(&state.auth, offset, limit)
                .await
                .map_err(|_| WebError::authentication("review installation security history"))?;
            truncate(&mut view.events, size)
        }
        _ => return Err(WebError::not_found()),
    };
    let path = match id {
        Some(id) => format!(
            "/admin/{}/{id}",
            if section == "account" {
                "accounts"
            } else {
                "teams"
            }
        ),
        None => format!("/admin/{section}"),
    };
    view.pagination.previous_href = (page > 1).then(|| format!("{path}?page={}", page - 1));
    view.pagination.next_href =
        (next && page < MAX_SESSION_PAGE).then(|| format!("{path}?page={}", page + 1));
    for event in &mut view.events {
        event.occurred_at = user.preferences.format_timestamp(&event.occurred_at);
    }
    for token in &mut view.tokens {
        token.created_at = user.preferences.format_timestamp(&token.created_at);
        token.expires_at = user.preferences.format_timestamp(&token.expires_at);
        format_optional_timestamp(&user.preferences, &mut token.last_used_at);
    }
    Ok(Html(
        view.render()
            .map_err(|e| WebError::template("render administration", e))?,
    )
    .into_response())
}

fn truncate<T>(rows: &mut Vec<T>, size: usize) -> bool {
    let next = rows.len() > size;
    rows.truncate(size);
    next
}

pub(super) async fn mutate(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
    Form(form): Form<ActionForm>,
) -> Result<Response, WebError> {
    guard(&state, &user, Capability::AccountsAdminister)?;
    verify_session_action(&session, &form.csrf_token).await?;
    if !state.auth.accept_password_attempt() {
        return Ok(auth::password_rate_limited());
    }
    let confirmed = state
        .auth
        .authenticate(&user.username, &form.current_password)
        .await
        .map_err(|_| WebError::authentication("reauthenticate administrator"))?;
    let Some((confirmed, _)) = confirmed else {
        auth::record_failed_login(&state.auth, &user.username)
            .await
            .map_err(|_| WebError::authentication("record failed administrative confirmation"))?;
        return Ok((StatusCode::FORBIDDEN, "Administrative confirmation failed.").into_response());
    };
    if confirmed.id != user.id
        || confirmed.credential_generation != user.credential_generation
        || !confirmed.access.permits(Capability::AccountsAdminister)
    {
        return Err(WebError::forbidden());
    }
    let inventory = auth::current_inventory_id(&session)
        .await
        .map_err(|_| WebError::authentication("resolve administrative session"))?
        .ok_or_else(WebError::forbidden)?;
    match admin::execute(&state.auth,&confirmed,&inventory,form.command).await {
        Ok(())=>Ok(axum::response::Redirect::to("/admin/accounts").into_response()),
        Err(_)=>Ok((StatusCode::CONFLICT,"Operation rejected. Reload the target, confirm its exact current name and state, and check the role, password policy, source ID, uniqueness, and last-enabled-administrator safeguard. No change was committed.").into_response()),
    }
}
