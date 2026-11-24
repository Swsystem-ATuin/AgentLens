use anyhow::{Context, Result};
use askama::Template;
use axum::{
    Form, Json, Router,
    extract::{Extension, Path, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::Path as FilePath;
use tower_http::trace::TraceLayer;

mod admin_pages;
#[cfg(test)]
mod administration;
mod markdown;
mod mfa_pages;
#[cfg(test)]
mod mfa_regression;
mod passkey_pages;
#[cfg(test)]
mod passkey_regression;
mod presentation;
pub(crate) mod queries;
#[cfg(test)]
mod regression;
mod token_pages;

#[cfg(test)]
use presentation::present_timeline;
use presentation::{
    FilterOptions, Pagination, TimelineItem, filtered_transcript_href, message_href,
    present_diagnostic_evidence, present_pressure, present_recent_usage,
    present_timeline_with_aliases, present_turns, present_usage_groups, present_usage_totals,
    select_options, sessions_href, usage_href,
};
use queries::{
    MESSAGE_CHUNK_CHARS, MessageChunk, SessionFilters, SessionRow, ToolChunk, UsageFilters,
};
#[cfg(test)]
use queries::{MessageRow, TimelineRow, USAGE_PAGE_SIZE};

const MAX_SESSION_PAGE: u32 = 100_000;

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
    projects: crate::projects::ProjectConfig,
    auth: crate::auth::AuthState,
}

#[derive(Deserialize, Default)]
struct LoginQuery {
    return_to: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct LogoutForm {
    csrf_token: String,
}

#[derive(Deserialize, Default)]
struct AccountQuery {
    password_changed: Option<String>,
    preferences_saved: Option<String>,
    profile_saved: Option<String>,
}

#[derive(Deserialize)]
struct ChangePasswordForm {
    csrf_token: String,
    current_password: String,
    #[serde(default)]
    mfa_code: String,
    new_password: String,
    confirm_password: String,
}

#[derive(Deserialize)]
struct ProfileForm {
    csrf_token: String,
    #[serde(default)]
    display_name: String,
}

#[derive(Deserialize)]
struct SessionActionForm {
    csrf_token: String,
}

#[derive(Deserialize, Default)]
struct AccountSessionsQuery {
    page: Option<u32>,
    events_page: Option<u32>,
}

#[derive(Default)]
struct AccountNotices<'a> {
    password_changed: bool,
    password_error: Option<&'a str>,
    preferences_saved: bool,
    preferences_error: Option<&'a str>,
    profile_saved: bool,
    profile_error: Option<&'a str>,
}

#[derive(Deserialize)]
struct PreferencesForm {
    csrf_token: String,
    timezone_mode: String,
    timezone_name: String,
    date_time_format: String,
    appearance: String,
    font_scale: u16,
    page_size: usize,
    live_interval_seconds: u64,
    live_paused: Option<String>,
    transcript_detail: String,
    code_wrap: Option<String>,
    default_sessions_view: String,
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginTemplate {
    csrf_token: String,
    return_to: String,
    failed: bool,
    passkeys_enabled: bool,
}

#[derive(Template)]
#[template(path = "account.html")]
struct AccountTemplate {
    username: String,
    display_name: String,
    password_age: String,
    profile_saved: bool,
    profile_failed: bool,
    profile_error: String,
    role: &'static str,
    source_count: usize,
    csrf_token: String,
    local_mode: bool,
    can_administer: bool,
    password_changed: bool,
    password_failed: bool,
    password_error: String,
    preferences: crate::preferences::UserPreferences,
    preferences_saved: bool,
    preferences_failed: bool,
    preferences_error: String,
}

#[derive(Template)]
#[template(path = "account_sessions.html")]
struct AccountSessionsTemplate {
    sessions: Vec<crate::auth::AccountSession>,
    events: Vec<crate::auth::SecurityEventRow>,
    csrf_token: String,
    sessions_pagination: Pagination,
    events_pagination: Pagination,
}

#[cfg(test)]
fn group_transcript(messages: Vec<MessageRow>) -> Vec<TimelineItem> {
    group_transcript_for_source(messages, crate::sources::codex::KIND, false)
}

#[cfg(test)]
fn group_transcript_for_source(
    messages: Vec<MessageRow>,
    source_kind: &str,
    conversation_started: bool,
) -> Vec<TimelineItem> {
    let rows = messages
        .into_iter()
        .map(|message| {
            let total_chars =
                message.content.chars().count() as i64 + i64::from(message.content_truncated);
            TimelineRow {
                event_type: "message".into(),
                sequence: message.sequence,
                sequence_known: true,
                timestamp: message.timestamp,
                role: Some(message.role),
                content: message.content,
                total_chars,
                tool_name: None,
                call_id: None,
                outcome: None,
            }
        })
        .collect();
    present_timeline(rows, source_kind, "stable-session", conversation_started)
}

#[derive(Deserialize, Default)]
struct Filters {
    q: Option<String>,
    host: Option<String>,
    user: Option<String>,
    model: Option<String>,
    status: Option<String>,
    agent: Option<String>,
    project: Option<String>,
    archive: Option<String>,
    cron: Option<String>,
    from: Option<String>,
    to: Option<String>,
    page: Option<u32>,
    browser_timezone: Option<String>,
}

#[derive(Template)]
#[template(path = "sessions.html")]
struct SessionsTemplate {
    sessions: Vec<SessionRow>,
    options: FilterOptions,
    show_origins: bool,
    pagination: Pagination,
    search: Option<String>,
    from: Option<String>,
    to: Option<String>,
    timezone: String,
    cron_all: bool,
    cron_hide: bool,
    empty_message: &'static str,
}

#[derive(Deserialize, Default)]
struct UsageQuery {
    period: Option<String>,
    group: Option<String>,
    from: Option<String>,
    to: Option<String>,
    page: Option<u32>,
    browser_timezone: Option<String>,
}

#[derive(Template)]
#[template(path = "usage.html")]
struct UsageTemplate {
    totals: presentation::UsageTotalsView,
    rows: Vec<presentation::UsageGroupView>,
    recent: Vec<presentation::RecentUsageView>,
    period_options: Vec<presentation::FilterOption>,
    group_options: Vec<presentation::FilterOption>,
    period_label: &'static str,
    group_label: &'static str,
    from: Option<String>,
    to: Option<String>,
    timezone: String,
    pagination: Pagination,
}

#[derive(Template)]
#[template(path = "diagnostics.html")]
struct DiagnosticsTemplate {
    totals: presentation::DiagnosticTotalsView,
    pressure: Vec<presentation::PressureView>,
    interruptions: Vec<presentation::DiagnosticEvidenceView>,
    incomplete: Vec<presentation::DiagnosticEvidenceView>,
    accounting: Vec<presentation::DiagnosticEvidenceView>,
    failures: Vec<presentation::DiagnosticEvidenceView>,
    show_origins: bool,
}

#[derive(Deserialize, Default)]
struct HealthQuery {
    page: Option<u32>,
}

#[derive(Template)]
#[template(path = "health.html")]
struct HealthTemplate {
    latest_run: Option<crate::health::IngestHealth>,
    totals: crate::health::HealthTotals,
    sources: Vec<crate::health::SourceHealth>,
    show_origins: bool,
    pagination: Pagination,
}

#[derive(Template)]
#[template(path = "session.html")]
struct SessionTemplate {
    session: SessionRow,
    segments: Vec<TimelineItem>,
    generated_images: Vec<GeneratedImageView>,
    show_origin: bool,
    event_options: Vec<presentation::FilterOption>,
    pagination: Pagination,
    live: LiveStatus,
}

#[derive(Template)]
#[template(path = "generated_images.html")]
struct GeneratedImagesTemplate {
    session: SessionRow,
    images: Vec<GeneratedImageView>,
}

struct GeneratedImageView {
    ordinal: usize,
    filename: String,
    size: String,
    href: String,
}

#[derive(Serialize)]
struct LiveStatus {
    revision: i64,
    connection: &'static str,
    last_successful_import: Option<String>,
    source_observed_at: Option<String>,
    source_state: &'static str,
    source_projection_status: &'static str,
}

impl From<queries::LiveStatusRow> for LiveStatus {
    fn from(row: queries::LiveStatusRow) -> Self {
        Self {
            revision: row.revision,
            connection: "connected",
            last_successful_import: row.last_successful_import,
            source_observed_at: row.source_observed_at,
            source_state: match row.source_age_seconds {
                Some(0..=120) => "recent",
                Some(_) => "stale",
                None => "unavailable",
            },
            source_projection_status: match row.source_status.as_str() {
                "ok" => "ok",
                "partial" => "partial",
                "error" => "error",
                _ => "unknown",
            },
        }
    }
}

#[cfg(test)]
fn test_live_status() -> LiveStatus {
    LiveStatus {
        revision: 0,
        connection: "connected",
        last_successful_import: None,
        source_observed_at: None,
        source_state: "unavailable",
        source_projection_status: "unknown",
    }
}

#[derive(Template)]
#[template(path = "turns.html")]
struct TurnsTemplate {
    session: SessionRow,
    turns: Vec<presentation::TurnView>,
    pagination: Pagination,
}

#[derive(Deserialize, Default)]
struct TranscriptQuery {
    section: Option<u32>,
    event: Option<String>,
}

#[derive(Deserialize, Default)]
struct MessageQuery {
    part: Option<u32>,
}

#[derive(Deserialize, Default)]
struct TurnHistoryQuery {
    page: Option<u32>,
}

#[derive(Template)]
#[template(path = "message.html")]
struct MessageTemplate {
    message: MessageChunk,
    content_html: String,
    transcript_href: String,
    pagination: Pagination,
}

#[derive(Template)]
#[template(path = "tool.html")]
struct ToolTemplate {
    item: ToolChunk,
    title: String,
    state: String,
    content_html: String,
    transcript_href: String,
    pagination: Pagination,
}

#[cfg(test)]
fn router(pool: SqlitePool, credentials: crate::auth::Credentials) -> Router {
    router_with_projects(pool, credentials, crate::projects::ProjectConfig::default())
}

fn app_routes(
    pool: SqlitePool,
    auth: crate::auth::AuthState,
    projects: crate::projects::ProjectConfig,
) -> Router {
    let state = AppState {
        pool,
        projects,
        auth: auth.clone(),
    };
    Router::new()
        .route("/login", get(login).post(login_submit))
        .route("/login/passkey/start",post(passkey_pages::login_start))
        .route("/login/passkey/finish",post(passkey_pages::login_finish))
        .route("/account/passkeys",get(passkey_pages::page))
        .route("/account/passkeys/register/start",post(passkey_pages::register_start))
        .route("/account/passkeys/register/finish",post(passkey_pages::register_finish))
        .route("/account/passkeys/auth/start",post(passkey_pages::auth_start))
        .route("/account/passkeys/auth/finish",post(passkey_pages::auth_finish))
        .route("/account/passkeys/delete",post(passkey_pages::delete))
        .route("/assets/passkeys.js",get(passkey_pages::script))
        .route(
            "/",
            get(home),
        )
        .route("/account", get(account))
        .route("/account/access", get(account_access))
        .route("/account/mfa",get(mfa_pages::page).post(mfa_pages::mutate))
        .route("/admin/accounts", get(admin_pages::accounts))
        .route("/admin/accounts/{id}", get(admin_pages::account))
        .route("/admin/teams", get(admin_pages::teams))
        .route("/admin/teams/{id}", get(admin_pages::team))
        .route("/admin/audit", get(admin_pages::audit))
        .route("/admin/actions", post(admin_pages::mutate))
        .route("/account/tokens", get(token_pages::list).post(token_pages::create))
        .route("/account/tokens/{id}", post(token_pages::revoke))
        .route("/api/sessions/{id}/live-status", get(live_status))
        .route("/api/sessions/{id}/postmortem", get(api_postmortem))
        .route("/account/password", post(change_password))
        .route("/account/preferences", post(change_preferences))
        .route("/account/profile", post(change_profile))
        .route("/account/sessions", get(account_sessions))
        .route(
            "/account/sessions/revoke-others",
            post(revoke_other_sessions),
        )
        .route(
            "/account/sessions/{id}",
            post(revoke_account_session),
        )
        .route("/logout", post(logout))
        .route("/sessions", get(sessions))
        .route("/usage", get(usage))
        .route("/diagnostics", get(diagnostics))
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .route("/sessions/{id}", get(session))
        .route("/sessions/{id}/live-status", get(live_status))
        .route("/sessions/{id}/generated-images", get(generated_images))
        .route(
            "/sessions/{id}/generated-images/{filename}",
            get(generated_image),
        )
        .route("/sessions/{id}/turns", get(turn_history))
        .route("/sessions/{id}/postmortem/{format}", get(postmortem))
        .route("/sessions/{id}/messages/{sequence}", get(message))
        .route("/sessions/{id}/tools/{sequence}", get(tool_call))
        .route("/sessions/{id}/results/{sequence}", get(tool_result))
        .route("/favicon.ico", get(favicon))
        .route("/assets/favicon-32.png", get(favicon_png))
        .route("/assets/agent-lens-logo.png", get(application_logo))
        .route("/assets/live-reader.js", get(live_reader_script))
        .route("/assets/preferences.js", get(preferences_script))
        .route("/assets/preferences.css", get(preferences_stylesheet))
        // Query strings can contain private filter values. Keep request spans
        // useful without recording the URI or authentication material.
        .layer(TraceLayer::new_for_http().make_span_with(
            |request: &axum::http::Request<axum::body::Body>| {
                tracing::info_span!("http_request", method = %request.method())
            },
        ))
        .with_state(state)
        .layer(axum::middleware::from_fn_with_state(auth, crate::auth::require_auth))
        .layer(axum::middleware::from_fn(crate::auth::security_headers))
}

#[cfg(test)]
fn router_with_projects(
    pool: SqlitePool,
    credentials: crate::auth::Credentials,
    projects: crate::projects::ProjectConfig,
) -> Router {
    use tower_sessions::{Expiry, MemoryStore, SessionManagerLayer, cookie::time::Duration};
    let auth = crate::auth::test_state(credentials);
    app_routes(pool, auth, projects).layer(
        SessionManagerLayer::new(MemoryStore::default())
            .with_secure(false)
            .with_expiry(Expiry::OnInactivity(Duration::minutes(
                crate::auth::INACTIVITY_MINUTES,
            )))
            .with_always_save(true),
    )
}

#[cfg(test)]
fn session_router(pool: SqlitePool, state: crate::auth::StateStore) -> Router {
    use tower_sessions::{Expiry, SessionManagerLayer, cookie::time::Duration};
    let layer = SessionManagerLayer::new(state.sessions)
        .with_name("agent_lens_session")
        .with_secure(false)
        .with_expiry(Expiry::OnInactivity(Duration::minutes(
            crate::auth::INACTIVITY_MINUTES,
        )))
        .with_always_save(true);
    app_routes(pool, state.auth, crate::projects::ProjectConfig::default()).layer(layer)
}

pub async fn serve(
    pool: SqlitePool,
    bind: &str,
    state_store: crate::auth::StateStore,
    projects: crate::projects::ProjectConfig,
    secure_cookies: bool,
) -> Result<()> {
    use tower_sessions::{Expiry, SessionManagerLayer, cookie::time::Duration};
    let session_layer = SessionManagerLayer::new(state_store.sessions)
        .with_name("agent_lens_session")
        .with_http_only(true)
        .with_same_site(tower_sessions::cookie::SameSite::Lax)
        .with_secure(secure_cookies)
        .with_expiry(Expiry::OnInactivity(Duration::minutes(
            crate::auth::INACTIVITY_MINUTES,
        )))
        .with_always_save(true);
    let app = app_routes(pool, state_store.auth, projects).layer(session_layer);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    tracing::info!(%bind, "Agent Lens listening");
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn validate_local_bind(bind: &str) -> Result<()> {
    let address: std::net::SocketAddr = bind
        .parse()
        .context("local mode requires a numeric loopback bind address")?;
    anyhow::ensure!(
        address.ip().is_loopback(),
        "local mode requires a loopback bind address"
    );
    Ok(())
}

async fn home(Extension(user): Extension<crate::auth::CurrentUser>) -> axum::response::Redirect {
    axum::response::Redirect::temporary(default_landing(&user))
}

fn default_landing(user: &crate::auth::CurrentUser) -> &'static str {
    if user
        .access
        .permits(crate::authorization::Capability::ContentRead)
    {
        "/sessions"
    } else {
        "/usage"
    }
}

async fn login(
    State(state): State<AppState>,
    session: tower_sessions::Session,
    Query(query): Query<LoginQuery>,
) -> Result<Response, WebError> {
    if state.auth.is_local_mode() {
        return Ok(axum::response::Redirect::to("/sessions").into_response());
    }
    let csrf_token = crate::auth::csrf_token(&session)
        .await
        .map_err(|_| WebError::authentication("prepare login"))?;
    let return_to =
        crate::auth::safe_return_to(query.return_to.as_deref().unwrap_or("/sessions")).to_owned();
    Ok(Html(
        LoginTemplate {
            csrf_token,
            return_to,
            failed: query.error.is_some(),
            passkeys_enabled: state.auth.passkeys.enabled(),
        }
        .render()
        .map_err(|error| WebError::template("render login", error))?,
    )
    .into_response())
}

async fn login_submit(
    State(state): State<AppState>,
    session: tower_sessions::Session,
    headers: axum::http::HeaderMap,
    Form(form): Form<crate::auth::LoginForm>,
) -> Result<Response, WebError> {
    if state.auth.is_local_mode() {
        return Ok(axum::response::Redirect::to("/sessions").into_response());
    }
    if !crate::auth::verify_csrf(&session, &form.csrf_token)
        .await
        .map_err(|_| WebError::authentication("verify login request"))?
    {
        return Err(WebError::invalid_auth_request());
    }
    let return_to = crate::auth::safe_return_to(&form.return_to).to_owned();
    if !state.auth.accept_password_attempt() {
        return Ok(crate::auth::password_rate_limited());
    }
    if let Some((user, generation)) = state
        .auth
        .authenticate(&form.username, &form.password)
        .await
        .map_err(|_| WebError::authentication("authenticate account"))?
        && crate::mfa::verify(&state.auth, &user, &form.mfa_code)
            .await
            .map_err(|_| WebError::authentication("verify login factor"))?
    {
        crate::auth::establish_session(
            &state.auth,
            &session,
            &user,
            generation,
            headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok()),
        )
        .await
        .map_err(|_| WebError::authentication("establish session"))?;
        session
            .insert("auth.mfa_verified", true)
            .await
            .map_err(|_| WebError::authentication("record completed sign-in factors"))?;
        let target = if return_to == "/sessions" {
            default_landing(&user)
        } else {
            &return_to
        };
        return Ok(axum::response::Redirect::to(target).into_response());
    }
    crate::auth::record_failed_login(&state.auth, &form.username)
        .await
        .map_err(|_| WebError::authentication("record failed login"))?;
    let location = format!(
        "/login?error=1&return_to={}",
        percent_encode_component(&return_to)
    );
    Ok((StatusCode::SEE_OTHER, [(header::LOCATION, location)], "").into_response())
}

#[derive(Template)]
#[template(path = "account_access.html")]
struct AccountAccessTemplate {
    role: &'static str,
    capabilities: Vec<&'static str>,
    unrestricted: bool,
    origins: Vec<crate::access::GrantOrigin>,
    pagination: Pagination,
}

async fn account_access(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Query(query): Query<AccountSessionsQuery>,
) -> Result<Response, WebError> {
    if state.auth.is_local_mode() {
        return Ok(axum::response::Redirect::to("/account").into_response());
    }
    let page = preference_page(
        &user.preferences,
        query.page,
        "validate effective access page",
    )?;
    let size = user.preferences.display_page_size();
    let mut origins = crate::access::grant_origins(
        &state.auth.pool,
        &user.id,
        i64::from(page - 1) * size as i64,
        Some(size as i64 + 1),
    )
    .await
    .map_err(|_| WebError::authentication("explain account access"))?;
    let next = origins.len() > size;
    origins.truncate(size);
    Ok(Html(
        AccountAccessTemplate {
            role: user.access.role(),
            capabilities: crate::authorization::Capability::ALL
                .into_iter()
                .filter(|capability| user.access.permits(*capability))
                .map(|capability| capability.label())
                .collect(),
            unrestricted: user.access.unrestricted(),
            origins,
            pagination: Pagination {
                page,
                previous_href: (page > 1).then(|| format!("/account/access?page={}", page - 1)),
                next_href: (next && page < MAX_SESSION_PAGE)
                    .then(|| format!("/account/access?page={}", page + 1)),
            },
        }
        .render()
        .map_err(|error| WebError::template("render effective account access", error))?,
    )
    .into_response())
}

async fn account(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    session: tower_sessions::Session,
    Query(query): Query<AccountQuery>,
) -> Result<Html<String>, WebError> {
    render_account(
        &state,
        &user,
        &session,
        AccountNotices {
            password_changed: query.password_changed.as_deref() == Some("1"),
            preferences_saved: query.preferences_saved.as_deref() == Some("1"),
            profile_saved: query.profile_saved.as_deref() == Some("1"),
            ..AccountNotices::default()
        },
    )
    .await
}

async fn render_account(
    state: &AppState,
    user: &crate::auth::CurrentUser,
    session: &tower_sessions::Session,
    notices: AccountNotices<'_>,
) -> Result<Html<String>, WebError> {
    let csrf_token = crate::auth::csrf_token(session)
        .await
        .map_err(|_| WebError::authentication("prepare account page"))?;
    Ok(Html(
        AccountTemplate {
            username: user.username.clone(),
            display_name: user.display_name.clone().unwrap_or_default(),
            password_age: password_age(&user.password_changed_at),
            profile_saved: notices.profile_saved,
            profile_failed: notices.profile_error.is_some(),
            profile_error: notices.profile_error.unwrap_or_default().to_owned(),
            role: user.access.role(),
            source_count: user.access.source_count(),
            csrf_token,
            local_mode: state.auth.is_local_mode(),
            can_administer: !state.auth.is_local_mode()
                && user
                    .access
                    .permits(crate::authorization::Capability::AccountsAdminister),
            password_changed: notices.password_changed,
            password_failed: notices.password_error.is_some(),
            password_error: notices.password_error.unwrap_or_default().to_owned(),
            preferences: user.preferences.clone(),
            preferences_saved: notices.preferences_saved,
            preferences_failed: notices.preferences_error.is_some(),
            preferences_error: notices.preferences_error.unwrap_or_default().to_owned(),
        }
        .render()
        .map_err(|error| WebError::template("render account page", error))?,
    ))
}

async fn change_password(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    session: tower_sessions::Session,
    Form(form): Form<ChangePasswordForm>,
) -> Result<Response, WebError> {
    if state.auth.is_local_mode() {
        return Ok(axum::response::Redirect::to("/account").into_response());
    }
    if !crate::auth::verify_csrf(&session, &form.csrf_token)
        .await
        .map_err(|_| WebError::authentication("verify password change request"))?
    {
        return Err(WebError::invalid_auth_request());
    }
    if !state.auth.accept_password_attempt() {
        return Ok(crate::auth::password_rate_limited());
    }
    if crate::mfa::status(&state.auth, &user.id)
        .await
        .map_err(|_| WebError::authentication("check password-change MFA status"))?
        .enrolled
    {
        let confirmed = state
            .auth
            .authenticate(&user.username, &form.current_password)
            .await
            .map_err(|_| WebError::authentication("confirm password-change credentials"))?;
        if !confirmed.is_some_and(|(confirmed, _)| {
            confirmed.id == user.id && confirmed.credential_generation == user.credential_generation
        }) {
            return Err(WebError::forbidden());
        }
    }
    if !crate::mfa::verify_in_session(&state.auth, &user, &form.mfa_code, &session)
        .await
        .map_err(|_| WebError::authentication("confirm password change factor"))?
    {
        return Err(WebError::forbidden());
    }
    let error = if form.new_password != form.confirm_password {
        Some("New password and confirmation do not match.")
    } else {
        match crate::auth::change_password(
            &state.auth,
            &user.id,
            &form.current_password,
            &form.new_password,
        )
        .await
        .map_err(|_| WebError::authentication("change account password"))?
        {
            crate::auth::ChangePasswordResult::Changed { generation } => {
                crate::auth::establish_session(&state.auth, &session, &user, generation, None)
                    .await
                    .map_err(|_| WebError::authentication("refresh changed-password session"))?;
                session
                    .insert("auth.mfa_verified", true)
                    .await
                    .map_err(|_| {
                        WebError::authentication("preserve changed-password factor proof")
                    })?;
                return Ok(
                    axum::response::Redirect::to("/account?password_changed=1").into_response()
                );
            }
            crate::auth::ChangePasswordResult::InvalidCurrentPassword => {
                Some("Current password is incorrect.")
            }
            crate::auth::ChangePasswordResult::InvalidNewPassword(error) => {
                return Ok((
                    StatusCode::BAD_REQUEST,
                    render_account(
                        &state,
                        &user,
                        &session,
                        AccountNotices {
                            password_error: Some(&error.to_string()),
                            ..AccountNotices::default()
                        },
                    )
                    .await?,
                )
                    .into_response());
            }
        }
    };
    Ok((
        StatusCode::BAD_REQUEST,
        render_account(
            &state,
            &user,
            &session,
            AccountNotices {
                password_error: error,
                ..AccountNotices::default()
            },
        )
        .await?,
    )
        .into_response())
}

async fn change_preferences(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    session: tower_sessions::Session,
    Form(form): Form<PreferencesForm>,
) -> Result<Response, WebError> {
    if !crate::auth::verify_csrf(&session, &form.csrf_token)
        .await
        .map_err(|_| WebError::authentication("verify preferences request"))?
    {
        return Err(WebError::invalid_auth_request());
    }
    let preferences = crate::preferences::UserPreferences {
        timezone_mode: form.timezone_mode,
        timezone_name: form.timezone_name.trim().to_owned(),
        date_time_format: form.date_time_format,
        appearance: form.appearance,
        font_scale: form.font_scale,
        page_size: form.page_size,
        live_interval_seconds: form.live_interval_seconds,
        live_paused: form.live_paused.is_some(),
        transcript_detail: form.transcript_detail,
        code_wrap: form.code_wrap.is_some(),
        default_sessions_view: form.default_sessions_view,
    };
    if let Err(error) = preferences.validate() {
        return Ok((
            StatusCode::BAD_REQUEST,
            render_account(
                &state,
                &user,
                &session,
                AccountNotices {
                    preferences_error: Some(error),
                    ..AccountNotices::default()
                },
            )
            .await?,
        )
            .into_response());
    }
    if state.auth.is_local_mode() {
        let mut response =
            axum::response::Redirect::to("/account?preferences_saved=1").into_response();
        response.headers_mut().append(
            header::SET_COOKIE,
            crate::local_preferences::set(&preferences)
                .map_err(|_| WebError::authentication("save local display preferences"))?,
        );
        return Ok(response);
    }
    crate::auth::update_preferences(&state.auth, &user.id, &preferences)
        .await
        .map_err(|_| WebError::authentication("save account preferences"))?;
    Ok(axum::response::Redirect::to("/account?preferences_saved=1").into_response())
}

async fn change_profile(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    session: tower_sessions::Session,
    Form(form): Form<ProfileForm>,
) -> Result<Response, WebError> {
    if state.auth.is_local_mode() {
        return Ok(axum::response::Redirect::to("/account").into_response());
    }
    if !crate::auth::verify_csrf(&session, &form.csrf_token)
        .await
        .map_err(|_| WebError::authentication("verify profile request"))?
    {
        return Err(WebError::invalid_auth_request());
    }
    let display_name = form.display_name.trim();
    if display_name.chars().count() > 80 || display_name.chars().any(char::is_control) {
        return Ok((
            StatusCode::BAD_REQUEST,
            render_account(
                &state,
                &user,
                &session,
                AccountNotices {
                    profile_error: Some(
                        "Display name must be at most 80 characters without control characters.",
                    ),
                    ..AccountNotices::default()
                },
            )
            .await?,
        )
            .into_response());
    }
    crate::auth::update_profile(&state.auth, &user.id, display_name)
        .await
        .map_err(|_| WebError::authentication("update account profile"))?;
    Ok(axum::response::Redirect::to("/account?profile_saved=1").into_response())
}

async fn account_sessions(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    session: tower_sessions::Session,
    Query(query): Query<AccountSessionsQuery>,
) -> Result<Html<String>, WebError> {
    if state.auth.is_local_mode() {
        return Ok(Html("Local mode does not retain account sessions.".into()));
    }
    let page = preference_page(
        &user.preferences,
        query.page,
        "validate account session page",
    )?;
    let events_page = preference_page(
        &user.preferences,
        query.events_page,
        "validate security event page",
    )?;
    let page_size = user.preferences.display_page_size();
    let current_id = crate::auth::current_inventory_id(&session)
        .await
        .map_err(|_| WebError::authentication("load current session inventory"))?
        .ok_or_else(WebError::invalid_auth_request)?;
    let mut sessions = crate::auth::account_sessions(
        &state.auth,
        &user.id,
        Some(&current_id),
        i64::from(page - 1) * page_size as i64,
        page_size + 1,
    )
    .await
    .map_err(|_| WebError::authentication("load account sessions"))?;
    let has_next = sessions.len() > page_size;
    sessions.truncate(page_size);
    for item in &mut sessions {
        item.logged_in_at = user.preferences.format_timestamp(&item.logged_in_at);
        item.last_seen_at = user.preferences.format_timestamp(&item.last_seen_at);
    }
    let mut events = crate::auth::security_events(
        &state.auth,
        &user.id,
        i64::from(events_page - 1) * page_size as i64,
        page_size + 1,
    )
    .await
    .map_err(|_| WebError::authentication("load account security events"))?;
    let events_has_next = events.len() > page_size;
    events.truncate(page_size);
    for event in &mut events {
        event.occurred_at = user.preferences.format_timestamp(&event.occurred_at);
    }
    let csrf_token = crate::auth::csrf_token(&session)
        .await
        .map_err(|_| WebError::authentication("prepare session inventory"))?;
    Ok(Html(
        AccountSessionsTemplate {
            sessions,
            events,
            csrf_token,
            sessions_pagination: Pagination {
                page,
                previous_href: (page > 1).then(|| account_sessions_href(page - 1, events_page)),
                next_href: has_next.then(|| account_sessions_href(page + 1, events_page)),
            },
            events_pagination: Pagination {
                page: events_page,
                previous_href: (events_page > 1)
                    .then(|| account_sessions_href(page, events_page - 1)),
                next_href: events_has_next.then(|| account_sessions_href(page, events_page + 1)),
            },
        }
        .render()
        .map_err(|error| WebError::template("render account sessions", error))?,
    ))
}

async fn revoke_account_session(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    session: tower_sessions::Session,
    Path(target_id): Path<String>,
    Form(form): Form<SessionActionForm>,
) -> Result<Response, WebError> {
    verify_session_action(&session, &form.csrf_token).await?;
    let current_id = crate::auth::current_inventory_id(&session)
        .await
        .map_err(|_| WebError::authentication("load current session inventory"))?
        .ok_or_else(WebError::invalid_auth_request)?;
    let revoked =
        crate::auth::revoke_account_session(&state.auth, &user.id, &current_id, &target_id)
            .await
            .map_err(|_| WebError::authentication("revoke account session"))?;
    if !revoked {
        return Err(WebError::not_found());
    }
    Ok(axum::response::Redirect::to("/account/sessions").into_response())
}

async fn revoke_other_sessions(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    session: tower_sessions::Session,
    Form(form): Form<SessionActionForm>,
) -> Result<Response, WebError> {
    verify_session_action(&session, &form.csrf_token).await?;
    let current_id = crate::auth::current_inventory_id(&session)
        .await
        .map_err(|_| WebError::authentication("load current session inventory"))?
        .ok_or_else(WebError::invalid_auth_request)?;
    crate::auth::revoke_other_account_sessions(&state.auth, &user.id, &current_id)
        .await
        .map_err(|_| WebError::authentication("revoke other account sessions"))?;
    Ok(axum::response::Redirect::to("/account/sessions").into_response())
}

async fn verify_session_action(
    session: &tower_sessions::Session,
    supplied: &str,
) -> Result<(), WebError> {
    if !crate::auth::verify_csrf(session, supplied)
        .await
        .map_err(|_| WebError::authentication("verify session action"))?
    {
        return Err(WebError::invalid_auth_request());
    }
    Ok(())
}

fn account_sessions_href(page: u32, events_page: u32) -> String {
    format!("/account/sessions?page={page}&events_page={events_page}")
}

fn password_age(value: &str) -> String {
    let timestamp = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
        .map(|value| value.and_utc())
        .or_else(|_| chrono::DateTime::parse_from_rfc3339(value).map(|value| value.to_utc()));
    timestamp.map_or_else(
        |_| "Unavailable".into(),
        |timestamp| {
            let days = chrono::Utc::now()
                .signed_duration_since(timestamp)
                .num_days()
                .max(0);
            match days {
                0 => "Changed today".into(),
                1 => "Changed 1 day ago".into(),
                days => format!("Changed {days} days ago"),
            }
        },
    )
}

async fn logout(
    State(state): State<AppState>,
    session: tower_sessions::Session,
    Form(form): Form<LogoutForm>,
) -> Result<Response, WebError> {
    if state.auth.is_local_mode() {
        return Ok(axum::response::Redirect::to("/sessions").into_response());
    }
    if !crate::auth::verify_csrf(&session, &form.csrf_token)
        .await
        .map_err(|_| WebError::authentication("verify logout request"))?
    {
        return Err(WebError::invalid_auth_request());
    }
    crate::auth::end_session(&state.auth, &session)
        .await
        .map_err(|_| WebError::authentication("end session"))?;
    Ok(axum::response::Redirect::to("/login").into_response())
}

fn percent_encode_component(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

fn format_optional_timestamp(
    preferences: &crate::preferences::UserPreferences,
    timestamp: &mut Option<String>,
) {
    if let Some(value) = timestamp {
        *value = preferences.format_timestamp(value);
    }
}

fn format_timestamp_or(
    preferences: &crate::preferences::UserPreferences,
    timestamp: &str,
) -> String {
    if matches!(timestamp, "Unavailable" | "Timestamp unavailable") {
        timestamp.to_owned()
    } else {
        preferences.format_timestamp(timestamp)
    }
}

fn effective_preferences(
    preferences: &crate::preferences::UserPreferences,
    browser_timezone: Option<&str>,
) -> crate::preferences::UserPreferences {
    let mut effective = preferences.clone();
    if preferences.timezone_mode == "browser"
        && let Some(timezone) = browser_timezone
        && timezone.len() <= 64
        && timezone.parse::<chrono_tz::Tz>().is_ok()
    {
        effective.timezone_mode = "iana".into();
        effective.timezone_name = timezone.to_owned();
    }
    effective
}

async fn usage(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Query(mut query): Query<UsageQuery>,
) -> Result<Html<String>, WebError> {
    let preferences = effective_preferences(&user.preferences, query.browser_timezone.as_deref());
    let page_size = user.preferences.display_page_size();
    let projects = state
        .projects
        .with_timezone(preferences.sql_timezone_modifier());
    query.normalize();
    query.validate()?;
    let page = preference_page(&user.preferences, query.page, "validate usage page")?;
    let show_origins = queries::show_origins(&state.pool)
        .await
        .map_err(|error| WebError::database("load source layout", error))?;
    if !show_origins && matches!(query.group.as_deref(), Some("host" | "user")) {
        query.group = Some("session".to_owned());
    }
    let period = query.period.as_deref().unwrap_or("day");
    let group = query.group.as_deref().unwrap_or("model");
    let filters = UsageFilters {
        period,
        group,
        from: query.from.as_deref(),
        to: query.to.as_deref(),
    };
    let (totals, mut rows, recent) = tokio::try_join!(
        queries::usage_totals(&state.pool, &filters, &projects, &user.access),
        queries::usage_groups(
            &state.pool,
            &filters,
            &projects,
            &user.access,
            i64::from(page - 1) * page_size as i64,
            page_size,
        ),
        queries::recent_usage(&state.pool, &filters, &projects, &user.access),
    )
    .map_err(|error| WebError::database("load usage dashboard", error))?;
    let has_next = rows.len() > page_size;
    rows.truncate(page_size);
    let pagination = Pagination {
        page,
        previous_href: (page > 1).then(|| {
            usage_href(
                period,
                group,
                filters.from,
                filters.to,
                query.browser_timezone.as_deref(),
                page - 1,
            )
        }),
        next_href: (has_next && page < MAX_SESSION_PAGE).then(|| {
            usage_href(
                period,
                group,
                filters.from,
                filters.to,
                query.browser_timezone.as_deref(),
                page + 1,
            )
        }),
    };
    let mut groups = vec!["session", "model", "project", "agent"];
    if show_origins {
        groups.extend(["host", "user"]);
    }
    let mut totals = present_usage_totals(totals);
    totals.latest_at = format_timestamp_or(&user.preferences, &totals.latest_at);
    let mut recent = present_recent_usage(recent);
    for row in &mut recent {
        row.timestamp = format_timestamp_or(&user.preferences, &row.timestamp);
    }
    Ok(Html(
        UsageTemplate {
            totals,
            rows: present_usage_groups(rows),
            recent,
            period_options: select_options(
                ["day", "week", "month"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                Some(period),
            ),
            group_options: select_options(
                groups.into_iter().map(str::to_owned).collect(),
                Some(group),
            ),
            period_label: match period {
                "week" => "Week starting Monday",
                "month" => "Month",
                _ => "Day",
            },
            group_label: match group {
                "session" => "Session",
                "project" => "Project",
                "host" => "Host",
                "user" => "User",
                "agent" => "Agent",
                _ => "Model",
            },
            from: query.from,
            to: query.to,
            timezone: user.preferences.timezone_label().to_owned(),
            pagination,
        }
        .render()
        .map_err(|error| WebError::template("render usage dashboard", error))?,
    ))
}

async fn diagnostics(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
) -> Result<Html<String>, WebError> {
    let (totals, pressure, interruptions, incomplete, accounting, failures, show_origins) =
        tokio::try_join!(
            queries::diagnostic_totals(&state.pool, &user.access),
            queries::pressure_snapshots(&state.pool, &user.access),
            queries::diagnostic_evidence(&state.pool, "interruption", &user.access),
            queries::diagnostic_evidence(&state.pool, "incomplete", &user.access),
            queries::diagnostic_evidence(&state.pool, "accounting", &user.access),
            queries::diagnostic_evidence(&state.pool, "failure", &user.access),
            queries::show_origins(&state.pool),
        )
        .map_err(|error| WebError::database("load diagnostics", error))?;
    let page_size = user.preferences.display_page_size();
    let mut pressure = present_pressure(pressure, page_size);
    for row in &mut pressure {
        row.timestamp = format_timestamp_or(&user.preferences, &row.timestamp);
    }
    let mut interruptions = present_diagnostic_evidence(interruptions, "interruption", page_size);
    let mut incomplete = present_diagnostic_evidence(incomplete, "incomplete", page_size);
    let mut accounting = present_diagnostic_evidence(accounting, "accounting", page_size);
    let mut failures = present_diagnostic_evidence(failures, "failure", page_size);
    for row in interruptions
        .iter_mut()
        .chain(incomplete.iter_mut())
        .chain(accounting.iter_mut())
        .chain(failures.iter_mut())
    {
        row.timestamp = format_timestamp_or(&user.preferences, &row.timestamp);
    }
    Ok(Html(
        DiagnosticsTemplate {
            totals: totals.into(),
            pressure,
            interruptions,
            incomplete,
            accounting,
            failures,
            show_origins,
        }
        .render()
        .map_err(|error| WebError::template("render diagnostics", error))?,
    ))
}

async fn health(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Query(query): Query<HealthQuery>,
) -> Result<Html<String>, WebError> {
    if !user
        .access
        .permits(crate::authorization::Capability::OperationsRead)
    {
        return Err(WebError::forbidden());
    }
    let page = preference_page(&user.preferences, query.page, "validate health page")?;
    let page_size = user.preferences.display_page_size();
    let (mut snapshot, show_origins) = tokio::try_join!(
        crate::health::snapshot(
            &state.pool,
            i64::from(page - 1) * page_size as i64,
            page_size,
        ),
        queries::show_origins(&state.pool),
    )
    .map_err(|error| WebError::database("load source health", error))?;
    let has_next = snapshot.sources.len() > page_size;
    snapshot.sources.truncate(page_size);
    if let Some(run) = &mut snapshot.latest_run {
        run.started_at = format_timestamp_or(&user.preferences, &run.started_at);
        format_optional_timestamp(&user.preferences, &mut run.finished_at);
    }
    for source in &mut snapshot.sources {
        format_optional_timestamp(&user.preferences, &mut source.observed_at);
    }
    Ok(Html(
        HealthTemplate {
            latest_run: snapshot.latest_run,
            totals: snapshot.totals,
            sources: snapshot.sources,
            show_origins,
            pagination: Pagination {
                page,
                previous_href: (page > 1).then(|| format!("/health?page={}", page - 1)),
                next_href: (has_next && page < MAX_SESSION_PAGE)
                    .then(|| format!("/health?page={}", page + 1)),
            },
        }
        .render()
        .map_err(|error| WebError::template("render source health", error))?,
    ))
}

async fn metrics(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
) -> Result<Response, WebError> {
    if !user
        .access
        .permits(crate::authorization::Capability::OperationsRead)
    {
        return Err(WebError::forbidden());
    }
    let body = crate::metrics::render(&state.pool)
        .await
        .map_err(|error| WebError::database("load operational metrics", error))?;
    Ok((
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response())
}

impl UsageQuery {
    fn normalize(&mut self) {
        for value in [
            &mut self.period,
            &mut self.group,
            &mut self.from,
            &mut self.to,
            &mut self.browser_timezone,
        ] {
            if value.as_deref().is_some_and(str::is_empty) {
                *value = None;
            }
        }
    }

    fn validate(&self) -> Result<(), WebError> {
        if self
            .period
            .as_deref()
            .is_some_and(|value| !matches!(value, "day" | "week" | "month"))
            || self.group.as_deref().is_some_and(|value| {
                !matches!(
                    value,
                    "session" | "model" | "project" | "host" | "user" | "agent"
                )
            })
        {
            return Err(WebError::invalid_filters());
        }
        if self
            .browser_timezone
            .as_deref()
            .is_some_and(|value| value.len() > 64 || value.parse::<chrono_tz::Tz>().is_err())
        {
            return Err(WebError::invalid_filters());
        }
        for value in [self.from.as_deref(), self.to.as_deref()]
            .into_iter()
            .flatten()
        {
            if !valid_date(value) {
                return Err(WebError::invalid_filters());
            }
        }
        if self
            .from
            .as_deref()
            .zip(self.to.as_deref())
            .is_some_and(|(from, to)| from > to)
        {
            return Err(WebError::invalid_filters());
        }
        Ok(())
    }
}

async fn sessions(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Query(mut filters): Query<Filters>,
) -> Result<Html<String>, WebError> {
    filters.normalize();
    if filters.archive.is_none() && user.preferences.default_sessions_view != "all" {
        filters.archive = Some(user.preferences.default_sessions_view.clone());
    }
    filters.validate()?;
    let page = user.preferences.display_page(filters.page_number()?);
    let preferences = effective_preferences(&user.preferences, filters.browser_timezone.as_deref());
    let page_size = user.preferences.display_page_size();
    let projects = state
        .projects
        .with_timezone(preferences.sql_timezone_modifier());
    let show_origins = queries::show_origins(&state.pool)
        .await
        .map_err(|error| WebError::database("load source layout", error))?;
    if !show_origins {
        filters.host = None;
        filters.user = None;
    }
    let values = queries::filter_values(&state.pool, show_origins, &projects, &user.access)
        .await
        .map_err(|error| WebError::database("load filter options", error))?;
    let query_filters = filters.as_query();
    let options = FilterOptions::new(values, &query_filters);
    let mut rows = queries::sessions(
        &state.pool,
        &query_filters,
        &projects,
        &user.access,
        i64::from(page - 1) * page_size as i64,
        page_size,
    )
    .await
    .map_err(|error| WebError::database("load sessions", error))?;
    let has_next = rows.len() > page_size;
    rows.truncate(page_size);
    for row in &mut rows {
        format_optional_timestamp(&user.preferences, &mut row.started_at);
        format_optional_timestamp(&user.preferences, &mut row.last_activity);
    }
    if let Some(search) = filters.q.as_deref() {
        let session_ids: Vec<i64> = rows.iter().map(|row| row.id).collect();
        let matches = queries::search_matches(&state.pool, &session_ids, search)
            .await
            .map_err(|error| WebError::database("load transcript search previews", error))?;
        for row in &mut rows {
            let candidates: Vec<_> = matches
                .iter()
                .filter(|candidate| candidate.session_id == row.id)
                .collect();
            let selected = candidates
                .iter()
                .copied()
                .find(|candidate| {
                    crate::sources::classify_message(
                        &row.agent_kind,
                        &candidate.role,
                        &candidate.content_prefix,
                        true,
                    )
                    .is_none()
                })
                .or_else(|| candidates.first().copied());
            if let Some(selected) = selected {
                row.match_sequence = Some(selected.sequence);
                row.match_snippet = Some(crate::search::highlighted_html(&selected.snippet));
            }
        }
    }
    let pagination = Pagination {
        page,
        previous_href: (page > 1).then(|| filters.href(page - 1)),
        next_href: (has_next && page < MAX_SESSION_PAGE).then(|| filters.href(page + 1)),
    };
    Ok(Html(
        SessionsTemplate {
            sessions: rows,
            options,
            show_origins,
            pagination,
            search: filters.q.clone(),
            from: filters.from.clone(),
            to: filters.to.clone(),
            timezone: user.preferences.timezone_label().to_owned(),
            cron_all: filters.cron.as_deref() == Some("all"),
            cron_hide: filters.cron.as_deref() == Some("hide"),
            empty_message: if page > 1 {
                "No sessions on this page."
            } else if filters.has_active() {
                "No sessions match these filters."
            } else {
                "No sessions have been indexed."
            },
        }
        .render()
        .map_err(|error| WebError::template("render sessions", error))?,
    ))
}

impl Filters {
    fn normalize(&mut self) {
        if let Some(search) = &mut self.q {
            *search = search.trim().to_owned();
        }
        for value in [
            &mut self.q,
            &mut self.host,
            &mut self.user,
            &mut self.model,
            &mut self.status,
            &mut self.agent,
            &mut self.project,
            &mut self.archive,
            &mut self.cron,
            &mut self.from,
            &mut self.to,
            &mut self.browser_timezone,
        ] {
            if value.as_deref().is_some_and(str::is_empty) {
                *value = None;
            }
        }
    }

    fn validate(&self) -> Result<(), WebError> {
        if self
            .cron
            .as_deref()
            .is_some_and(|value| !matches!(value, "all" | "hide"))
        {
            return Err(WebError::invalid_filters());
        }
        if self
            .archive
            .as_deref()
            .is_some_and(|value| !matches!(value, "all" | "active" | "archived" | "unknown"))
        {
            return Err(WebError::invalid_filters());
        }
        if self
            .q
            .as_deref()
            .is_some_and(|query| query.chars().count() > crate::search::MAX_QUERY_CHARS)
        {
            return Err(WebError::invalid_filters());
        }
        if self
            .browser_timezone
            .as_deref()
            .is_some_and(|value| value.len() > 64 || value.parse::<chrono_tz::Tz>().is_err())
        {
            return Err(WebError::invalid_filters());
        }
        for value in [self.from.as_deref(), self.to.as_deref()]
            .into_iter()
            .flatten()
        {
            if !valid_date(value) {
                return Err(WebError::invalid_filters());
            }
        }
        if self
            .from
            .as_deref()
            .zip(self.to.as_deref())
            .is_some_and(|(from, to)| from > to)
        {
            return Err(WebError::invalid_filters());
        }
        Ok(())
    }

    fn has_active(&self) -> bool {
        self.q.is_some()
            || self.host.is_some()
            || self.user.is_some()
            || self.model.is_some()
            || self.status.is_some()
            || self.agent.is_some()
            || self.project.is_some()
            || self.archive.is_some()
            || self.cron.is_some()
            || self.from.is_some()
            || self.to.is_some()
    }

    fn page_number(&self) -> Result<u32, WebError> {
        let page = self.page.unwrap_or(1);
        if page == 0 || page > MAX_SESSION_PAGE {
            return Err(WebError::invalid_page());
        }
        Ok(page)
    }

    fn as_query(&self) -> SessionFilters<'_> {
        SessionFilters {
            search: self.q.as_deref(),
            host: self.host.as_deref(),
            user: self.user.as_deref(),
            model: self.model.as_deref(),
            status: self.status.as_deref(),
            agent: self.agent.as_deref(),
            project: self.project.as_deref(),
            archive: self.archive.as_deref(),
            cron: self.cron.as_deref(),
            from: self.from.as_deref(),
            to: self.to.as_deref(),
            browser_timezone: self.browser_timezone.as_deref(),
        }
    }

    fn href(&self, page: u32) -> String {
        sessions_href(&self.as_query(), page)
    }
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes
            .iter()
            .enumerate()
            .any(|(index, byte)| !matches!(index, 4 | 7) && !byte.is_ascii_digit())
    {
        return false;
    }
    let number = |start: usize, end: usize| {
        value[start..end]
            .parse::<u32>()
            .expect("validated ASCII digits")
    };
    let year = number(0, 4);
    let month = number(5, 7);
    let day = number(8, 10);
    if year == 0 {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    day != 0 && day <= days
}

async fn session(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Path(stable_id): Path<String>,
    Query(query): Query<TranscriptQuery>,
) -> Result<Html<String>, WebError> {
    let page_size = user.preferences.display_page_size();
    let section = preference_page(
        &user.preferences,
        query.section,
        "validate transcript section",
    )?;
    let event_filter = query.event.as_deref().unwrap_or("all");
    if !matches!(
        event_filter,
        "all" | "messages" | "tools" | "questions" | "turns" | "diagnostics"
    ) {
        return Err(WebError::invalid_filters());
    }
    let show_origin = queries::show_origins(&state.pool)
        .await
        .map_err(|error| WebError::database("load source layout", error))?;
    let mut row = load_session(&state.pool, &stable_id, &state.projects, &user.access).await?;
    format_optional_timestamp(&user.preferences, &mut row.started_at);
    format_optional_timestamp(&user.preferences, &mut row.last_activity);
    let mut live: LiveStatus = queries::live_status(&state.pool, &stable_id, &user.access)
        .await
        .map_err(|error| WebError::database("load live reader status", error))?
        .ok_or_else(WebError::not_found)?
        .into();
    format_optional_timestamp(&user.preferences, &mut live.last_successful_import);
    format_optional_timestamp(&user.preferences, &mut live.source_observed_at);
    let mut events = queries::timeline_section(
        &state.pool,
        row.id,
        event_filter,
        i64::from(section - 1) * page_size as i64,
        page_size,
    )
    .await
    .map_err(|error| WebError::database("load transcript section", error))?;
    let has_next = events.len() > page_size;
    events.truncate(page_size);
    for event in &mut events {
        format_optional_timestamp(&user.preferences, &mut event.timestamp);
    }
    let pagination = Pagination {
        page: section,
        previous_href: (section > 1)
            .then(|| filtered_transcript_href(&stable_id, section - 1, event_filter, None)),
        next_href: (has_next && section < MAX_SESSION_PAGE)
            .then(|| filtered_transcript_href(&stable_id, section + 1, event_filter, None)),
    };
    let generated_image_aliases = load_generated_image_aliases(
        &state.pool,
        row.id,
        &stable_id,
        &row.agent_kind,
        &user.access,
    )
    .await?;
    let generated_images = if !has_next && matches!(event_filter, "all" | "messages") {
        load_unlinked_generated_images(
            &state.pool,
            &stable_id,
            &row.agent_kind,
            &user.access,
            &generated_image_aliases,
        )
        .await?
    } else {
        Vec::new()
    };
    let mut segments = present_timeline_with_aliases(
        events,
        &row.agent_kind,
        &stable_id,
        section > 1 && matches!(event_filter, "all" | "messages"),
        &generated_image_aliases,
    );
    for segment in &mut segments {
        segment.initially_open = user.preferences.detail_open(segment.initially_open);
    }
    let event_options = select_options(
        [
            "all",
            "messages",
            "tools",
            "questions",
            "turns",
            "diagnostics",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        Some(event_filter),
    );
    Ok(Html(
        SessionTemplate {
            session: row,
            segments,
            generated_images,
            show_origin,
            event_options,
            pagination,
            live,
        }
        .render()
        .map_err(|error| WebError::template("render transcript", error))?,
    ))
}

async fn live_status(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Path(stable_id): Path<String>,
) -> Result<Json<LiveStatus>, WebError> {
    Ok(Json(
        queries::live_status(&state.pool, &stable_id, &user.access)
            .await
            .map_err(|error| WebError::database("load live reader status", error))?
            .ok_or_else(WebError::not_found)?
            .into(),
    ))
}

async fn generated_images(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Path(stable_id): Path<String>,
) -> Result<Html<String>, WebError> {
    let mut session = load_session(&state.pool, &stable_id, &state.projects, &user.access).await?;
    format_optional_timestamp(&user.preferences, &mut session.started_at);
    format_optional_timestamp(&user.preferences, &mut session.last_activity);
    let source = queries::artifact_source(&state.pool, &stable_id, &user.access)
        .await
        .map_err(|error| WebError::database("load generated image source", error))?
        .ok_or_else(WebError::not_found)?;
    let images = if source.agent_kind == crate::sources::codex::KIND {
        crate::sources::codex::generated_images(
            FilePath::new(&source.source_home),
            &source.external_id,
        )
        .map_err(|_| WebError::filesystem("list generated images"))?
        .into_iter()
        .enumerate()
        .map(|(index, image)| GeneratedImageView {
            ordinal: index + 1,
            size: format_bytes(image.size_bytes),
            href: format!("/sessions/{stable_id}/generated-images/{}", image.filename),
            filename: image.filename,
        })
        .collect()
    } else {
        Vec::new()
    };
    Ok(Html(
        GeneratedImagesTemplate { session, images }
            .render()
            .map_err(|error| WebError::template("render generated images", error))?,
    ))
}

async fn generated_image(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Path((stable_id, filename)): Path<(String, String)>,
) -> Result<Response, WebError> {
    let source = queries::artifact_source(&state.pool, &stable_id, &user.access)
        .await
        .map_err(|error| WebError::database("load generated image source", error))?
        .ok_or_else(WebError::not_found)?;
    if source.agent_kind != crate::sources::codex::KIND {
        return Err(WebError::not_found());
    }
    let path = crate::sources::codex::generated_image_path(
        FilePath::new(&source.source_home),
        &source.external_id,
        &filename,
    )
    .map_err(|_| WebError::filesystem("resolve generated image"))?
    .ok_or_else(WebError::not_found)?;
    let body = tokio::fs::read(path)
        .await
        .map_err(|_| WebError::filesystem("read generated image"))?;
    let content_type = match filename.rsplit_once('.').map(|(_, extension)| extension) {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        _ => return Err(WebError::not_found()),
    };
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_DISPOSITION, "inline"),
        ],
        body,
    )
        .into_response())
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024 * 1024) as f64)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

async fn load_unlinked_generated_images(
    pool: &SqlitePool,
    stable_id: &str,
    agent_kind: &str,
    access: &crate::auth::AccessScope,
    aliases: &std::collections::HashMap<String, String>,
) -> Result<Vec<GeneratedImageView>, WebError> {
    if agent_kind != crate::sources::codex::KIND {
        return Ok(Vec::new());
    }
    let source = queries::artifact_source(pool, stable_id, access)
        .await
        .map_err(|error| WebError::database("load generated image source", error))?
        .ok_or_else(WebError::not_found)?;
    let linked: std::collections::HashSet<_> = aliases.values().map(String::as_str).collect();
    Ok(crate::sources::codex::generated_images(
        FilePath::new(&source.source_home),
        &source.external_id,
    )
    .map_err(|_| WebError::filesystem("list generated images"))?
    .into_iter()
    .filter(|image| !linked.contains(image.filename.as_str()))
    .enumerate()
    .map(|(index, image)| GeneratedImageView {
        ordinal: index + 1,
        size: format_bytes(image.size_bytes),
        href: format!("/sessions/{stable_id}/generated-images/{}", image.filename),
        filename: image.filename,
    })
    .collect())
}

async fn load_generated_image_aliases(
    pool: &SqlitePool,
    session_id: i64,
    stable_id: &str,
    agent_kind: &str,
    access: &crate::auth::AccessScope,
) -> Result<std::collections::HashMap<String, String>, WebError> {
    if agent_kind != crate::sources::codex::KIND {
        return Ok(Default::default());
    }
    let source = queries::artifact_source(pool, stable_id, access)
        .await
        .map_err(|error| WebError::database("load generated image source", error))?
        .ok_or_else(WebError::not_found)?;
    let arguments = queries::generated_image_tool_arguments(pool, session_id)
        .await
        .map_err(|error| WebError::database("load generated image aliases", error))?;
    crate::sources::codex::generated_image_aliases(
        FilePath::new(&source.source_home),
        &source.external_id,
        &arguments,
    )
    .map_err(|_| WebError::filesystem("resolve generated image aliases"))
}

async fn live_reader_script() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../assets/live-reader.js"),
    )
}

async fn preferences_script(
    Extension(user): Extension<crate::auth::CurrentUser>,
) -> impl IntoResponse {
    let preferences = user.preferences;
    let script = format!(
        "(() => {{\n  'use strict';\n  window.agentLensPreferences = {{ liveIntervalMs: {}, livePaused: {} }};\n  if ({:?} !== 'browser') return;\n  const browserTimezone = Intl.DateTimeFormat().resolvedOptions().timeZone;\n  if (browserTimezone && ['/sessions', '/usage'].includes(window.location.pathname)) {{\n    const url = new URL(window.location.href);\n    if (!url.searchParams.has('browser_timezone')) {{\n      url.searchParams.set('browser_timezone', browserTimezone);\n      window.location.replace(url);\n      return;\n    }}\n  }}\n  const format = {:?};\n  const locale = format === 'iso' ? 'sv-SE' : undefined;\n  const options = format === 'long-12' ? {{ dateStyle: 'medium', timeStyle: 'medium', hour12: true }} : format === 'long-24' ? {{ dateStyle: 'medium', timeStyle: 'medium', hour12: false }} : {{ year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }};\n  for (const item of document.querySelectorAll('time[data-user-time]')) {{\n    const parsed = new Date(item.textContent.trim());\n    if (!Number.isNaN(parsed.valueOf())) item.textContent = new Intl.DateTimeFormat(locale, options).format(parsed);\n  }}\n}})();\n",
        preferences.live_interval_seconds * 1_000,
        preferences.live_paused,
        preferences.timezone_mode,
        preferences.date_time_format,
    );
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        script,
    )
}

async fn preferences_stylesheet(
    Extension(user): Extension<crate::auth::CurrentUser>,
) -> impl IntoResponse {
    let preferences = user.preferences;
    let theme = match preferences.appearance.as_str() {
        "light" => {
            ":root{color-scheme:light;--bg:#f7f9fb;--panel:#fff;--line:#b7c2cc;--text:#17212b;--muted:#52697b;--accent:#08785a}"
        }
        "high-contrast" => {
            ":root{color-scheme:dark;--bg:#000;--panel:#000;--line:#fff;--text:#fff;--muted:#ddd;--accent:#5fffd0}a{text-decoration:underline}button,input,select{border-width:2px}"
        }
        "dark" => ":root{color-scheme:dark}",
        _ => {
            "@media(prefers-color-scheme:light){:root{color-scheme:light;--bg:#f7f9fb;--panel:#fff;--line:#b7c2cc;--text:#17212b;--muted:#52697b;--accent:#08785a}}"
        }
    };
    let wrapping = if preferences.code_wrap {
        ".rich-text pre{white-space:pre-wrap;overflow-wrap:anywhere}"
    } else {
        ".rich-text pre{white-space:pre;overflow:auto}"
    };
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        format!(
            "{theme}\n:root{{font-size:{}%}}\n{wrapping}\n",
            preferences.font_scale
        ),
    )
}

async fn favicon() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/x-icon")],
        include_bytes!("../assets/favicon.ico").as_slice(),
    )
}

async fn favicon_png() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/png")],
        include_bytes!("../assets/favicon-32.png").as_slice(),
    )
}

async fn application_logo() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "image/png")],
        include_bytes!("../assets/agent-lens-logo.png").as_slice(),
    )
}

async fn turn_history(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Path(stable_id): Path<String>,
    Query(query): Query<TurnHistoryQuery>,
) -> Result<Html<String>, WebError> {
    let page = preference_page(&user.preferences, query.page, "validate turn page")?;
    let page_size = user.preferences.display_page_size();
    let session = load_session(&state.pool, &stable_id, &state.projects, &user.access).await?;
    let mut rows = queries::session_turns(
        &state.pool,
        session.id,
        i64::from(page - 1) * page_size as i64,
        page_size,
    )
    .await
    .map_err(|error| WebError::database("load turn history", error))?;
    let has_next = rows.len() > page_size;
    rows.truncate(page_size);
    for row in &mut rows {
        format_optional_timestamp(&user.preferences, &mut row.start_timestamp);
        format_optional_timestamp(&user.preferences, &mut row.end_timestamp);
    }
    Ok(Html(
        TurnsTemplate {
            turns: present_turns(rows, &stable_id, page_size),
            session,
            pagination: Pagination {
                page,
                previous_href: (page > 1)
                    .then(|| format!("/sessions/{stable_id}/turns?page={}", page - 1)),
                next_href: (has_next && page < MAX_SESSION_PAGE)
                    .then(|| format!("/sessions/{stable_id}/turns?page={}", page + 1)),
            },
        }
        .render()
        .map_err(|error| WebError::template("render turn history", error))?,
    ))
}

async fn api_postmortem(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Path(id): Path<String>,
) -> Result<Response, WebError> {
    postmortem(State(state), Extension(user), Path((id, "json".into()))).await
}

async fn postmortem(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Path((stable_id, format)): Path<(String, String)>,
) -> Result<Response, WebError> {
    let session = load_session(&state.pool, &stable_id, &state.projects, &user.access).await?;
    let row = queries::postmortem(&state.pool, session.id)
        .await
        .map_err(|error| WebError::database("load postmortem", error))?;
    let report = crate::postmortem::PostmortemReport::from_row(row);
    let (content_type, filename, body) = match format.as_str() {
        "json" => (
            "application/json; charset=utf-8",
            "agent-lens-postmortem.json",
            report.json().map_err(|_| WebError::export())?,
        ),
        "md" => (
            "text/markdown; charset=utf-8",
            "agent-lens-postmortem.md",
            report.markdown(),
        ),
        _ => return Err(WebError::not_found()),
    };
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response())
}

async fn load_session(
    pool: &SqlitePool,
    stable_id: &str,
    projects: &crate::projects::ProjectConfig,
    access: &crate::auth::AccessScope,
) -> Result<SessionRow, WebError> {
    queries::session(pool, stable_id, projects, access)
        .await
        .map_err(|error| WebError::database("load session", error))?
        .ok_or_else(WebError::not_found)
}

async fn message(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Path((stable_id, sequence)): Path<(String, i64)>,
    Query(query): Query<MessageQuery>,
) -> Result<Html<String>, WebError> {
    let part = bounded_page(query.part, "validate message part")?;
    let session = load_session(&state.pool, &stable_id, &state.projects, &user.access).await?;
    let offset = i64::from(part - 1) * MESSAGE_CHUNK_CHARS;
    let mut message = queries::message_chunk(&state.pool, session.id, sequence, offset)
        .await
        .map_err(|error| WebError::database("load message part", error))?
        .ok_or_else(WebError::not_found)?;
    if part > 1 && offset >= message.total_chars {
        return Err(WebError::invalid_pagination("validate message part"));
    }
    format_optional_timestamp(&user.preferences, &mut message.timestamp);
    let preceding = queries::preceding_timeline_event_count(&state.pool, session.id, sequence, 0)
        .await
        .map_err(|error| WebError::database("locate message section", error))?;
    let section = preceding / user.preferences.display_page_size() as i64 + 1;
    let pagination = Pagination {
        page: part,
        previous_href: (part > 1).then(|| message_href(&stable_id, sequence, part - 1)),
        next_href: (offset + MESSAGE_CHUNK_CHARS < message.total_chars && part < MAX_SESSION_PAGE)
            .then(|| message_href(&stable_id, sequence, part + 1)),
    };
    let generated_image_aliases = load_generated_image_aliases(
        &state.pool,
        session.id,
        &stable_id,
        &session.agent_kind,
        &user.access,
    )
    .await?;
    Ok(Html(
        MessageTemplate {
            content_html: if session.agent_kind == crate::sources::codex::KIND {
                markdown::render_with_generated_images(
                    &message.content,
                    &stable_id,
                    &generated_image_aliases,
                )
            } else {
                markdown::render(&message.content)
            },
            message,
            transcript_href: filtered_transcript_href(
                &stable_id,
                section as u32,
                "all",
                Some(&format!("event-{sequence}-message")),
            ),
            pagination,
        }
        .render()
        .map_err(|error| WebError::template("render message part", error))?,
    ))
}

async fn tool_call(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Path((stable_id, sequence)): Path<(String, i64)>,
    Query(query): Query<MessageQuery>,
) -> Result<Html<String>, WebError> {
    tool_detail(
        state,
        user.access,
        user.preferences,
        stable_id,
        sequence,
        query.part,
        false,
    )
    .await
}

async fn tool_result(
    State(state): State<AppState>,
    Extension(user): Extension<crate::auth::CurrentUser>,
    Path((stable_id, sequence)): Path<(String, i64)>,
    Query(query): Query<MessageQuery>,
) -> Result<Html<String>, WebError> {
    tool_detail(
        state,
        user.access,
        user.preferences,
        stable_id,
        sequence,
        query.part,
        true,
    )
    .await
}

async fn tool_detail(
    state: AppState,
    access: crate::auth::AccessScope,
    preferences: crate::preferences::UserPreferences,
    stable_id: String,
    sequence: i64,
    requested_part: Option<u32>,
    result: bool,
) -> Result<Html<String>, WebError> {
    let page_size = preferences.display_page_size();
    let part = bounded_page(requested_part, "validate tool detail part")?;
    let session = load_session(&state.pool, &stable_id, &state.projects, &access).await?;
    let offset = i64::from(part - 1) * MESSAGE_CHUNK_CHARS;
    let mut item = if result {
        queries::tool_result_chunk(&state.pool, session.id, sequence, offset).await
    } else {
        queries::tool_call_chunk(&state.pool, session.id, sequence, offset).await
    }
    .map_err(|error| WebError::database("load tool detail part", error))?
    .ok_or_else(WebError::not_found)?;
    if part > 1 && offset >= item.total_chars {
        return Err(WebError::invalid_pagination("validate tool detail part"));
    }
    format_optional_timestamp(&preferences, &mut item.timestamp);
    let event_order = if result { 2 } else { 1 };
    let preceding =
        queries::preceding_timeline_event_count(&state.pool, session.id, sequence, event_order)
            .await
            .map_err(|error| WebError::database("locate tool timeline section", error))?;
    let section = preceding / page_size as i64 + 1;
    let detail_kind = if result { "results" } else { "tools" };
    let title = if item.kind == "user_input" {
        if result { "Answer" } else { "Question" }.to_owned()
    } else if result {
        format!(
            "Tool result: {}",
            item.tool_name.as_deref().unwrap_or("unknown tool")
        )
    } else {
        format!(
            "Tool request: {}",
            item.tool_name.as_deref().unwrap_or("unknown tool")
        )
    };
    let state_label = if result {
        if item.outcome == "failed" {
            "failed"
        } else {
            "output observed"
        }
    } else {
        match item.outcome.as_str() {
            "failed" => "requested · failed",
            "output observed" => "requested · output observed",
            _ => "requested · outcome unknown",
        }
    };
    let pagination = Pagination {
        page: part,
        previous_href: (part > 1).then(|| {
            format!(
                "/sessions/{stable_id}/{detail_kind}/{sequence}?part={}",
                part - 1
            )
        }),
        next_href: (offset + MESSAGE_CHUNK_CHARS < item.total_chars && part < MAX_SESSION_PAGE)
            .then(|| {
                format!(
                    "/sessions/{stable_id}/{detail_kind}/{sequence}?part={}",
                    part + 1
                )
            }),
    };
    Ok(Html(
        ToolTemplate {
            content_html: markdown::render_plain(&item.content),
            transcript_href: filtered_transcript_href(
                &stable_id,
                section as u32,
                "all",
                Some(&format!("event-{sequence}-{detail_kind}")),
            ),
            item,
            title,
            state: state_label.to_owned(),
            pagination,
        }
        .render()
        .map_err(|error| WebError::template("render tool detail", error))?,
    ))
}

fn preference_page(
    preferences: &crate::preferences::UserPreferences,
    page: Option<u32>,
    operation: &'static str,
) -> Result<u32, WebError> {
    bounded_page(page, operation).map(|page| preferences.display_page(page))
}

fn bounded_page(page: Option<u32>, operation: &'static str) -> Result<u32, WebError> {
    let page = page.unwrap_or(1);
    if page == 0 || page > MAX_SESSION_PAGE {
        return Err(WebError::invalid_pagination(operation));
    }
    Ok(page)
}

struct WebError {
    status: StatusCode,
    public_message: &'static str,
    operation: &'static str,
    error_kind: &'static str,
    error_code: Option<String>,
}
impl WebError {
    fn forbidden() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            public_message: "access denied",
            operation: "authorize capability",
            error_kind: "forbidden",
            error_code: None,
        }
    }

    fn authentication(operation: &'static str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            public_message: "Internal server error",
            operation,
            error_kind: "authentication",
            error_code: None,
        }
    }

    fn invalid_auth_request() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            public_message: "invalid authentication request",
            operation: "validate authentication request",
            error_kind: "invalid_input",
            error_code: None,
        }
    }

    fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            public_message: "session not found",
            operation: "load session",
            error_kind: "not_found",
            error_code: None,
        }
    }

    fn invalid_page() -> Self {
        Self::invalid_pagination("validate session page")
    }

    fn invalid_filters() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            public_message: "invalid filters",
            operation: "validate session filters",
            error_kind: "invalid_input",
            error_code: None,
        }
    }

    fn invalid_pagination(operation: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            public_message: "invalid page",
            operation,
            error_kind: "invalid_input",
            error_code: None,
        }
    }

    fn database(operation: &'static str, error: sqlx::Error) -> Self {
        let (error_kind, error_code) = classify_database_error(&error);
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            public_message: "Internal server error",
            operation,
            error_kind,
            error_code,
        }
    }

    fn template(operation: &'static str, _error: askama::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            public_message: "Internal server error",
            operation,
            error_kind: "template",
            error_code: None,
        }
    }

    fn export() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            public_message: "Internal server error",
            operation: "serialize postmortem",
            error_kind: "serialization",
            error_code: None,
        }
    }

    fn filesystem(operation: &'static str) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            public_message: "Internal server error",
            operation,
            error_kind: "io",
            error_code: None,
        }
    }
}

fn classify_database_error(error: &sqlx::Error) -> (&'static str, Option<String>) {
    match error {
        sqlx::Error::Configuration(_) => ("configuration", None),
        sqlx::Error::Database(error) => ("database", error.code().map(|code| code.into_owned())),
        sqlx::Error::Io(error) => ("io", Some(format!("{:?}", error.kind()))),
        sqlx::Error::Tls(_) => ("tls", None),
        sqlx::Error::Protocol(_) => ("protocol", None),
        sqlx::Error::RowNotFound => ("row_not_found", None),
        sqlx::Error::TypeNotFound { .. } => ("type_not_found", None),
        sqlx::Error::ColumnIndexOutOfBounds { .. } => ("column_index", None),
        sqlx::Error::ColumnNotFound(_) => ("column_not_found", None),
        sqlx::Error::ColumnDecode { .. } => ("column_decode", None),
        sqlx::Error::Decode(_) => ("decode", None),
        sqlx::Error::AnyDriverError(_) => ("driver", None),
        sqlx::Error::PoolTimedOut => ("pool_timeout", None),
        sqlx::Error::PoolClosed => ("pool_closed", None),
        sqlx::Error::WorkerCrashed => ("worker_crashed", None),
        sqlx::Error::Migrate(_) => ("migration", None),
        _ => ("unknown", None),
    }
}
impl IntoResponse for WebError {
    fn into_response(self) -> axum::response::Response {
        tracing::error!(
            operation = self.operation,
            error_kind = self.error_kind,
            error_code = self.error_code.as_deref(),
            status = self.status.as_u16(),
            "request failed"
        );
        let mut response = (self.status, self.public_message).into_response();
        if self.status == StatusCode::TOO_MANY_REQUESTS {
            response.headers_mut().insert(
                header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("60"),
            );
        }
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    mod all_rows;
    mod local_preferences;

    #[test]
    fn local_mode_accepts_only_numeric_loopback_bind_addresses() {
        for bind in ["127.0.0.1:3001", "[::1]:3001"] {
            assert!(validate_local_bind(bind).is_ok(), "{bind}");
        }
        for bind in [
            "0.0.0.0:3001",
            "[::]:3001",
            "192.0.2.1:3001",
            "localhost:3001",
        ] {
            assert!(validate_local_bind(bind).is_err(), "{bind}");
        }
    }

    #[tokio::test]
    async fn local_mode_serves_data_and_reports_without_an_account() {
        use axum::{
            body::{Body, to_bytes},
            http::Request,
        };
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open(&dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let mut state = crate::auth::StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        state.enable_local_mode();
        let app = session_router(pool, state);
        for path in [
            "/sessions",
            "/usage",
            "/diagnostics",
            "/health",
            "/metrics",
            "/account",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            if path == "/account" {
                let body = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
                let html = String::from_utf8(body.to_vec()).unwrap();
                assert!(html.contains("Local mode is active"));
                assert!(!html.contains("Sign out"));
            }
        }
        let login = app
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::SEE_OTHER);
        assert_eq!(login.headers()[header::LOCATION], "/sessions");
    }

    pub(super) async fn login_cookie(app: &Router, username: &str, password: &str) -> String {
        login_cookie_with_agent(app, username, password, None).await
    }

    async fn login_cookie_with_agent(
        app: &Router,
        username: &str,
        password: &str,
        user_agent: Option<&str>,
    ) -> String {
        use axum::{
            body::{Body, to_bytes},
            http::{Request, header},
        };
        use tower::ServiceExt;
        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let html = String::from_utf8(
            to_bytes(login.into_body(), 32 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let csrf = html
            .split("name=\"csrf_token\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let body = format!(
            "username={username}&password={password}&csrf_token={csrf}&return_to=%2Fsessions"
        );
        let mut request = Request::builder()
            .method("POST")
            .uri("/login")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::COOKIE, cookie);
        if let Some(user_agent) = user_agent {
            request = request.header(header::USER_AGENT, user_agent);
        }
        let response = app
            .clone()
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }

    async fn account_response(app: &Router, cookie: &str) -> Response {
        use axum::{
            body::Body,
            http::{Request, header},
        };
        use tower::ServiceExt;
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/account")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn every_route_requires_authentication() {
        use axum::{
            body::{Body, to_bytes},
            http::{Request, header},
        };
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let password = dir.path().join("password");
        std::fs::write(&password, "Synthetic1:password").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let state = crate::auth::StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        crate::auth::create_account(&state, "test", &password, None)
            .await
            .unwrap();
        let pool = crate::db::open(&dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let app = session_router(pool, state);
        for (path, content_type, signature) in [
            (
                "/favicon.ico",
                "image/x-icon",
                b"\x00\x00\x01\x00".as_slice(),
            ),
            (
                "/assets/favicon-32.png",
                "image/png",
                b"\x89PNG\r\n\x1a\n".as_slice(),
            ),
            (
                "/assets/agent-lens-logo.png",
                "image/png",
                b"\x89PNG\r\n\x1a\n".as_slice(),
            ),
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let body = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
            assert!(
                body.starts_with(signature),
                "invalid asset signature: {path}"
            );
        }
        for path in [
            "/",
            "/sessions",
            "/usage",
            "/diagnostics",
            "/health",
            "/metrics",
            "/account",
            "/account/sessions",
            "/account/profile",
            "/account/password",
            "/account/preferences",
            "/account/sessions/revoke-others",
            "/account/sessions/synthetic",
            "/logout",
            "/sessions/1",
            "/sessions/1/live-status",
            "/sessions/1/turns",
            "/sessions/1/postmortem/json",
            "/sessions/1/postmortem/md",
            "/sessions/1/messages/0",
            "/assets/live-reader.js",
            "/sessions/invalid",
            "/missing",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert_eq!(
                to_bytes(response.into_body(), 1024).await.unwrap(),
                "Authentication required"
            );
        }

        let redirected = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sessions?model=private")
                    .header(header::ACCEPT, "text/html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(redirected.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            redirected.headers()[header::LOCATION],
            "/login?return_to=%2Fsessions%3Fmodel%3Dprivate"
        );
        let invalid_csrf = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(
                        "username=test&password=synthetic%3Apassword&csrf_token=wrong",
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_csrf.status(), StatusCode::BAD_REQUEST);

        use base64::{Engine, engine::general_purpose::STANDARD};
        let metrics_basic = format!("Basic {}", STANDARD.encode("test:Synthetic1:password"));
        let metrics = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .header(header::AUTHORIZATION, metrics_basic)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(metrics.status(), StatusCode::OK);

        let login = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/login?return_to=%2Fsessions%3Fmodel%3Dx")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::OK);
        let anonymous_cookie = login.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let login_html = String::from_utf8(
            to_bytes(login.into_body(), 32 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(login_html.contains("href=\"/favicon.ico\""));
        assert!(login_html.contains("src=\"/assets/agent-lens-logo.png\""));
        let csrf = login_html
            .split("name=\"csrf_token\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let failed_body = format!(
            "username=test&password=wrong-password&csrf_token={csrf}&return_to=%2Fsessions"
        );
        let failed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &anonymous_cookie)
                    .body(Body::from(failed_body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(failed.status(), StatusCode::SEE_OTHER);
        let location = failed.headers()[header::LOCATION].to_str().unwrap();
        assert!(location.starts_with("/login?error=1"));
        assert!(!location.contains("test"));
        assert!(!location.contains("wrong-password"));
        let body = format!(
            "username=test&password=Synthetic1%3Apassword&csrf_token={csrf}&return_to=%2Fsessions%3Fmodel%3Dx"
        );
        let logged_in = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/login")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &anonymous_cookie)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(logged_in.status(), StatusCode::SEE_OTHER);
        assert_eq!(logged_in.headers()[header::LOCATION], "/sessions?model=x");
        let authenticated_cookie = logged_in.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        assert_ne!(
            anonymous_cookie, authenticated_cookie,
            "login must rotate the session identifier"
        );

        for (path, status) in [
            ("/", StatusCode::TEMPORARY_REDIRECT),
            ("/sessions", StatusCode::OK),
            ("/usage", StatusCode::OK),
            ("/diagnostics", StatusCode::OK),
            ("/health", StatusCode::OK),
            ("/metrics", StatusCode::OK),
            ("/account", StatusCode::OK),
            ("/account/sessions", StatusCode::OK),
            ("/sessions/1", StatusCode::NOT_FOUND),
            ("/sessions/1/live-status", StatusCode::NOT_FOUND),
            ("/sessions/1/turns", StatusCode::NOT_FOUND),
            ("/sessions/1/postmortem/json", StatusCode::NOT_FOUND),
            ("/sessions/1/postmortem/md", StatusCode::NOT_FOUND),
            ("/assets/live-reader.js", StatusCode::OK),
            ("/assets/preferences.js", StatusCode::OK),
            ("/assets/preferences.css", StatusCode::OK),
            ("/missing", StatusCode::FORBIDDEN),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::COOKIE, &authenticated_cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
    }

    #[tokio::test]
    async fn sessions_survive_restart_and_support_logout_expiry_disable_and_revocation() {
        use axum::{
            body::{Body, to_bytes},
            http::{Request, header},
        };
        use tower::ServiceExt;
        let dir = tempfile::tempdir().unwrap();
        let password_file = dir.path().join("password");
        std::fs::write(&password_file, "Synthetic1-password").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&password_file, std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        let state = crate::auth::StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        crate::auth::create_account(&state, "alice", &password_file, None)
            .await
            .unwrap();
        let pool = crate::db::open(&dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let app = session_router(pool.clone(), state.clone());

        let cookie = login_cookie(&app, "alice", "Synthetic1-password").await;
        let restarted = session_router(pool.clone(), state.clone());
        let account = account_response(&restarted, &cookie).await;
        assert_eq!(account.status(), StatusCode::OK);
        let html = String::from_utf8(
            to_bytes(account.into_body(), 32 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("Signed in as <strong>alice</strong>"));

        crate::auth::revoke_sessions(&state, "alice").await.unwrap();
        assert_eq!(
            account_response(&restarted, &cookie).await.status(),
            StatusCode::UNAUTHORIZED
        );

        let cookie = login_cookie(&restarted, "alice", "Synthetic1-password").await;
        crate::auth::create_account(
            &state,
            "recovery",
            &password_file,
            Some(crate::auth::AccountRole::Admin),
        )
        .await
        .unwrap();
        crate::auth::set_enabled(&state, "alice", false)
            .await
            .unwrap();
        assert_eq!(
            account_response(&restarted, &cookie).await.status(),
            StatusCode::UNAUTHORIZED
        );
        crate::auth::set_enabled(&state, "alice", true)
            .await
            .unwrap();

        let cookie = login_cookie(&restarted, "alice", "Synthetic1-password").await;
        sqlx::query("UPDATE tower_sessions SET expiry_date=0")
            .execute(crate::auth::state_pool(&state))
            .await
            .unwrap();
        assert_eq!(
            account_response(&restarted, &cookie).await.status(),
            StatusCode::UNAUTHORIZED
        );

        let cookie = login_cookie(&restarted, "alice", "Synthetic1-password").await;
        let account = account_response(&restarted, &cookie).await;
        let html = String::from_utf8(
            to_bytes(account.into_body(), 32 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let csrf = html
            .split("name=\"csrf_token\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let invalid = restarted
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/logout")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from("csrf_token=wrong"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            account_response(&restarted, &cookie).await.status(),
            StatusCode::OK
        );
        let logout = restarted
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/logout")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(format!("csrf_token={csrf}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(logout.status(), StatusCode::SEE_OTHER);
        assert_eq!(logout.headers()[header::LOCATION], "/login");
        assert_eq!(
            account_response(&restarted, &cookie).await.status(),
            StatusCode::UNAUTHORIZED
        );

        let mut short_state = state.clone();
        crate::auth::expire_immediately(&mut short_state);
        let expiring = session_router(pool, short_state);
        let cookie = login_cookie(&expiring, "alice", "Synthetic1-password").await;
        assert_eq!(
            account_response(&expiring, &cookie).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn signed_in_users_can_change_their_password() {
        use axum::{
            body::{Body, to_bytes},
            http::{Request, header},
        };
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let password_file = dir.path().join("password");
        std::fs::write(&password_file, "Synthetic1-password").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&password_file, std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        let state = crate::auth::StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        crate::auth::create_account(&state, "alice", &password_file, None)
            .await
            .unwrap();
        let pool = crate::db::open(&dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let app = session_router(pool.clone(), state.clone());
        let other_cookie = login_cookie(&app, "alice", "Synthetic1-password").await;
        let changing_cookie = login_cookie(&app, "alice", "Synthetic1-password").await;

        let account = account_response(&app, &changing_cookie).await;
        let html = String::from_utf8(
            to_bytes(account.into_body(), 32 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("action=\"/account/password\""));
        assert!(html.contains("at least 7 characters"));
        let csrf = html
            .split("name=\"csrf_token\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();

        for (body, expected_message) in [
            (
                format!(
                    "csrf_token={csrf}&current_password=wrong&new_password=Newpass2!&confirm_password=Newpass2!"
                ),
                "Current password is incorrect.",
            ),
            (
                format!(
                    "csrf_token={csrf}&current_password=Synthetic1-password&new_password=Newpass2!&confirm_password=Different3!"
                ),
                "New password and confirmation do not match.",
            ),
            (
                format!(
                    "csrf_token={csrf}&current_password=Synthetic1-password&new_password=weakpass&confirm_password=weakpass"
                ),
                "password must contain an uppercase character",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/account/password")
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .header(header::COOKIE, &changing_cookie)
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = String::from_utf8(
                to_bytes(response.into_body(), 32 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert!(body.contains(expected_message), "{body}");
            assert!(!body.contains("value=\"Synthetic1-password\""));
        }

        let invalid_csrf = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/account/password")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &changing_cookie)
                    .body(Body::from("csrf_token=wrong&current_password=Synthetic1-password&new_password=Newpass2!&confirm_password=Newpass2!"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_csrf.status(), StatusCode::BAD_REQUEST);

        let changed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/account/password")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &changing_cookie)
                    .body(Body::from(format!("csrf_token={csrf}&current_password=Synthetic1-password&new_password=Newpass2!&confirm_password=Newpass2!")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(changed.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            changed.headers()[header::LOCATION],
            "/account?password_changed=1"
        );
        let changed_cookie = changed.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        assert_eq!(
            account_response(&app, &other_cookie).await.status(),
            StatusCode::UNAUTHORIZED
        );
        let account = account_response(&app, &changed_cookie).await;
        assert_eq!(account.status(), StatusCode::OK);
        assert!(
            state
                .auth
                .authenticate("alice", "Synthetic1-password")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            state
                .auth
                .authenticate("alice", "Newpass2!")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn account_preferences_validate_persist_and_drive_accessible_assets() {
        use axum::{
            body::{Body, to_bytes},
            http::{Request, header},
        };
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let password_file = dir.path().join("password");
        std::fs::write(&password_file, "Synthetic1-password").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&password_file, std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        let state = crate::auth::StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        crate::auth::create_account(&state, "alice", &password_file, None)
            .await
            .unwrap();
        let pool = crate::db::open(&dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let app = session_router(pool.clone(), state.clone());
        let cookie = login_cookie(&app, "alice", "Synthetic1-password").await;
        let account = account_response(&app, &cookie).await;
        let html = String::from_utf8(
            to_bytes(account.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("action=\"/account/preferences\""));
        assert!(html.contains("Browser-local display uses UTC"));
        let csrf = html
            .split("name=\"csrf_token\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let fields = "timezone_mode=iana&timezone_name=Europe%2FLondon&date_time_format=long-24&appearance=high-contrast&font_scale=125&page_size=25&live_interval_seconds=30&transcript_detail=expanded&code_wrap=1&default_sessions_view=active";

        let invalid = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/account/preferences")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(format!("csrf_token=wrong&{fields}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

        let invalid_timezone = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/account/preferences")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(format!(
                        "csrf_token={csrf}&{}",
                        fields.replace("Europe%2FLondon", "Invalid%2FZone")
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid_timezone.status(), StatusCode::BAD_REQUEST);
        let invalid_timezone = String::from_utf8(
            to_bytes(invalid_timezone.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(invalid_timezone.contains("timezone must be a valid IANA name"));

        let saved = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/account/preferences")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &cookie)
                    .body(Body::from(format!("csrf_token={csrf}&{fields}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(saved.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            saved.headers()[header::LOCATION],
            "/account?preferences_saved=1"
        );

        let account = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/account?preferences_saved=1")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let html = String::from_utf8(
            to_bytes(account.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        for selected in [
            "value=\"iana\" selected",
            "value=\"high-contrast\" selected",
            "value=\"25\" selected",
            "value=\"expanded\" selected",
            "Preferences saved.",
        ] {
            assert!(html.contains(selected), "missing {selected} in {html}");
        }

        for (path, expected) in [
            ("/assets/preferences.css", "font-size:125%"),
            ("/assets/preferences.js", "liveIntervalMs: 30000"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::COOKIE, &cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = String::from_utf8(
                to_bytes(response.into_body(), 32 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert!(body.contains(expected), "{path}: {body}");
        }

        sqlx::raw_sql(
            "INSERT INTO agents VALUES(1,'codex','Codex'); INSERT INTO hosts VALUES(1,'host'); INSERT INTO users VALUES(1,1,'user'); INSERT INTO agent_instances(id,agent_id,user_id,source_home) VALUES(1,1,1,'home');",
        )
        .execute(&pool)
        .await
        .unwrap();
        for index in 0..30 {
            sqlx::query("INSERT INTO sessions(instance_id,external_id,stable_id,title,started_at) VALUES(1,?,?,?,'2026-07-01T12:00:00Z')")
                .bind(format!("native-{index}"))
                .bind(format!("preference-{index:02}"))
                .bind(format!("Preference {index:02}"))
                .execute(&pool)
                .await
                .unwrap();
        }
        let default_sessions = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sessions")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let default_sessions = String::from_utf8(
            to_bytes(default_sessions.into_body(), 128 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(default_sessions.contains("No sessions match these filters."));

        let page = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sessions?archive=unknown")
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let page = String::from_utf8(
            to_bytes(page.into_body(), 256 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(page.matches("href=\"/sessions/preference-").count(), 25);
        assert!(page.contains("page=2"));
        assert!(page.len() < 256 * 1024, "preference page was too large");
    }

    #[tokio::test]
    async fn account_session_inventory_isolated_sanitised_and_revocable() {
        use axum::{
            body::{Body, to_bytes},
            http::{Request, header},
        };
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let password_file = dir.path().join("password");
        std::fs::write(&password_file, "Synthetic1-password").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&password_file, std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        let state = crate::auth::StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        crate::auth::create_account(&state, "alice", &password_file, None)
            .await
            .unwrap();
        crate::auth::create_account(&state, "bob", &password_file, None)
            .await
            .unwrap();
        let pool = crate::db::open(&dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let app = session_router(pool, state.clone());
        let old_cookie = login_cookie_with_agent(
            &app,
            "alice",
            "Synthetic1-password",
            Some("<script>private header</script>"),
        )
        .await;
        let current_cookie = login_cookie_with_agent(
            &app,
            "alice",
            "Synthetic1-password",
            Some("Mozilla/5.0 (X11; Linux x86_64) Firefox/140.0"),
        )
        .await;
        let bob_cookie = login_cookie_with_agent(
            &app,
            "bob",
            "Synthetic1-password",
            Some("Mozilla/5.0 (Windows NT 10.0) Chrome/140.0"),
        )
        .await;

        let alice_id: String = sqlx::query_scalar("SELECT id FROM accounts WHERE username='alice'")
            .fetch_one(&state.auth.pool)
            .await
            .unwrap();
        let old_id: String = sqlx::query_scalar(
            "SELECT id FROM account_sessions WHERE account_id=? AND user_agent_summary='Other client'",
        )
        .bind(&alice_id)
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
        sqlx::query("UPDATE account_sessions SET last_seen_at=unixepoch()-1900 WHERE id=?")
            .bind(&old_id)
            .execute(&state.auth.pool)
            .await
            .unwrap();

        let inventory = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/account/sessions")
                    .header(header::COOKIE, &current_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inventory.status(), StatusCode::OK);
        let html = String::from_utf8(
            to_bytes(inventory.into_body(), 128 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("Firefox on Linux"));
        assert!(html.contains("Other client"));
        assert!(html.contains("expired"));
        assert!(!html.contains("private header"));
        assert!(!html.contains("Chrome on Windows"));
        assert!(html.contains("Signed in"));
        assert!(html.contains("Network addresses are not collected"));
        let csrf = html
            .split("name=\"csrf_token\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();

        let third_cookie =
            login_cookie_with_agent(&app, "alice", "Synthetic1-password", Some("curl/8.12.1"))
                .await;
        let target_id: String = sqlx::query_scalar(
            "SELECT id FROM account_sessions WHERE account_id=? AND user_agent_summary='curl'",
        )
        .bind(&alice_id)
        .fetch_one(&state.auth.pool)
        .await
        .unwrap();
        let rejected = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/account/sessions/{target_id}"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &current_cookie)
                    .body(Body::from("csrf_token=wrong"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        let revoked = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/account/sessions/{target_id}"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &current_cookie)
                    .body(Body::from(format!("csrf_token={csrf}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::SEE_OTHER);
        let already_revoked = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/account/sessions/{target_id}"))
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &current_cookie)
                    .body(Body::from(format!("csrf_token={csrf}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(already_revoked.status(), StatusCode::NOT_FOUND);
        let revoked_request = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/account")
                    .header(header::COOKIE, &third_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoked_request.status(), StatusCode::UNAUTHORIZED);
        let current_request = account_response(&app, &current_cookie).await;
        assert_eq!(current_request.status(), StatusCode::OK);
        let bob_request = account_response(&app, &bob_cookie).await;
        assert_eq!(bob_request.status(), StatusCode::OK);

        let other_cookie = login_cookie_with_agent(
            &app,
            "alice",
            "Synthetic1-password",
            Some("Mozilla/5.0 (Macintosh) Safari/605.1"),
        )
        .await;
        let revoke_others = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/account/sessions/revoke-others")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &current_cookie)
                    .body(Body::from(format!("csrf_token={csrf}")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoke_others.status(), StatusCode::SEE_OTHER);
        let other_request = account_response(&app, &other_cookie).await;
        assert_eq!(other_request.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            account_response(&app, &current_cookie).await.status(),
            StatusCode::OK
        );

        let profile = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/account/profile")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, &current_cookie)
                    .body(Body::from(format!(
                        "csrf_token={csrf}&display_name=%3Cb%3EAlice%3C%2Fb%3E"
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(profile.status(), StatusCode::SEE_OTHER);
        let profile_page = account_response(&app, &current_cookie).await;
        let profile_html = String::from_utf8(
            to_bytes(profile_page.into_body(), 64 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(profile_html.contains("value=\"&#60;b&#62;Alice&#60;/b&#62;\""));
        assert!(!profile_html.contains("value=\"<b>Alice</b>\""));
        assert!(profile_html.contains("Changed today"));

        let preferences = crate::preferences::UserPreferences {
            page_size: 25,
            ..crate::preferences::UserPreferences::default()
        };
        crate::auth::update_preferences(&state.auth, &alice_id, &preferences)
            .await
            .unwrap();
        for index in 0..30 {
            sqlx::query("INSERT INTO account_sessions(id,account_id,tower_session_id,session_generation,logged_in_at,last_seen_at,absolute_expires_at,user_agent_summary,revoked_at) VALUES(?,?,?,?,unixepoch()+?,unixepoch()+?,unixepoch()+3600,'Synthetic client',unixepoch())")
                .bind(format!("synthetic-{index:02}"))
                .bind(&alice_id)
                .bind(format!("synthetic-tower-{index:02}"))
                .bind(0_i64)
                .bind(index + 100)
                .bind(index + 100)
                .execute(&state.auth.pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO security_events(account_id,event_type) VALUES(?,'login_failed')",
            )
            .bind(&alice_id)
            .execute(&state.auth.pool)
            .await
            .unwrap();
        }
        let bounded = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/account/sessions")
                    .header(header::COOKIE, &current_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bounded_html = String::from_utf8(
            to_bytes(bounded.into_body(), 256 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(bounded_html.matches("Synthetic client").count(), 25);
        assert_eq!(bounded_html.matches("Failed sign-in").count(), 25);
        assert!(bounded_html.contains("events_page=2"));
        assert!(bounded_html.contains("page=2&#38;events_page=1"));

        let _ = old_cookie;
    }

    #[tokio::test]
    async fn token_management_checks_csrf_password_isolation_and_one_time_reveal() {
        use axum::{
            body::{Body, to_bytes},
            http::{Request, header},
        };
        use tower::ServiceExt;
        let dir = tempfile::tempdir().unwrap();
        let password_file = dir.path().join("password");
        std::fs::write(&password_file, "Synthetic1-password").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&password_file, std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        let state = crate::auth::StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        crate::auth::create_account(&state, "alice", &password_file, None)
            .await
            .unwrap();
        crate::auth::create_account(&state, "bob", &password_file, None)
            .await
            .unwrap();
        let pool = crate::db::open(&dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let app = session_router(pool, state.clone());
        let cookie = login_cookie(&app, "alice", "Synthetic1-password").await;
        let bob_cookie = login_cookie(&app, "bob", "Synthetic1-password").await;
        async fn body(response: Response) -> String {
            String::from_utf8(
                to_bytes(response.into_body(), 256 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap()
        }
        async fn get_page(app: &Router, cookie: &str, path: &str) -> Response {
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
        }
        async fn post_form(app: &Router, cookie: &str, path: &str, fields: String) -> Response {
            app.clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .header(header::COOKIE, cookie)
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(Body::from(fields))
                        .unwrap(),
                )
                .await
                .unwrap()
        }
        let page = body(get_page(&app, &cookie, "/account/tokens").await).await;
        let csrf = page
            .split("name=\"csrf_token\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let fields = format!(
            "csrf_token={csrf}&current_password=Synthetic1-password&description=%3Cscript%3ESynthetic%3C%2Fscript%3E&scope=reports%3Aread&days=30"
        );
        assert_eq!(
            post_form(
                &app,
                &cookie,
                "/account/tokens",
                fields.replace(csrf, "wrong")
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_form(
                &app,
                &cookie,
                "/account/tokens",
                fields.replace("days=30", "days=91")
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_form(
                &app,
                &cookie,
                "/account/tokens",
                fields.replace("Synthetic1-password", "wrong")
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        let response = post_form(&app, &cookie, "/account/tokens", fields).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let reveal = body(response).await;
        let bearer = reveal
            .split("<pre>")
            .nth(1)
            .unwrap()
            .split("</pre>")
            .next()
            .unwrap()
            .to_owned();
        assert!(bearer.starts_with("al_pat_"));
        assert!(reveal.contains("&#60;script&#62;Synthetic&#60;/script&#62;"));
        assert!(!reveal.contains("<script>Synthetic</script>"));
        let listing = body(get_page(&app, &cookie, "/account/tokens").await).await;
        assert!(!listing.contains(&bearer));
        assert!(!listing.contains("al_pat_"));
        let bob_page = body(get_page(&app, &bob_cookie, "/account/tokens").await).await;
        assert!(!bob_page.contains("Synthetic"));
        assert!(!bob_page.contains("value=\"metrics:read\""));
        let bob_csrf = bob_page
            .split("name=\"csrf_token\" value=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert_eq!(post_form(&app,&bob_cookie,"/account/tokens",format!("csrf_token={bob_csrf}&current_password=Synthetic1-password&description=Synthetic&scope=metrics%3Aread&days=30")).await.status(),StatusCode::FORBIDDEN);
        let id = &bearer[7..23];
        assert_eq!(
            post_form(
                &app,
                &bob_cookie,
                &format!("/account/tokens/{id}"),
                format!("csrf_token={bob_csrf}")
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            post_form(
                &app,
                &cookie,
                &format!("/account/tokens/{id}"),
                "csrf_token=wrong".into()
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post_form(
                &app,
                &cookie,
                &format!("/account/tokens/{id}"),
                format!("csrf_token={csrf}")
            )
            .await
            .status(),
            StatusCode::SEE_OTHER
        );
        let revoked = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/unknown/postmortem")
                    .header(header::COOKIE, &cookie)
                    .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
        assert!(!body(revoked).await.contains(&bearer));
        let alice = state
            .auth
            .authenticate("alice", "Synthetic1-password")
            .await
            .unwrap()
            .unwrap()
            .0;
        crate::auth::update_preferences(
            &state.auth,
            &alice.id,
            &crate::preferences::UserPreferences {
                page_size: 25,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        for index in 0..30 {
            sqlx::query("INSERT INTO personal_access_tokens(id,account_id,description,scope,digest,account_generation,created_at,expires_at) VALUES(?,?,'Bounded synthetic token','reports:read',zeroblob(32),0,unixepoch()+?,unixepoch()+3600)")
                .bind(format!("bounded-{index:02}")).bind(&alice.id).bind(index+10).execute(&state.auth.pool).await.unwrap();
        }
        let bounded = body(get_page(&app, &cookie, "/account/tokens").await).await;
        assert_eq!(bounded.matches("Bounded synthetic token").count(), 25);
        assert!(bounded.contains("/account/tokens?page=2"));
        let second = body(get_page(&app, &cookie, "/account/tokens?page=2").await).await;
        assert_eq!(second.matches("Bounded synthetic token").count(), 5);
    }

    #[tokio::test]
    async fn source_permissions_scope_every_data_surface_and_update_immediately() {
        use axum::{
            body::{Body, to_bytes},
            http::{Request, header},
        };
        use tower::ServiceExt;

        async fn get(app: &Router, cookie: &str, path: &str) -> (StatusCode, String) {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let body = String::from_utf8(
                to_bytes(response.into_body(), 256 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            (status, body)
        }

        let dir = tempfile::tempdir().unwrap();
        let password_file = dir.path().join("password");
        std::fs::write(&password_file, "Synthetic1-password").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&password_file, std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        let state = crate::auth::StateStore::open(&dir.path().join("state.sqlite"))
            .await
            .unwrap();
        crate::auth::create_account(&state, "admin", &password_file, None)
            .await
            .unwrap();
        crate::auth::create_account(&state, "viewer", &password_file, None)
            .await
            .unwrap();
        let source_a = format!("src_{}", "a".repeat(64));
        let source_b = format!("src_{}", "b".repeat(64));
        let pool = crate::db::open(&dir.path().join("index.sqlite"))
            .await
            .unwrap();
        sqlx::query("INSERT INTO agents(id,kind,display_name) VALUES(1,'codex','Codex')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql("INSERT INTO hosts(id,name) VALUES(1,'host-a'),(2,'host-b'); INSERT INTO users(id,host_id,os_user) VALUES(1,1,'user-a'),(2,2,'user-b'); INSERT INTO ingest_runs(id,finished_at,status,files_seen,errors) VALUES(1,CURRENT_TIMESTAMP,'complete',2,0);")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO agent_instances(id,agent_id,user_id,source_home,source_key) VALUES(1,1,1,'/synthetic/a',?),(2,1,2,'/synthetic/b',?)")
            .bind(&source_a)
            .bind(&source_b)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql("INSERT INTO source_files(id,instance_id,ingest_run_id,path,kind,observed_at,parse_status) VALUES(1,1,1,'a.jsonl','sessions',CURRENT_TIMESTAMP,'ok'),(2,2,1,'b.jsonl','sessions',CURRENT_TIMESTAMP,'ok'); INSERT INTO sessions(id,instance_id,source_file_id,external_id,stable_id,started_at,status,title) VALUES(1,1,1,'native-a','ses_allowed','2026-01-01T00:00:00Z','complete','Allowed synthetic session'),(2,2,2,'native-b','ses_denied','2026-01-02T00:00:00Z','complete','Denied synthetic session'); INSERT INTO messages(session_id,sequence,role,content) VALUES(1,1,'user','allowed searchable phrase'),(2,1,'user','denied searchable phrase'); INSERT INTO tool_calls(id,session_id,sequence,call_id,tool_name,arguments) VALUES(1,1,3,'call-a','allowed-tool','allowed arguments'),(2,2,3,'call-b','denied-tool','denied arguments'); INSERT INTO tool_results(session_id,source_sequence,call_id,tool_call_id,output,outcome) VALUES(1,4,'call-a',1,'allowed output','observed'),(2,4,'call-b',2,'denied output','observed'); INSERT INTO usage_events(session_id,source_sequence,sequence_known,timestamp,delta_total_tokens,cumulative_present) VALUES(1,2,1,'2026-01-01T00:01:00Z',11,1),(2,2,1,'2026-01-02T00:01:00Z',99,1); INSERT INTO turns(session_id,ordinal,start_sequence,start_kind,end_sequence,outcome) VALUES(1,0,1,'user',2,'interrupted'),(2,0,1,'user',2,'interrupted');")
            .execute(&pool)
            .await
            .unwrap();
        let app = session_router(pool, state.clone());
        let admin_cookie = login_cookie(&app, "admin", "Synthetic1-password").await;
        let viewer_cookie = login_cookie(&app, "viewer", "Synthetic1-password").await;

        for cookie in [&admin_cookie, &viewer_cookie] {
            for path in [
                "/future-data",
                "/account/tokens/future/action",
                "/refresh",
                "/annotations",
            ] {
                assert_eq!(
                    get(&app, cookie, path).await.0,
                    StatusCode::FORBIDDEN,
                    "{path}"
                );
            }
            for (method, path) in [
                (axum::http::Method::POST, "/sessions"),
                (axum::http::Method::DELETE, "/account"),
                (axum::http::Method::GET, "/account/sessions/revoke-others"),
            ] {
                let response = app
                    .clone()
                    .oneshot(
                        Request::builder()
                            .method(method)
                            .uri(path)
                            .header(header::COOKIE, cookie)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::FORBIDDEN, "{path}");
            }
            assert_eq!(
                get(&app, cookie, "/account/sessions").await.0,
                StatusCode::OK
            );
            assert_eq!(get(&app, cookie, "/account/tokens").await.0, StatusCode::OK);
        }

        let (status, empty_viewer) = get(&app, &viewer_cookie, "/sessions").await;
        assert_eq!(status, StatusCode::OK);
        assert!(!empty_viewer.contains("Allowed synthetic session"));
        assert!(!empty_viewer.contains("Denied synthetic session"));
        let (_, account) = get(&app, &viewer_cookie, "/account").await;
        assert!(account.contains("<dd>viewer</dd>"));
        assert!(account.contains("<dd>0</dd>"));
        crate::auth::grant_source(&state, "viewer", &source_a)
            .await
            .unwrap();

        let viewer_user = state
            .auth
            .authenticate("viewer", "Synthetic1-password")
            .await
            .unwrap()
            .unwrap()
            .0;
        let admin_user = state
            .auth
            .authenticate("admin", "Synthetic1-password")
            .await
            .unwrap()
            .unwrap()
            .0;
        let report_token = crate::tokens::create(
            &state.auth,
            &viewer_user,
            "Synthetic reports",
            "reports:read",
            1,
        )
        .await
        .unwrap();
        let status_token = crate::tokens::create(
            &state.auth,
            &viewer_user,
            "Synthetic status",
            "content:read",
            1,
        )
        .await
        .unwrap();
        let metrics_token = crate::tokens::create(
            &state.auth,
            &admin_user,
            "Synthetic metrics",
            "metrics:read",
            1,
        )
        .await
        .unwrap();
        async fn bearer_request(
            app: &Router,
            token: &str,
            cookie: &str,
            path: &str,
        ) -> (StatusCode, String) {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            let status = response.status();
            let body = String::from_utf8(
                to_bytes(response.into_body(), 256 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert!(
                !body.contains(token),
                "bearer material must not appear in responses"
            );
            (status, body)
        }
        let (status, report) = bearer_request(
            &app,
            &report_token,
            &admin_cookie,
            "/api/sessions/ses_allowed/postmortem",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(serde_json::from_str::<serde_json::Value>(&report).is_ok());
        assert!(!report.contains("allowed searchable phrase"));
        for path in [
            "/api/sessions/ses_denied/postmortem",
            "/api/sessions/unknown/postmortem",
        ] {
            let (status, error) = bearer_request(&app, &report_token, &admin_cookie, path).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&error).unwrap()["error"],
                "not_found"
            );
        }
        assert_eq!(
            bearer_request(
                &app,
                &status_token,
                &admin_cookie,
                "/api/sessions/ses_allowed/live-status"
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            bearer_request(
                &app,
                &status_token,
                &admin_cookie,
                "/api/sessions/ses_denied/live-status"
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            bearer_request(&app, &metrics_token, &viewer_cookie, "/metrics")
                .await
                .0,
            StatusCode::OK
        );
        for path in [
            "/metrics",
            "/api/sessions/ses_allowed/live-status",
            "/account/tokens",
            "/sessions/ses_allowed",
        ] {
            assert_eq!(
                bearer_request(&app, &report_token, &admin_cookie, path)
                    .await
                    .0,
                StatusCode::FORBIDDEN,
                "{path}"
            );
        }
        assert_eq!(
            get(&app, &admin_cookie, "/api/sessions/ses_allowed/postmortem")
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            bearer_request(
                &app,
                "invalid",
                &admin_cookie,
                "/api/sessions/ses_allowed/postmortem"
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
        let duplicate = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions/ses_allowed/postmortem")
                    .header(header::AUTHORIZATION, format!("Bearer {report_token}"))
                    .header(header::AUTHORIZATION, format!("Bearer {report_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(duplicate.status(), StatusCode::UNAUTHORIZED);

        let (status, admin_sessions) = get(&app, &admin_cookie, "/sessions").await;
        assert_eq!(status, StatusCode::OK);
        assert!(admin_sessions.contains("Allowed synthetic session"));
        assert!(admin_sessions.contains("Denied synthetic session"));

        let (status, viewer_sessions) = get(&app, &viewer_cookie, "/sessions").await;
        assert_eq!(status, StatusCode::OK);
        assert!(viewer_sessions.contains("Allowed synthetic session"));
        assert!(!viewer_sessions.contains("Denied synthetic session"));
        assert!(viewer_sessions.contains("host-a"));
        assert!(!viewer_sessions.contains("host-b"));

        let (_, search) = get(&app, &viewer_cookie, "/sessions?q=searchable").await;
        assert!(search.contains("Allowed synthetic session"));
        assert!(search.contains("allowed"));
        assert!(!search.contains("Denied synthetic session"));
        assert!(!search.contains("denied"));
        let (_, usage) = get(&app, &viewer_cookie, "/usage?group=session").await;
        assert!(usage.contains("Allowed synthetic session"));
        assert!(!usage.contains("Denied synthetic session"));
        assert!(!usage.contains(">99<"));
        let (_, diagnostics) = get(&app, &viewer_cookie, "/diagnostics").await;
        assert!(diagnostics.contains("Allowed synthetic session"));
        assert!(!diagnostics.contains("Denied synthetic session"));

        for path in [
            "/sessions/ses_denied",
            "/sessions/ses_denied/live-status",
            "/sessions/ses_denied/turns",
            "/sessions/ses_denied/generated-images",
            "/sessions/ses_denied/generated-images/exec-private.png",
            "/sessions/ses_denied/postmortem/json",
            "/sessions/ses_denied/messages/1",
            "/sessions/ses_denied/tools/3",
            "/sessions/ses_denied/results/4",
        ] {
            assert_eq!(
                get(&app, &viewer_cookie, path).await.0,
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
        for path in [
            "/sessions/ses_allowed",
            "/sessions/ses_allowed/live-status",
            "/sessions/ses_allowed/turns",
            "/sessions/ses_allowed/generated-images",
            "/sessions/ses_allowed/postmortem/json",
            "/sessions/ses_allowed/messages/1",
            "/sessions/ses_allowed/tools/3",
            "/sessions/ses_allowed/results/4",
        ] {
            assert_eq!(
                get(&app, &viewer_cookie, path).await.0,
                StatusCode::OK,
                "{path}"
            );
        }
        assert_eq!(
            get(&app, &viewer_cookie, "/health").await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get(&app, &viewer_cookie, "/metrics").await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            get(&app, &viewer_cookie, "/future-data").await.0,
            StatusCode::FORBIDDEN
        );
        use base64::{Engine, engine::general_purpose::STANDARD};
        let viewer_basic = format!("Basic {}", STANDARD.encode("viewer:Synthetic1-password"));
        let basic_metrics = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .header(header::AUTHORIZATION, viewer_basic)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(basic_metrics.status(), StatusCode::FORBIDDEN);
        assert_eq!(get(&app, &admin_cookie, "/health").await.0, StatusCode::OK);
        assert_eq!(get(&app, &admin_cookie, "/metrics").await.0, StatusCode::OK);

        crate::auth::revoke_source(&state, "viewer", &source_a)
            .await
            .unwrap();
        assert_eq!(
            bearer_request(
                &app,
                &report_token,
                &admin_cookie,
                "/api/sessions/ses_allowed/postmortem"
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            bearer_request(
                &app,
                &status_token,
                &admin_cookie,
                "/api/sessions/ses_allowed/live-status"
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(&app, &viewer_cookie, "/sessions/ses_allowed").await.0,
            StatusCode::NOT_FOUND
        );
        let team = crate::access::create(&state, "<b>Synthetic team</b>")
            .await
            .unwrap();
        crate::access::membership(&state, &team, "viewer", true)
            .await
            .unwrap();
        crate::access::grant(&state, &team, &source_a, true)
            .await
            .unwrap();
        assert_eq!(
            get(&app, &viewer_cookie, "/sessions/ses_allowed").await.0,
            StatusCode::OK
        );
        let (_, access_page) = get(&app, &viewer_cookie, "/account/access").await;
        assert!(access_page.contains("&#60;b&#62;Synthetic team&#60;/b&#62;"));
        assert!(access_page.contains(&team));
        assert!(
            !get(&app, &admin_cookie, "/account/access")
                .await
                .1
                .contains(&team)
        );
        crate::auth::set_role(&state, "viewer", crate::auth::AccountRole::Auditor)
            .await
            .unwrap();
        for path in [
            "/usage",
            "/diagnostics",
            "/account/access",
            "/sessions/ses_allowed/postmortem/json",
        ] {
            assert_eq!(
                get(&app, &viewer_cookie, path).await.0,
                StatusCode::OK,
                "{path}"
            );
        }
        for path in [
            "/sessions",
            "/sessions/ses_allowed",
            "/sessions/ses_allowed/messages/1",
            "/sessions/ses_allowed/tools/3",
            "/sessions/ses_allowed/results/4",
            "/sessions/ses_allowed/generated-images",
            "/health",
            "/metrics",
        ] {
            assert_eq!(
                get(&app, &viewer_cookie, path).await.0,
                StatusCode::FORBIDDEN,
                "{path}"
            );
        }
        let home = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::COOKIE, &viewer_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(home.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(home.headers()[header::LOCATION], "/usage");
        assert_eq!(
            bearer_request(
                &app,
                &report_token,
                &admin_cookie,
                "/api/sessions/ses_allowed/postmortem"
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            bearer_request(
                &app,
                &status_token,
                &admin_cookie,
                "/api/sessions/ses_allowed/live-status"
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        crate::auth::set_role(&state, "viewer", crate::auth::AccountRole::Operator)
            .await
            .unwrap();
        assert_eq!(get(&app, &viewer_cookie, "/health").await.0, StatusCode::OK);
        assert_eq!(
            get(&app, &viewer_cookie, "/metrics").await.0,
            StatusCode::OK
        );
        assert_eq!(
            get(&app, &viewer_cookie, "/sessions/ses_allowed").await.0,
            StatusCode::OK
        );
        crate::access::membership(&state, &team, "viewer", false)
            .await
            .unwrap();
        assert_eq!(
            get(&app, &viewer_cookie, "/sessions/ses_allowed").await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            bearer_request(
                &app,
                &report_token,
                &admin_cookie,
                "/api/sessions/ses_allowed/postmortem"
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        for path in ["/admin/accounts", "/admin/audit", "/refresh"] {
            assert_eq!(
                get(&app, &viewer_cookie, path).await.0,
                StatusCode::FORBIDDEN
            );
        }
        crate::auth::grant_source(&state, "viewer", &source_b)
            .await
            .unwrap();
        assert_eq!(
            get(&app, &viewer_cookie, "/sessions/ses_denied").await.0,
            StatusCode::OK
        );
        crate::auth::set_role(&state, "viewer", crate::auth::AccountRole::Admin)
            .await
            .unwrap();
        assert_eq!(
            get(&app, &viewer_cookie, "/metrics").await.0,
            StatusCode::OK
        );
        crate::auth::set_role(&state, "viewer", crate::auth::AccountRole::Viewer)
            .await
            .unwrap();
        assert_eq!(
            get(&app, &viewer_cookie, "/sessions/ses_allowed").await.0,
            StatusCode::NOT_FOUND
        );
        let viewer = state
            .auth
            .authenticate("viewer", "Synthetic1-password")
            .await
            .unwrap()
            .unwrap()
            .0;
        crate::auth::update_preferences(
            &state.auth,
            &viewer.id,
            &crate::preferences::UserPreferences {
                page_size: 25,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        for index in 0..30 {
            crate::auth::grant_source(&state, "viewer", &format!("src_{index:064x}"))
                .await
                .unwrap();
        }
        let (_, first) = get(&app, &viewer_cookie, "/account/access").await;
        assert_eq!(first.matches("<td><code>src_").count(), 25);
        assert!(first.contains("/account/access?page=2"));
        let (_, second) = get(&app, &viewer_cookie, "/account/access?page=2").await;
        assert_eq!(second.matches("<td><code>src_").count(), 6);
    }

    #[test]
    fn recognized_initialization_is_closed_regardless_of_count_or_order() {
        let messages = vec![
            message(
                0,
                "developer",
                "<skills_instructions>redacted</skills_instructions>",
            ),
            message(1, "developer", "You are a redacted test agent."),
            message(
                2,
                "developer",
                "<multi_agent_mode>redacted</multi_agent_mode>",
            ),
            message(
                3,
                "developer",
                "<apps_instructions>redacted</apps_instructions>",
            ),
            message(
                4,
                "developer",
                "<plugins_instructions>redacted</plugins_instructions>",
            ),
            message(
                5,
                "developer",
                "<recommended_plugins>redacted</recommended_plugins>",
            ),
            message(6, "user", "<app-context>redacted</app-context>"),
            message(
                7,
                "user",
                "<image_resize_notice>redacted</image_resize_notice>",
            ),
            message(
                8,
                "user",
                "<environment_context>redacted</environment_context>",
            ),
            message(9, "user", "actual request"),
        ];
        let html = SessionTemplate {
            session: SessionRow {
                id: 1,
                stable_id: "stable-session".into(),
                agent_kind: "codex".into(),
                external_id: "redacted-session".into(),
                title: "Redacted title".into(),
                started_at: None,
                last_activity: None,
                model: None,
                status: None,
                cwd: None,
                project: None,
                archive_state: "unknown".into(),
                hostname: "fixture-host".into(),
                os_user: "fixture-user".into(),
                message_count: 10,
                match_sequence: None,
                match_snippet: None,
            },
            segments: group_transcript(messages),
            generated_images: vec![],
            event_options: vec![],
            show_origin: true,
            pagination: no_pagination(),
            live: test_live_status(),
        }
        .render()
        .expect("template should render");

        assert!(html.contains("<strong>Available skills</strong>"));
        assert!(html.contains("<strong>Initial agent configuration</strong>"));
        assert!(html.contains("<strong>Multi-agent mode</strong>"));
        assert!(html.contains("<strong>App instructions</strong>"));
        assert!(html.contains("<strong>Plugin instructions</strong>"));
        assert!(html.contains("<strong>Recommended plugins</strong>"));
        assert!(html.contains("<strong>App context</strong>"));
        assert!(html.contains("<strong>Image resize notice</strong>"));
        assert!(html.contains("<strong>Environment context</strong>"));
        assert_eq!(html.matches("<details class=\"message").count(), 10);
        assert_eq!(html.matches(" open>").count(), 1);
    }

    #[test]
    fn hermes_instructions_and_failures_render_with_ordinary_conversation() {
        for resumed in [false, true] {
            let mut messages = Vec::new();
            if resumed {
                messages.push(message(0, "user", "Earlier synthetic request"));
                messages.push(message(1, "assistant", "Earlier synthetic response"));
            }
            messages.push(message(2, "developer", "Synthetic Hermes instructions"));
            messages.push(message(3, "user", "Ordinary synthetic request"));
            messages.push(message(4, "assistant", "Ordinary synthetic response"));
            messages.push(message(
                5,
                "error",
                "Hermes provider request failed after retry attempts.",
            ));
            let html = SessionTemplate {
                session: SessionRow {
                    id: 1,
                    stable_id: "stable-hermes-session".into(),
                    agent_kind: "hermes".into(),
                    external_id: "synthetic-hermes-session".into(),
                    title: "Synthetic Hermes title".into(),
                    started_at: None,
                    last_activity: None,
                    model: None,
                    status: None,
                    cwd: None,
                    project: None,
                    archive_state: "unknown".into(),
                    hostname: "synthetic-host".into(),
                    os_user: "synthetic-user".into(),
                    message_count: messages.len() as i64,
                    match_sequence: None,
                    match_snippet: None,
                },
                segments: group_transcript_for_source(messages, "hermes", false),
                generated_images: vec![],
                event_options: vec![],
                show_origin: true,
                pagination: no_pagination(),
                live: test_live_status(),
            }
            .render()
            .expect("Hermes transcript should render");
            assert!(html.contains("<strong>Hermes instructions</strong>"));
            assert!(html.contains("<strong>Hermes request failure</strong>"));
            assert!(html.contains("Ordinary synthetic request"));
            assert!(html.contains("Ordinary synthetic response"));
            assert!(html.contains(
                "<details class=\"message developer\"><summary><strong>Hermes instructions</strong>"
            ));
            assert!(html.contains(
                "<details class=\"message error\" open><summary><strong>Hermes request failure</strong>"
            ));
        }
    }

    #[test]
    fn multi_agent_role_is_collapsed_in_rendered_initial_and_resumed_transcripts() {
        for resumed in [false, true] {
            let mut messages = Vec::new();
            if resumed {
                messages.push(message(0, "user", "synthetic request"));
                messages.push(message(1, "assistant", "synthetic reply"));
            }
            messages.push(message(
                2,
                "developer",
                "  <multi_agent_role>synthetic setup</multi_agent_role>",
            ));
            messages.push(message(3, "user", "Explain <multi_agent_role> please"));
            messages.push(message(4, "assistant", "<multi_agent_role>example output"));
            let html = SessionTemplate {
                session: SessionRow {
                    id: 1,
                    stable_id: "stable-session".into(),
                    agent_kind: "codex".into(),
                    external_id: "synthetic-session".into(),
                    title: "Synthetic title".into(),
                    started_at: None,
                    last_activity: None,
                    model: None,
                    status: None,
                    cwd: None,
                    project: None,
                    archive_state: "unknown".into(),
                    hostname: "synthetic-host".into(),
                    os_user: "synthetic-user".into(),
                    message_count: messages.len() as i64,
                    match_sequence: None,
                    match_snippet: None,
                },
                segments: group_transcript(messages),
                generated_images: vec![],
                event_options: vec![],
                show_origin: true,
                pagination: no_pagination(),
                live: test_live_status(),
            }
            .render()
            .expect("transcript should render");
            assert!(
                html.contains("<details class=\"message developer\"><summary><strong>Multi-agent role</strong>"),
                "setup must render collapsed with a descriptive title (resumed={resumed})"
            );
            assert!(
                html.contains("synthetic setup"),
                "setup content must remain available"
            );
            assert_eq!(html.matches(" open>").count(), if resumed { 4 } else { 2 });
            assert!(html.contains("<details class=\"message user\" open>"));
            assert!(html.contains("<details class=\"message assistant\" open>"));
        }
    }

    #[test]
    fn initialization_markers_after_user_input_are_not_minimized() {
        let segments = group_transcript(vec![
            message(0, "user", "actual request"),
            message(1, "user", "You are part of the requested prose."),
        ]);
        assert!(segments.iter().all(|segment| segment.initially_open));
    }

    #[test]
    fn resume_setup_is_collapsed_but_notices_and_conversation_remain_visible() {
        let markers = [
            (
                "user",
                "# AGENTS.md instructions for /synthetic",
                "Repository instructions",
            ),
            ("developer", "<permissions instructions>", "Permissions"),
            ("user", "<environment_context>", "Environment context"),
            ("developer", "<skills_instructions>", "Available skills"),
            ("developer", "<apps_instructions>", "App instructions"),
            ("developer", "<plugins_instructions>", "Plugin instructions"),
            ("developer", "<recommended_plugins>", "Recommended plugins"),
            ("user", "<app-context>", "App context"),
            ("user", "<image_resize_notice>", "Image resize notice"),
            ("developer", "<multi_agent_mode>", "Multi-agent mode"),
            ("developer", "<multi_agent_role>", "Multi-agent role"),
            ("developer", "<collaboration_mode>", "Collaboration mode"),
            ("developer", "<model_switch>", "Model switch"),
        ];
        for (role, prefix, title) in markers {
            let initial = group_transcript(vec![message(0, role, prefix)]);
            assert_eq!(initial[0].title, title);
            assert!(!initial[0].initially_open);
            let segments = group_transcript(vec![
                message(0, "user", "initial request"),
                message(1, "assistant", "initial reply"),
                message(2, role, &format!("  {prefix}\nsynthetic setup")),
                message(3, "user", "resumed request"),
            ]);
            assert_eq!(segments[2].title, title);
            assert!(!segments[2].initially_open);
            assert!(segments[3].initially_open);
        }
        for (prefix, title) in [
            ("<turn_aborted>", "Turn interrupted"),
            ("<user_action>", "User action"),
        ] {
            let segments = group_transcript(vec![message(0, "user", prefix)]);
            assert_eq!(segments[0].title, title);
            assert!(segments[0].initially_open);
        }
    }

    #[test]
    fn quoted_markers_and_unknown_tags_are_not_setup() {
        let segments = group_transcript(vec![
            message(0, "user", "Explain <environment_context> please"),
            message(1, "user", "<example>ordinary input</example>"),
            message(2, "assistant", "<permissions instructions>example output"),
            message(3, "user", "Explain <app-context> please"),
            message(
                4,
                "assistant",
                "<recommended_plugins>example output</recommended_plugins>",
            ),
            message(5, "user", "Explain <image_resize_notice> please"),
            message(
                6,
                "assistant",
                "<apps_instructions>example output</apps_instructions>",
            ),
            message(
                7,
                "assistant",
                "<plugins_instructions>example output</plugins_instructions>",
            ),
        ]);
        assert!(segments.iter().all(|segment| segment.initially_open));
        assert_eq!(segments[2].title, "assistant");
    }

    #[test]
    fn consecutive_assistant_replies_share_a_segment_as_paragraphs() {
        let messages = vec![
            MessageRow {
                sequence: 10,
                timestamp: Some("2026-01-01T00:00:00Z".into()),
                role: "user".into(),
                content: "question".into(),
                content_truncated: false,
            },
            MessageRow {
                sequence: 11,
                timestamp: Some("2026-01-01T00:00:01Z".into()),
                role: "assistant".into(),
                content: "first reply".into(),
                content_truncated: false,
            },
            MessageRow {
                sequence: 12,
                timestamp: Some("2026-01-01T00:00:02Z".into()),
                role: "assistant".into(),
                content: "second reply".into(),
                content_truncated: false,
            },
            MessageRow {
                sequence: 13,
                timestamp: None,
                role: "user".into(),
                content: "follow-up".into(),
                content_truncated: false,
            },
        ];

        let segments = group_transcript(messages);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[1].role, "assistant");
        assert_eq!(segments[1].contents.len(), 2);
        assert_eq!(segments[2].contents.len(), 1);
    }

    #[test]
    fn tool_events_separate_otherwise_consecutive_assistant_replies() {
        let rows = vec![
            TimelineRow {
                event_type: "message".into(),
                sequence: 10,
                sequence_known: true,
                timestamp: None,
                role: Some("assistant".into()),
                content: "before tool".into(),
                total_chars: 11,
                tool_name: None,
                call_id: None,
                outcome: None,
            },
            TimelineRow {
                event_type: "tool_call".into(),
                sequence: 11,
                sequence_known: true,
                timestamp: None,
                role: None,
                content: "{}".into(),
                total_chars: 2,
                tool_name: Some("runner".into()),
                call_id: Some("call".into()),
                outcome: Some("output observed".into()),
            },
            TimelineRow {
                event_type: "message".into(),
                sequence: 12,
                sequence_known: true,
                timestamp: None,
                role: Some("assistant".into()),
                content: "after tool".into(),
                total_chars: 10,
                tool_name: None,
                call_id: None,
                outcome: None,
            },
        ];

        let timeline = present_timeline(rows, crate::sources::codex::KIND, "stable", true);
        assert_eq!(timeline.len(), 3);
        assert_eq!(timeline[0].contents.len(), 1);
        assert_eq!(timeline[2].contents.len(), 1);
    }

    #[test]
    fn later_sections_do_not_reclassify_conversation_as_initial_setup() {
        let segments = group_transcript_for_source(
            vec![message(50, "user", "You are seeing ordinary conversation")],
            crate::sources::codex::KIND,
            true,
        );

        assert_eq!(segments[0].title, "user");
        assert!(segments[0].initially_open);
    }

    #[test]
    fn source_specific_markers_are_ordinary_for_unknown_agents() {
        let segments = group_transcript_for_source(
            vec![message(
                0,
                "developer",
                "<skills_instructions>generic content</skills_instructions>",
            )],
            "other-agent",
            false,
        );

        assert_eq!(segments[0].title, "developer");
        assert!(segments[0].initially_open);
    }

    #[test]
    fn sessions_page_renders_known_filter_values_and_separate_identity_columns() {
        let html = SessionsTemplate {
            sessions: vec![SessionRow {
                id: 1,
                stable_id: "stable-session".into(),
                agent_kind: "synthetic".into(),
                external_id: "redacted-session".into(),
                title: "Redacted title".into(),
                started_at: None,
                last_activity: None,
                model: Some("fixture-model".into()),
                status: Some("complete".into()),
                cwd: None,
                project: Some("Fixture project".into()),
                archive_state: "active".into(),
                hostname: "fixture-host".into(),
                os_user: "fixture-user".into(),
                message_count: 2,
                match_sequence: None,
                match_snippet: None,
            }],
            options: FilterOptions {
                hosts: select_options(vec!["fixture-host".into()], Some("fixture-host")),
                users: select_options(vec!["fixture-user".into()], None),
                models: select_options(vec!["fixture-model".into()], None),
                statuses: select_options(vec!["complete".into()], None),
                agents: select_options(vec!["synthetic".into()], None),
                projects: select_options(vec!["Fixture project".into()], None),
                archives: select_options(vec!["active".into()], None),
            },
            show_origins: true,
            pagination: Pagination {
                page: 1,
                previous_href: None,
                next_href: None,
            },
            search: None,
            from: None,
            to: None,
            timezone: "+00:00".into(),
            cron_all: false,
            cron_hide: false,
            empty_message: "No sessions have been indexed.",
        }
        .render()
        .expect("template should render");

        assert!(html.contains("<th>Host</th><th>User</th>"));
        assert!(html.contains("href=\"/sessions/stable-session\">Redacted title</a>"));
        assert!(html.contains("<option value=\"fixture-host\" selected>fixture-host</option>"));
        assert!(html.contains("<option value=\"fixture-user\">fixture-user</option>"));
        assert!(html.contains("<td>fixture-host</td><td>fixture-user</td>"));
    }

    #[test]
    fn empty_filter_values_are_normalized_to_no_filter() {
        let mut filters = Filters {
            host: Some(String::new()),
            user: Some(String::new()),
            model: Some(String::new()),
            status: Some(String::new()),
            ..Filters::default()
        };

        filters.normalize();

        assert!(filters.host.is_none());
        assert!(filters.user.is_none());
        assert!(filters.model.is_none());
        assert!(filters.status.is_none());
    }

    #[test]
    fn pagination_links_encode_and_preserve_active_filters() {
        let filters = Filters {
            q: Some("needle text".into()),
            host: Some("host name".into()),
            user: Some("user/name".into()),
            model: Some("model &+".into()),
            status: Some("完了".into()),
            agent: Some("codex".into()),
            project: Some("project one".into()),
            archive: Some("active".into()),
            cron: Some("all".into()),
            from: Some("2026-01-01".into()),
            to: Some("2026-01-31".into()),
            page: Some(2),
            browser_timezone: Some("Europe/London".into()),
        };

        assert_eq!(
            filters.href(3),
            "/sessions?q=needle%20text&host=host%20name&user=user%2Fname&model=model%20%26%2B&status=%E5%AE%8C%E4%BA%86&agent=codex&project=project%20one&archive=active&cron=all&from=2026-01-01&to=2026-01-31&browser_timezone=Europe%2FLondon&page=3"
        );
    }

    #[test]
    fn page_numbers_are_bounded() {
        for (page, expected) in [
            (None, 1),
            (Some(1), 1),
            (Some(MAX_SESSION_PAGE), MAX_SESSION_PAGE),
        ] {
            assert_eq!(
                Filters {
                    host: None,
                    user: None,
                    model: None,
                    status: None,
                    page,
                    ..Filters::default()
                }
                .page_number()
                .ok(),
                Some(expected)
            );
        }
        for page in [0, MAX_SESSION_PAGE + 1] {
            let error = Filters {
                host: None,
                user: None,
                model: None,
                status: None,
                page: Some(page),
                ..Filters::default()
            }
            .page_number()
            .unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            assert_eq!(error.public_message, "invalid page");
        }
    }

    #[test]
    fn date_filters_validate_real_gregorian_dates() {
        for date in ["2024-02-29", "2026-01-01", "9999-12-31"] {
            assert!(valid_date(date), "{date}");
        }
        for date in [
            "0000-01-01",
            "1900-02-29",
            "2026-00-01",
            "2026-04-31",
            "2026-01-00",
            "2026-1-01",
        ] {
            assert!(!valid_date(date), "{date}");
        }
    }

    fn message(sequence: i64, role: &str, content: &str) -> MessageRow {
        MessageRow {
            sequence,
            timestamp: None,
            role: role.into(),
            content: content.into(),
            content_truncated: false,
        }
    }

    fn no_pagination() -> Pagination {
        Pagination {
            page: 1,
            previous_href: None,
            next_href: None,
        }
    }
}
