use super::*;

#[derive(Deserialize, Default)]
pub(super) struct TokenQuery {
    page: Option<u32>,
}

#[derive(Deserialize)]
pub(super) struct CreateForm {
    csrf_token: String,
    current_password: String,
    description: String,
    scope: String,
    days: i64,
}

#[derive(Template)]
#[template(path = "account_tokens.html")]
struct TokensTemplate {
    tokens: Vec<crate::tokens::TokenView>,
    csrf_token: String,
    pagination: Pagination,
    new_token: Option<String>,
    error: Option<String>,
    metrics_allowed: bool,
}

pub(super) async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    session: tower_sessions::Session,
    Query(query): Query<TokenQuery>,
) -> Result<Response, WebError> {
    if state.auth.is_local_mode() {
        return Ok(axum::response::Redirect::to("/account").into_response());
    }
    render(
        &state,
        &user,
        &session,
        bounded_page(query.page, "validate token page")?,
        None,
        None,
    )
    .await
}

async fn render(
    state: &AppState,
    user: &crate::auth::CurrentUser,
    session: &tower_sessions::Session,
    page: u32,
    new_token: Option<String>,
    error: Option<String>,
) -> Result<Response, WebError> {
    let size = user.preferences.page_size;
    let mut tokens = crate::tokens::list(
        &state.auth.pool,
        &user.id,
        i64::from(page - 1) * size as i64,
        size + 1,
    )
    .await
    .map_err(|_| WebError::authentication("list account tokens"))?;
    let next = tokens.len() > size;
    tokens.truncate(size);
    for token in &mut tokens {
        token.created_at = user.preferences.format_timestamp(&token.created_at);
        token.expires_at = user.preferences.format_timestamp(&token.expires_at);
        format_optional_timestamp(&user.preferences, &mut token.last_used_at);
    }
    let csrf_token = crate::auth::csrf_token(session)
        .await
        .map_err(|_| WebError::authentication("prepare token page"))?;
    Ok(Html(
        TokensTemplate {
            tokens,
            csrf_token,
            new_token,
            error,
            metrics_allowed: user
                .access
                .permits(crate::authorization::Capability::OperationsRead),
            pagination: Pagination {
                page,
                previous_href: (page > 1).then(|| format!("/account/tokens?page={}", page - 1)),
                next_href: (next && page < MAX_SESSION_PAGE)
                    .then(|| format!("/account/tokens?page={}", page + 1)),
            },
        }
        .render()
        .map_err(|error| WebError::template("render token page", error))?,
    )
    .into_response())
}

pub(super) async fn create(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    session: tower_sessions::Session,
    Form(form): Form<CreateForm>,
) -> Result<Response, WebError> {
    if state.auth.is_local_mode() {
        return Ok(axum::response::Redirect::to("/account").into_response());
    }
    verify_session_action(&session, &form.csrf_token).await?;
    if let Err(error) = crate::tokens::validate(&form.description, &form.scope, form.days) {
        return Ok((
            StatusCode::BAD_REQUEST,
            render(&state, &user, &session, 1, None, Some(error.into())).await?,
        )
            .into_response());
    }
    if !crate::tokens::scope_capability(&form.scope)
        .is_some_and(|capability| user.access.permits(capability))
    {
        return Err(WebError::forbidden());
    }
    let confirmed = state
        .auth
        .authenticate(&user.username, &form.current_password)
        .await
        .map_err(|_| WebError::authentication("confirm token creation password"))?;
    let Some((confirmed_user, _)) = confirmed else {
        return Ok((
            StatusCode::BAD_REQUEST,
            render(
                &state,
                &user,
                &session,
                1,
                None,
                Some("Current password is incorrect.".into()),
            )
            .await?,
        )
            .into_response());
    };
    if !crate::tokens::scope_capability(&form.scope)
        .is_some_and(|capability| confirmed_user.access.permits(capability))
    {
        return Err(WebError::forbidden());
    }
    match crate::tokens::create(
        &state.auth,
        &confirmed_user,
        &form.description,
        &form.scope,
        form.days,
    )
    .await
    {
        Ok(token) => render(&state, &user, &session, 1, Some(token), None).await,
        Err(error) if error.is::<crate::tokens::CreationDenied>() => {
            let message = "Token could not be created. Check account access or the limit of 10 creations per hour and 100 active tokens.";
            Ok((
                StatusCode::BAD_REQUEST,
                render(&state, &user, &session, 1, None, Some(message.into())).await?,
            )
                .into_response())
        }
        Err(_) => Err(WebError::authentication("create personal access token")),
    }
}

pub(super) async fn revoke(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    session: tower_sessions::Session,
    Path(id): Path<String>,
    Form(form): Form<SessionActionForm>,
) -> Result<Response, WebError> {
    if state.auth.is_local_mode() {
        return Ok(axum::response::Redirect::to("/account").into_response());
    }
    verify_session_action(&session, &form.csrf_token).await?;
    if !crate::tokens::revoke(&state.auth, &user.id, &id)
        .await
        .map_err(|_| WebError::authentication("revoke account token"))?
    {
        return Err(WebError::not_found());
    }
    Ok(axum::response::Redirect::to("/account/tokens").into_response())
}
