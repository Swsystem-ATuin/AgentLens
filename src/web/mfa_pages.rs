use super::*;
use crate::{auth, mfa};

#[derive(Deserialize)]
pub(super) struct MfaForm {
    csrf_token: String,
    current_password: String,
    #[serde(default)]
    mfa_code: String,
    action: String,
}

#[derive(Template)]
#[template(path = "account_mfa.html")]
struct MfaTemplate {
    csrf_token: String,
    status: mfa::Status,
    enrollment: Option<mfa::Enrollment>,
    codes: Vec<String>,
}

async fn render(
    state: &AppState,
    user: &auth::CurrentUser,
    session: &tower_sessions::Session,
    enrollment: Option<mfa::Enrollment>,
    codes: Vec<String>,
) -> Result<Response, WebError> {
    if state.auth.is_local_mode() {
        return Err(WebError::forbidden());
    }
    Ok(Html(
        MfaTemplate {
            csrf_token: auth::csrf_token(session)
                .await
                .map_err(|_| WebError::authentication("prepare MFA page"))?,
            status: mfa::status(&state.auth, &user.id)
                .await
                .map_err(|_| WebError::authentication("load MFA settings"))?,
            enrollment,
            codes,
        }
        .render()
        .map_err(|e| WebError::template("render MFA settings", e))?,
    )
    .into_response())
}
pub(super) async fn page(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
) -> Result<Response, WebError> {
    render(&state, &user, &session, None, vec![]).await
}

pub(super) async fn mutate(
    State(state): State<AppState>,
    Extension(user): Extension<auth::CurrentUser>,
    session: tower_sessions::Session,
    Form(form): Form<MfaForm>,
) -> Result<Response, WebError> {
    if state.auth.is_local_mode() {
        return Err(WebError::forbidden());
    }
    verify_session_action(&session, &form.csrf_token).await?;
    if !state.auth.accept_password_attempt() {
        return Ok(auth::password_rate_limited());
    }
    let confirmed = state
        .auth
        .authenticate(&user.username, &form.current_password)
        .await
        .map_err(|_| WebError::authentication("confirm MFA management password"))?;
    let Some((confirmed, _)) = confirmed else {
        return Err(WebError::forbidden());
    };
    if confirmed.id != user.id || confirmed.credential_generation != user.credential_generation {
        return Err(WebError::forbidden());
    }
    let inventory = auth::current_inventory_id(&session)
        .await
        .map_err(|_| WebError::authentication("resolve MFA management session"))?
        .ok_or_else(WebError::forbidden)?;
    if form.action == "begin" {
        let enrollment = mfa::begin(&state.auth, &confirmed, &inventory)
            .await
            .map_err(|_| WebError::invalid_auth_request())?;
        return render(&state, &confirmed, &session, Some(enrollment), vec![]).await;
    }
    let changed = match form.action.as_str() {
        "finish" => mfa::finish(&state.auth, &confirmed, &inventory, &form.mfa_code).await,
        "disable" | "recovery" => {
            mfa::manage(
                &state.auth,
                &confirmed,
                &inventory,
                &form.mfa_code,
                form.action == "disable",
            )
            .await
        }
        _ => return Err(WebError::invalid_auth_request()),
    }
    .map_err(|_| WebError::invalid_auth_request())?;
    let Some((generation, codes)) = changed else {
        return Err(WebError::forbidden());
    };
    auth::establish_session(&state.auth, &session, &confirmed, generation, None)
        .await
        .map_err(|_| WebError::authentication("rotate MFA-managed session"))?;
    session
        .insert("auth.mfa_verified", true)
        .await
        .map_err(|_| WebError::authentication("record MFA-managed session proof"))?;
    render(&state, &confirmed, &session, None, codes).await
}
