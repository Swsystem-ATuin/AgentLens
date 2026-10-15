//! Thin, same-origin JSON endpoints for password-first WebAuthn.
use super::*;
use crate::{auth, mfa, passkeys};
use webauthn_rs::prelude::{PublicKeyCredential, RegisterPublicKeyCredential};

#[derive(Deserialize)]
pub(super) struct Start {
    csrf_token: String,
    #[serde(default)]
    current_password: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    mfa_code: String,
    #[serde(default)]
    name: String,
}
#[derive(Deserialize)]
pub(super) struct Finish {
    csrf_token: String,
    ticket: String,
    response: serde_json::Value,
}
#[derive(Deserialize)]
pub(super) struct Delete {
    csrf_token: String,
    current_password: String,
    mfa_code: String,
    id: String,
}
#[derive(Template)]
#[template(path = "account_passkeys.html")]
struct Page {
    enabled: bool,
    origin: String,
    csrf_token: String,
    keys: Vec<passkeys::KeyView>,
}

async fn guard(
    state: &AppState,
    session: &tower_sessions::Session,
    headers: &axum::http::HeaderMap,
    csrf: &str,
) -> Result<(), WebError> {
    if state.auth.is_local_mode() || !state.auth.passkeys.enabled() {
        return Err(WebError::forbidden());
    }
    if headers.get(header::ORIGIN).and_then(|h| h.to_str().ok()) != state.auth.passkeys.origin() {
        return Err(WebError::forbidden());
    }
    verify_session_action(session, csrf).await
}
async fn inventory(session: &tower_sessions::Session) -> Result<String, WebError> {
    auth::current_inventory_id(session)
        .await
        .map_err(|_| WebError::authentication("resolve passkey session"))?
        .ok_or_else(WebError::forbidden)
}
async fn binding(session: &tower_sessions::Session) -> Result<String, WebError> {
    session
        .save()
        .await
        .map_err(|_| WebError::authentication("prepare anonymous passkey session"))?;
    Ok(format!(
        "s:{}",
        session.id().ok_or_else(WebError::forbidden)?
    ))
}
async fn confirm(
    state: &AppState,
    user: &auth::CurrentUser,
    password: &str,
) -> Result<auth::CurrentUser, WebError> {
    if !state.auth.accept_password_attempt() {
        return Err(WebError {
            status: StatusCode::TOO_MANY_REQUESTS,
            public_message: "Authentication attempt limit reached",
            operation: "rate limit passkey password",
            error_kind: "rate_limited",
            error_code: None,
        });
    }
    let Some((confirmed, _)) = state
        .auth
        .authenticate(&user.username, password)
        .await
        .map_err(|_| WebError::authentication("confirm passkey password"))?
    else {
        return Err(WebError::forbidden());
    };
    if confirmed.id != user.id || confirmed.credential_generation != user.credential_generation {
        return Err(WebError::forbidden());
    }
    Ok(confirmed)
}
async fn rotate(
    state: &AppState,
    session: &tower_sessions::Session,
    user: &auth::CurrentUser,
    generation: i64,
) -> Result<(), WebError> {
    auth::establish_session(&state.auth, session, user, generation, None)
        .await
        .map_err(|_| WebError::authentication("rotate passkey-authenticated session"))?;
    session
        .insert("auth.mfa_verified", true)
        .await
        .map_err(|_| WebError::authentication("record passkey second-factor proof"))?;
    Ok(())
}
pub(super) async fn page(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
) -> Result<Response, WebError> {
    if state.auth.is_local_mode() {
        return Err(WebError::forbidden());
    }
    Ok(Html(
        Page {
            enabled: state.auth.passkeys.enabled(),
            origin: state.auth.passkeys.origin().unwrap_or_default().into(),
            csrf_token: auth::csrf_token(&session)
                .await
                .map_err(|_| WebError::authentication("prepare passkey page"))?,
            keys: passkeys::list(&state.auth, &user.id)
                .await
                .map_err(|_| WebError::authentication("list own passkeys"))?,
        }
        .render()
        .map_err(|e| WebError::template("render passkeys", e))?,
    )
    .into_response())
}
pub(super) async fn login_start(
    State(state): State<AppState>,
    session: tower_sessions::Session,
    headers: axum::http::HeaderMap,
    Json(form): Json<Start>,
) -> Result<Response, WebError> {
    guard(&state, &session, &headers, &form.csrf_token).await?;
    if !state.auth.accept_password_attempt() {
        return Ok(auth::password_rate_limited());
    }
    let Some((user, _)) = state
        .auth
        .authenticate(&form.username, &form.password)
        .await
        .map_err(|_| WebError::authentication("verify password before passkey"))?
    else {
        auth::record_failed_login(&state.auth, &form.username)
            .await
            .map_err(|_| WebError::authentication("record failed passkey password phase"))?;
        return Err(WebError::forbidden());
    };
    let challenge =
        passkeys::start_authentication(&state.auth, &user, &binding(&session).await?, true)
            .await
            .map_err(|_| WebError::forbidden())?;
    Ok(Json(challenge).into_response())
}
pub(super) async fn login_finish(
    State(state): State<AppState>,
    session: tower_sessions::Session,
    headers: axum::http::HeaderMap,
    Json(form): Json<Finish>,
) -> Result<Response, WebError> {
    guard(&state, &session, &headers, &form.csrf_token).await?;
    let response: PublicKeyCredential =
        serde_json::from_value(form.response).map_err(|_| WebError::invalid_auth_request())?;
    let user = passkeys::finish_authentication(
        &state.auth,
        &form.ticket,
        &binding(&session).await?,
        response,
        true,
    )
    .await
    .map_err(|_| WebError::forbidden())?;
    rotate(&state, &session, &user, user.credential_generation).await?;
    Ok(Json(serde_json::json!({"redirect":default_landing(&user)})).into_response())
}
pub(super) async fn register_start(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
    headers: axum::http::HeaderMap,
    Json(form): Json<Start>,
) -> Result<Response, WebError> {
    guard(&state, &session, &headers, &form.csrf_token).await?;
    let user = confirm(&state, &user, &form.current_password).await?;
    if !mfa::verify_in_session(&state.auth, &user, &form.mfa_code, &session)
        .await
        .map_err(|_| WebError::authentication("confirm existing passkey registration factor"))?
    {
        return Err(WebError::forbidden());
    }
    let challenge =
        passkeys::start_registration(&state.auth, &user, &inventory(&session).await?, &form.name)
            .await
            .map_err(|_| WebError::invalid_auth_request())?;
    Ok(Json(challenge).into_response())
}
pub(super) async fn register_finish(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
    headers: axum::http::HeaderMap,
    Json(form): Json<Finish>,
) -> Result<Response, WebError> {
    guard(&state, &session, &headers, &form.csrf_token).await?;
    let response: RegisterPublicKeyCredential =
        serde_json::from_value(form.response).map_err(|_| WebError::invalid_auth_request())?;
    let (generation, codes) = passkeys::finish_registration(
        &state.auth,
        &user,
        &inventory(&session).await?,
        &form.ticket,
        response,
    )
    .await
    .map_err(|_| WebError::forbidden())?;
    rotate(&state, &session, &user, generation).await?;
    Ok(Json(serde_json::json!({"codes":codes})).into_response())
}
pub(super) async fn auth_start(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
    headers: axum::http::HeaderMap,
    Json(form): Json<Start>,
) -> Result<Response, WebError> {
    guard(&state, &session, &headers, &form.csrf_token).await?;
    let user = confirm(&state, &user, &form.current_password).await?;
    let challenge = passkeys::start_authentication(
        &state.auth,
        &user,
        &format!("i:{}", inventory(&session).await?),
        false,
    )
    .await
    .map_err(|_| WebError::forbidden())?;
    Ok(Json(challenge).into_response())
}
pub(super) async fn auth_finish(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
    headers: axum::http::HeaderMap,
    Json(form): Json<Finish>,
) -> Result<Response, WebError> {
    guard(&state, &session, &headers, &form.csrf_token).await?;
    let inventory = inventory(&session).await?;
    let response: PublicKeyCredential =
        serde_json::from_value(form.response).map_err(|_| WebError::invalid_auth_request())?;
    let verified = passkeys::finish_authentication(
        &state.auth,
        &form.ticket,
        &format!("i:{inventory}"),
        response,
        false,
    )
    .await
    .map_err(|_| WebError::forbidden())?;
    if verified.id != user.id || verified.credential_generation != user.credential_generation {
        return Err(WebError::forbidden());
    }
    Ok(Json(serde_json::json!({"proof":passkeys::factor_proof(&state.auth,&verified,&inventory).map_err(|_|WebError::forbidden())?})).into_response())
}
pub(super) async fn delete(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
    headers: axum::http::HeaderMap,
    Json(form): Json<Delete>,
) -> Result<Response, WebError> {
    guard(&state, &session, &headers, &form.csrf_token).await?;
    let user = confirm(&state, &user, &form.current_password).await?;
    if !mfa::verify_in_session(&state.auth, &user, &form.mfa_code, &session)
        .await
        .map_err(|_| WebError::authentication("confirm passkey deletion factor"))?
    {
        return Err(WebError::forbidden());
    }
    let generation = passkeys::delete(&state.auth, &user, &inventory(&session).await?, &form.id)
        .await
        .map_err(|_| WebError::forbidden())?;
    rotate(&state, &session, &user, generation).await?;
    Ok(Json(serde_json::json!({"redirect":"/account/passkeys"})).into_response())
}
pub(super) async fn script(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        if state.auth.passkeys.enabled() {
            include_str!("../../assets/passkeys.js")
        } else {
            "// Passkeys disabled until public HTTPS URL is configured."
        },
    )
}
