use anyhow::{Context, Result};
use askama::Template;
use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse},
    routing::get,
};
use serde::Deserialize;
use sqlx::{FromRow, SqlitePool};
use tower_http::trace::TraceLayer;

#[cfg(test)]
mod regression;

// Presentation policy only: retain these sessions in the rebuildable index.
const HIDDEN_MODEL_PATTERN: &str = "codex-auto-review%";
const SESSION_PAGE_SIZE: usize = 50;
const MAX_SESSION_PAGE: u32 = 100_000;
const TRANSCRIPT_SECTION_SIZE: usize = 50;
const MESSAGE_PREVIEW_CHARS: i64 = 2_048;
const MESSAGE_CHUNK_CHARS: i64 = 16_384;

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
}

#[derive(FromRow)]
struct SessionRow {
    id: i64,
    external_id: String,
    started_at: Option<String>,
    ended_at: Option<String>,
    model: Option<String>,
    status: Option<String>,
    cwd: Option<String>,
    hostname: String,
    os_user: String,
    message_count: i64,
}
#[derive(FromRow)]
struct MessageRow {
    sequence: i64,
    timestamp: Option<String>,
    role: String,
    content: String,
    content_truncated: bool,
}

struct TranscriptSegment {
    sequence: i64,
    timestamp: Option<String>,
    role: String,
    paragraphs: Vec<String>,
    truncated_sequences: Vec<i64>,
    title: String,
    initially_open: bool,
}

#[cfg(test)]
fn group_transcript(messages: Vec<MessageRow>) -> Vec<TranscriptSegment> {
    group_transcript_section(messages, false)
}

fn group_transcript_section(
    messages: Vec<MessageRow>,
    mut conversation_started: bool,
) -> Vec<TranscriptSegment> {
    let mut segments: Vec<TranscriptSegment> = Vec::new();
    for message in messages {
        let initialization_title =
            (message.role == "user" || message.role == "developer" || message.role == "system")
                .then(|| initialization_title(&message.content, !conversation_started))
                .flatten();
        let notice_title = match message.role.as_str() {
            "user" | "developer" | "system"
                if message.content.trim_start().starts_with("<turn_aborted>") =>
            {
                Some("Turn interrupted")
            }
            "user" if message.content.trim_start().starts_with("<user_action>") => {
                Some("User action")
            }
            _ => None,
        };
        if !conversation_started && message.role == "user" && initialization_title.is_none() {
            conversation_started = true;
        }
        if message.role == "assistant"
            && initialization_title.is_none()
            && let Some(previous) = segments.last_mut()
            && previous.role == "assistant"
            && previous.initially_open
        {
            previous.paragraphs.push(message.content);
            if message.content_truncated {
                previous.truncated_sequences.push(message.sequence);
            }
            continue;
        }
        let truncated_sequences = message
            .content_truncated
            .then_some(message.sequence)
            .into_iter()
            .collect();
        segments.push(TranscriptSegment {
            sequence: message.sequence,
            timestamp: message.timestamp,
            title: initialization_title
                .or(notice_title)
                .map(str::to_owned)
                .unwrap_or_else(|| message.role.clone()),
            initially_open: initialization_title.is_none(),
            role: message.role,
            paragraphs: vec![message.content],
            truncated_sequences,
        });
    }
    segments
}

fn initialization_title(content: &str, before_conversation: bool) -> Option<&'static str> {
    let trimmed = content.trim_start();
    if trimmed.starts_with("# AGENTS.md instructions for ") {
        Some("Repository instructions")
    } else if trimmed.starts_with("<permissions instructions>") {
        Some("Permissions")
    } else if trimmed.starts_with("<collaboration_mode>") {
        Some("Collaboration mode")
    } else if trimmed.starts_with("<model_switch>") {
        Some("Model switch")
    } else if trimmed.starts_with("<skills_instructions>") {
        Some("Available skills")
    } else if before_conversation && trimmed.starts_with("You are ") {
        Some("Initial agent configuration")
    } else if trimmed.starts_with("<multi_agent_mode>") {
        Some("Multi-agent mode")
    } else if trimmed.starts_with("<multi_agent_role>") {
        Some("Multi-agent role")
    } else if trimmed.starts_with("<environment_context>") {
        Some("Environment context")
    } else {
        None
    }
}

#[derive(Deserialize, Default)]
struct Filters {
    host: Option<String>,
    user: Option<String>,
    model: Option<String>,
    status: Option<String>,
    page: Option<u32>,
}

struct FilterOption {
    value: String,
    selected: bool,
}

struct FilterOptions {
    hosts: Vec<FilterOption>,
    users: Vec<FilterOption>,
    models: Vec<FilterOption>,
    statuses: Vec<FilterOption>,
}

#[derive(Template)]
#[template(path = "sessions.html")]
struct SessionsTemplate {
    sessions: Vec<SessionRow>,
    options: FilterOptions,
    show_origins: bool,
    pagination: Pagination,
}

struct Pagination {
    page: u32,
    previous_href: Option<String>,
    next_href: Option<String>,
}

#[derive(Template)]
#[template(path = "session.html")]
struct SessionTemplate {
    session: SessionRow,
    segments: Vec<TranscriptSegment>,
    show_origin: bool,
    pagination: Pagination,
}

#[derive(Deserialize, Default)]
struct TranscriptQuery {
    section: Option<u32>,
}

#[derive(Deserialize, Default)]
struct MessageQuery {
    part: Option<u32>,
}

#[derive(FromRow)]
struct MessageChunk {
    sequence: i64,
    timestamp: Option<String>,
    role: String,
    content: String,
    total_chars: i64,
}

#[derive(Template)]
#[template(path = "message.html")]
struct MessageTemplate {
    message: MessageChunk,
    transcript_href: String,
    pagination: Pagination,
}

fn router(pool: SqlitePool, credentials: crate::auth::Credentials) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { axum::response::Redirect::temporary("/sessions") }),
        )
        .route("/sessions", get(sessions))
        .route("/sessions/{id}", get(session))
        .route("/sessions/{id}/messages/{sequence}", get(message))
        // Query strings can contain user-supplied private values. Keep request
        // spans useful without recording the URI in server diagnostics.
        .layer(TraceLayer::new_for_http().make_span_with(
            |request: &axum::http::Request<axum::body::Body>| {
                tracing::info_span!("http_request", method = %request.method())
            },
        ))
        .with_state(AppState { pool })
        .layer(axum::middleware::from_fn_with_state(
            credentials,
            crate::auth::require_auth,
        ))
}

pub async fn serve(
    pool: SqlitePool,
    bind: &str,
    credentials: crate::auth::Credentials,
) -> Result<()> {
    let app = router(pool, credentials);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    tracing::info!(%bind, "Agent Lens listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn sessions(
    State(state): State<AppState>,
    Query(mut filters): Query<Filters>,
) -> Result<Html<String>, WebError> {
    filters.normalize();
    let page = filters.page_number()?;
    let show_origins = show_origins(&state.pool).await?;
    if !show_origins {
        filters.host = None;
        filters.user = None;
    }
    let options = load_filter_options(&state.pool, &filters, show_origins).await?;
    let mut rows = sqlx::query_as::<_, SessionRow>(r#"
        SELECT s.id,s.external_id,s.started_at,s.ended_at,s.model,s.status,s.cwd,h.name hostname,u.os_user,
               (SELECT count(*) FROM messages m WHERE m.session_id=s.id) message_count
        FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id
        WHERE (s.model IS NULL OR s.model NOT LIKE ?) AND (? IS NULL OR h.name=?) AND (? IS NULL OR u.os_user=?) AND (? IS NULL OR s.model=?) AND (? IS NULL OR s.status=?)
        ORDER BY s.started_at DESC, s.id DESC
        LIMIT ? OFFSET ?
    "#).bind(HIDDEN_MODEL_PATTERN).bind(&filters.host).bind(&filters.host).bind(&filters.user).bind(&filters.user).bind(&filters.model).bind(&filters.model).bind(&filters.status).bind(&filters.status).bind((SESSION_PAGE_SIZE + 1) as i64).bind(i64::from(page - 1) * SESSION_PAGE_SIZE as i64).fetch_all(&state.pool).await.map_err(|error| WebError::database("load sessions", error))?;
    let has_next = rows.len() > SESSION_PAGE_SIZE;
    rows.truncate(SESSION_PAGE_SIZE);
    let pagination = Pagination {
        page,
        previous_href: (page > 1).then(|| page_href(&filters, page - 1)),
        next_href: (has_next && page < MAX_SESSION_PAGE).then(|| page_href(&filters, page + 1)),
    };
    Ok(Html(
        SessionsTemplate {
            sessions: rows,
            options,
            show_origins,
            pagination,
        }
        .render()
        .map_err(|error| WebError::template("render sessions", error))?,
    ))
}

impl Filters {
    fn normalize(&mut self) {
        for value in [
            &mut self.host,
            &mut self.user,
            &mut self.model,
            &mut self.status,
        ] {
            if value.as_deref().is_some_and(str::is_empty) {
                *value = None;
            }
        }
    }

    fn page_number(&self) -> Result<u32, WebError> {
        let page = self.page.unwrap_or(1);
        if page == 0 || page > MAX_SESSION_PAGE {
            return Err(WebError::invalid_page());
        }
        Ok(page)
    }
}

fn page_href(filters: &Filters, page: u32) -> String {
    let mut pairs = Vec::new();
    for (name, value) in [
        ("host", filters.host.as_deref()),
        ("user", filters.user.as_deref()),
        ("model", filters.model.as_deref()),
        ("status", filters.status.as_deref()),
    ] {
        if let Some(value) = value {
            pairs.push(format!("{name}={}", percent_encode(value)));
        }
    }
    pairs.push(format!("page={page}"));
    format!("/sessions?{}", pairs.join("&"))
}

fn percent_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[(byte >> 4) as usize]));
            encoded.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

async fn load_filter_options(
    pool: &SqlitePool,
    filters: &Filters,
    show_origins: bool,
) -> Result<FilterOptions, WebError> {
    let (hosts, users) = if show_origins {
        (
            sqlx::query_scalar("SELECT name FROM hosts ORDER BY name")
                .fetch_all(pool)
                .await
                .map_err(|error| WebError::database("load host filters", error))?,
            sqlx::query_scalar("SELECT DISTINCT os_user FROM users ORDER BY os_user")
                .fetch_all(pool)
                .await
                .map_err(|error| WebError::database("load user filters", error))?,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let models = sqlx::query_scalar(
        "SELECT DISTINCT model FROM sessions WHERE model IS NOT NULL AND model <> '' AND model NOT LIKE ? ORDER BY model",
    )
    .bind(HIDDEN_MODEL_PATTERN)
    .fetch_all(pool)
    .await
    .map_err(|error| WebError::database("load model filters", error))?;
    let statuses = sqlx::query_scalar(
        "SELECT DISTINCT status FROM sessions WHERE status IS NOT NULL AND status <> '' ORDER BY status",
    )
    .fetch_all(pool)
    .await
    .map_err(|error| WebError::database("load status filters", error))?;

    Ok(FilterOptions {
        hosts: select_options(hosts, filters.host.as_deref()),
        users: select_options(users, filters.user.as_deref()),
        models: select_options(models, filters.model.as_deref()),
        statuses: select_options(statuses, filters.status.as_deref()),
    })
}

async fn show_origins(pool: &SqlitePool) -> Result<bool, WebError> {
    let layout: Option<String> = sqlx::query_scalar(
        "SELECT source_layout FROM ingest_runs WHERE status='complete' ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .map_err(|error| WebError::database("load source layout", error))?;
    Ok(layout.as_deref() != Some("direct-codex-home"))
}

fn select_options(values: Vec<String>, selected: Option<&str>) -> Vec<FilterOption> {
    values
        .into_iter()
        .map(|value| FilterOption {
            selected: selected == Some(value.as_str()),
            value,
        })
        .collect()
}

async fn session(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(query): Query<TranscriptQuery>,
) -> Result<Html<String>, WebError> {
    let section = bounded_page(query.section, "validate transcript section")?;
    let show_origin = show_origins(&state.pool).await?;
    let row = load_session(&state.pool, id).await?;
    let mut messages = sqlx::query_as::<_, MessageRow>(
        "SELECT sequence,timestamp,role,substr(content,1,?) content,length(content)>? content_truncated \
         FROM messages WHERE session_id=? ORDER BY sequence LIMIT ? OFFSET ?",
    )
    .bind(MESSAGE_PREVIEW_CHARS)
    .bind(MESSAGE_PREVIEW_CHARS)
    .bind(id)
    .bind((TRANSCRIPT_SECTION_SIZE + 1) as i64)
    .bind(i64::from(section - 1) * TRANSCRIPT_SECTION_SIZE as i64)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| WebError::database("load transcript section", error))?;
    let has_next = messages.len() > TRANSCRIPT_SECTION_SIZE;
    messages.truncate(TRANSCRIPT_SECTION_SIZE);
    let pagination = Pagination {
        page: section,
        previous_href: (section > 1).then(|| transcript_href(id, section - 1)),
        next_href: (has_next && section < MAX_SESSION_PAGE)
            .then(|| transcript_href(id, section + 1)),
    };
    Ok(Html(
        SessionTemplate {
            session: row,
            segments: group_transcript_section(messages, section > 1),
            show_origin,
            pagination,
        }
        .render()
        .map_err(|error| WebError::template("render transcript", error))?,
    ))
}

async fn load_session(pool: &SqlitePool, id: i64) -> Result<SessionRow, WebError> {
    sqlx::query_as::<_, SessionRow>(r#"
        SELECT s.id,s.external_id,s.started_at,s.ended_at,s.model,s.status,s.cwd,h.name hostname,u.os_user,
               (SELECT count(*) FROM messages m WHERE m.session_id=s.id) message_count
        FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id WHERE s.id=? AND (s.model IS NULL OR s.model NOT LIKE ?)
    "#).bind(id).bind(HIDDEN_MODEL_PATTERN).fetch_optional(pool).await.map_err(|error| WebError::database("load session", error))?.ok_or_else(WebError::not_found)
}

async fn message(
    State(state): State<AppState>,
    Path((id, sequence)): Path<(i64, i64)>,
    Query(query): Query<MessageQuery>,
) -> Result<Html<String>, WebError> {
    let part = bounded_page(query.part, "validate message part")?;
    load_session(&state.pool, id).await?;
    let offset = i64::from(part - 1) * MESSAGE_CHUNK_CHARS;
    let message = sqlx::query_as::<_, MessageChunk>(
        "SELECT sequence,timestamp,role,substr(content,? + 1,?) content,length(content) total_chars \
         FROM messages WHERE session_id=? AND sequence=?",
    )
    .bind(offset)
    .bind(MESSAGE_CHUNK_CHARS)
    .bind(id)
    .bind(sequence)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| WebError::database("load message part", error))?
    .ok_or_else(WebError::not_found)?;
    if part > 1 && offset >= message.total_chars {
        return Err(WebError::invalid_pagination("validate message part"));
    }
    let preceding: i64 =
        sqlx::query_scalar("SELECT count(*) FROM messages WHERE session_id=? AND sequence<?")
            .bind(id)
            .bind(sequence)
            .fetch_one(&state.pool)
            .await
            .map_err(|error| WebError::database("locate message section", error))?;
    let section = preceding / TRANSCRIPT_SECTION_SIZE as i64 + 1;
    let pagination = Pagination {
        page: part,
        previous_href: (part > 1).then(|| message_href(id, sequence, part - 1)),
        next_href: (offset + MESSAGE_CHUNK_CHARS < message.total_chars && part < MAX_SESSION_PAGE)
            .then(|| message_href(id, sequence, part + 1)),
    };
    Ok(Html(
        MessageTemplate {
            message,
            transcript_href: transcript_href(id, section as u32),
            pagination,
        }
        .render()
        .map_err(|error| WebError::template("render message part", error))?,
    ))
}

fn bounded_page(page: Option<u32>, operation: &'static str) -> Result<u32, WebError> {
    let page = page.unwrap_or(1);
    if page == 0 || page > MAX_SESSION_PAGE {
        return Err(WebError::invalid_pagination(operation));
    }
    Ok(page)
}

fn transcript_href(id: i64, section: u32) -> String {
    format!("/sessions/{id}?section={section}")
}

fn message_href(id: i64, sequence: i64, part: u32) -> String {
    format!("/sessions/{id}/messages/{sequence}?part={part}")
}

struct WebError {
    status: StatusCode,
    public_message: &'static str,
    operation: &'static str,
    error_kind: &'static str,
    error_code: Option<String>,
}
impl WebError {
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
        (self.status, self.public_message).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_route_requires_authentication() {
        use axum::{
            body::{Body, to_bytes},
            http::{Request, header},
        };
        use base64::{Engine, engine::general_purpose::STANDARD};
        use tower::ServiceExt;

        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("auth.json");
        std::fs::write(
            &config,
            r#"{"username":"test","password":"synthetic:password"}"#,
        )
        .unwrap();
        let pool = crate::db::open(&dir.path().join("index.sqlite"))
            .await
            .unwrap();
        let app = router(pool, crate::auth::Credentials::load(&config).unwrap());
        let valid = format!("Basic {}", STANDARD.encode("test:synthetic:password"));
        let wrong_password = format!("Basic {}", STANDARD.encode("test:wrong"));
        let wrong_user = format!("Basic {}", STANDARD.encode("wrong:synthetic:password"));
        for path in [
            "/",
            "/sessions",
            "/sessions/1",
            "/sessions/1/messages/0",
            "/sessions/invalid",
            "/missing",
        ] {
            for auth in [
                None,
                Some("Basic !!!"),
                Some("Bearer token"),
                Some(wrong_password.as_str()),
                Some(wrong_user.as_str()),
            ] {
                let mut request = Request::builder().uri(path);
                if let Some(auth) = auth {
                    request = request.header(header::AUTHORIZATION, auth);
                }
                let response = app
                    .clone()
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
                assert!(
                    response.headers()[header::WWW_AUTHENTICATE]
                        .to_str()
                        .unwrap()
                        .starts_with("Basic ")
                );
                assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
                assert_eq!(
                    to_bytes(response.into_body(), 1024).await.unwrap(),
                    "Authentication required"
                );
            }
        }
        for (path, status) in [
            ("/", StatusCode::TEMPORARY_REDIRECT),
            ("/sessions", StatusCode::OK),
            ("/sessions/1", StatusCode::NOT_FOUND),
            ("/missing", StatusCode::NOT_FOUND),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::AUTHORIZATION, &valid)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/sessions")
                    .header(header::AUTHORIZATION, &valid)
                    .header(header::AUTHORIZATION, &valid)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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
                "user",
                "<environment_context>redacted</environment_context>",
            ),
            message(4, "user", "actual request"),
        ];
        let html = SessionTemplate {
            session: SessionRow {
                id: 1,
                external_id: "redacted-session".into(),
                started_at: None,
                ended_at: None,
                model: None,
                status: None,
                cwd: None,
                hostname: "fixture-host".into(),
                os_user: "fixture-user".into(),
                message_count: 5,
            },
            segments: group_transcript(messages),
            show_origin: true,
            pagination: no_pagination(),
        }
        .render()
        .expect("template should render");

        assert!(html.contains("<strong>Available skills</strong>"));
        assert!(html.contains("<strong>Initial agent configuration</strong>"));
        assert!(html.contains("<strong>Multi-agent mode</strong>"));
        assert!(html.contains("<strong>Environment context</strong>"));
        assert_eq!(html.matches("<details class=\"message").count(), 5);
        assert_eq!(html.matches(" open>").count(), 1);
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
                    external_id: "synthetic-session".into(),
                    started_at: None,
                    ended_at: None,
                    model: None,
                    status: None,
                    cwd: None,
                    hostname: "synthetic-host".into(),
                    os_user: "synthetic-user".into(),
                    message_count: messages.len() as i64,
                },
                segments: group_transcript(messages),
                show_origin: true,
                pagination: no_pagination(),
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
        assert_eq!(segments[1].paragraphs, ["first reply", "second reply"]);
        assert_eq!(segments[2].paragraphs, ["follow-up"]);
    }

    #[test]
    fn later_sections_do_not_reclassify_conversation_as_initial_setup() {
        let segments = group_transcript_section(
            vec![message(50, "user", "You are seeing ordinary conversation")],
            true,
        );

        assert_eq!(segments[0].title, "user");
        assert!(segments[0].initially_open);
    }

    #[test]
    fn sessions_page_renders_known_filter_values_and_separate_identity_columns() {
        let html = SessionsTemplate {
            sessions: vec![SessionRow {
                id: 1,
                external_id: "redacted-session".into(),
                started_at: None,
                ended_at: None,
                model: Some("fixture-model".into()),
                status: Some("complete".into()),
                cwd: None,
                hostname: "fixture-host".into(),
                os_user: "fixture-user".into(),
                message_count: 2,
            }],
            options: FilterOptions {
                hosts: select_options(vec!["fixture-host".into()], Some("fixture-host")),
                users: select_options(vec!["fixture-user".into()], None),
                models: select_options(vec!["fixture-model".into()], None),
                statuses: select_options(vec!["complete".into()], None),
            },
            show_origins: true,
            pagination: Pagination {
                page: 1,
                previous_href: None,
                next_href: None,
            },
        }
        .render()
        .expect("template should render");

        assert!(html.contains("<th>Host</th><th>User</th>"));
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
            page: None,
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
            host: Some("host name".into()),
            user: Some("user/name".into()),
            model: Some("model &+".into()),
            status: Some("完了".into()),
            page: Some(2),
        };

        assert_eq!(
            page_href(&filters, 3),
            "/sessions?host=host%20name&user=user%2Fname&model=model%20%26%2B&status=%E5%AE%8C%E4%BA%86&page=3"
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
            }
            .page_number()
            .unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            assert_eq!(error.public_message, "invalid page");
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
