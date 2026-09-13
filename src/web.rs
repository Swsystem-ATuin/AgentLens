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
use sqlx::SqlitePool;
use tower_http::trace::TraceLayer;

mod markdown;
mod presentation;
mod queries;
#[cfg(test)]
mod regression;

use presentation::{
    FilterOptions, Pagination, TimelineItem, filtered_transcript_href, message_href,
    present_recent_usage, present_timeline, present_turns, present_usage_groups,
    present_usage_totals, select_options, sessions_href, usage_href,
};
use queries::{
    MESSAGE_CHUNK_CHARS, MessageChunk, SESSION_PAGE_SIZE, SessionFilters, SessionRow,
    TRANSCRIPT_SECTION_SIZE, TURN_PAGE_SIZE, ToolChunk, USAGE_PAGE_SIZE, UsageFilters,
};
#[cfg(test)]
use queries::{MessageRow, TimelineRow};

const MAX_SESSION_PAGE: u32 = 100_000;

#[derive(Clone)]
struct AppState {
    pool: SqlitePool,
    projects: crate::projects::ProjectConfig,
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
    from: Option<String>,
    to: Option<String>,
    page: Option<u32>,
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
    empty_message: &'static str,
}

#[derive(Deserialize, Default)]
struct UsageQuery {
    period: Option<String>,
    group: Option<String>,
    from: Option<String>,
    to: Option<String>,
    page: Option<u32>,
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
#[template(path = "session.html")]
struct SessionTemplate {
    session: SessionRow,
    segments: Vec<TimelineItem>,
    show_origin: bool,
    event_options: Vec<presentation::FilterOption>,
    pagination: Pagination,
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

fn router_with_projects(
    pool: SqlitePool,
    credentials: crate::auth::Credentials,
    projects: crate::projects::ProjectConfig,
) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { axum::response::Redirect::temporary("/sessions") }),
        )
        .route("/sessions", get(sessions))
        .route("/usage", get(usage))
        .route("/sessions/{id}", get(session))
        .route("/sessions/{id}/turns", get(turn_history))
        .route("/sessions/{id}/messages/{sequence}", get(message))
        .route("/sessions/{id}/tools/{sequence}", get(tool_call))
        .route("/sessions/{id}/results/{sequence}", get(tool_result))
        // Query strings can contain user-supplied private values. Keep request
        // spans useful without recording the URI in server diagnostics.
        .layer(TraceLayer::new_for_http().make_span_with(
            |request: &axum::http::Request<axum::body::Body>| {
                tracing::info_span!("http_request", method = %request.method())
            },
        ))
        .with_state(AppState { pool, projects })
        .layer(axum::middleware::from_fn_with_state(
            credentials,
            crate::auth::require_auth,
        ))
}

pub async fn serve(
    pool: SqlitePool,
    bind: &str,
    credentials: crate::auth::Credentials,
    projects: crate::projects::ProjectConfig,
) -> Result<()> {
    let app = router_with_projects(pool, credentials, projects);
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("bind {bind}"))?;
    tracing::info!(%bind, "Agent Lens listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn usage(
    State(state): State<AppState>,
    Query(mut query): Query<UsageQuery>,
) -> Result<Html<String>, WebError> {
    query.normalize();
    query.validate()?;
    let page = bounded_page(query.page, "validate usage page")?;
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
        queries::usage_totals(&state.pool, &filters, &state.projects),
        queries::usage_groups(
            &state.pool,
            &filters,
            &state.projects,
            i64::from(page - 1) * USAGE_PAGE_SIZE as i64,
        ),
        queries::recent_usage(&state.pool, &filters, &state.projects),
    )
    .map_err(|error| WebError::database("load usage dashboard", error))?;
    let has_next = rows.len() > USAGE_PAGE_SIZE;
    rows.truncate(USAGE_PAGE_SIZE);
    let pagination = Pagination {
        page,
        previous_href: (page > 1)
            .then(|| usage_href(period, group, filters.from, filters.to, page - 1)),
        next_href: (has_next && page < MAX_SESSION_PAGE)
            .then(|| usage_href(period, group, filters.from, filters.to, page + 1)),
    };
    let mut groups = vec!["session", "model", "project", "agent"];
    if show_origins {
        groups.extend(["host", "user"]);
    }
    Ok(Html(
        UsageTemplate {
            totals: present_usage_totals(totals),
            rows: present_usage_groups(rows),
            recent: present_recent_usage(recent),
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
            timezone: state.projects.timezone().to_owned(),
            pagination,
        }
        .render()
        .map_err(|error| WebError::template("render usage dashboard", error))?,
    ))
}

impl UsageQuery {
    fn normalize(&mut self) {
        for value in [
            &mut self.period,
            &mut self.group,
            &mut self.from,
            &mut self.to,
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
    Query(mut filters): Query<Filters>,
) -> Result<Html<String>, WebError> {
    filters.normalize();
    filters.validate()?;
    let page = filters.page_number()?;
    let show_origins = queries::show_origins(&state.pool)
        .await
        .map_err(|error| WebError::database("load source layout", error))?;
    if !show_origins {
        filters.host = None;
        filters.user = None;
    }
    let values = queries::filter_values(&state.pool, show_origins, &state.projects)
        .await
        .map_err(|error| WebError::database("load filter options", error))?;
    let query_filters = filters.as_query();
    let options = FilterOptions::new(values, &query_filters);
    let mut rows = queries::sessions(
        &state.pool,
        &query_filters,
        &state.projects,
        i64::from(page - 1) * SESSION_PAGE_SIZE as i64,
    )
    .await
    .map_err(|error| WebError::database("load sessions", error))?;
    let has_next = rows.len() > SESSION_PAGE_SIZE;
    rows.truncate(SESSION_PAGE_SIZE);
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
            timezone: state.projects.timezone().to_owned(),
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
            &mut self.from,
            &mut self.to,
        ] {
            if value.as_deref().is_some_and(str::is_empty) {
                *value = None;
            }
        }
    }

    fn validate(&self) -> Result<(), WebError> {
        if self
            .q
            .as_deref()
            .is_some_and(|query| query.chars().count() > crate::search::MAX_QUERY_CHARS)
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
            from: self.from.as_deref(),
            to: self.to.as_deref(),
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
    Path(stable_id): Path<String>,
    Query(query): Query<TranscriptQuery>,
) -> Result<Html<String>, WebError> {
    let section = bounded_page(query.section, "validate transcript section")?;
    let event_filter = query.event.as_deref().unwrap_or("all");
    if !matches!(
        event_filter,
        "all" | "messages" | "tools" | "questions" | "turns"
    ) {
        return Err(WebError::invalid_filters());
    }
    let show_origin = queries::show_origins(&state.pool)
        .await
        .map_err(|error| WebError::database("load source layout", error))?;
    let row = load_session(&state.pool, &stable_id, &state.projects).await?;
    let mut events = queries::timeline_section(
        &state.pool,
        row.id,
        event_filter,
        i64::from(section - 1) * TRANSCRIPT_SECTION_SIZE as i64,
    )
    .await
    .map_err(|error| WebError::database("load transcript section", error))?;
    let has_next = events.len() > TRANSCRIPT_SECTION_SIZE;
    events.truncate(TRANSCRIPT_SECTION_SIZE);
    let pagination = Pagination {
        page: section,
        previous_href: (section > 1)
            .then(|| filtered_transcript_href(&stable_id, section - 1, event_filter, None)),
        next_href: (has_next && section < MAX_SESSION_PAGE)
            .then(|| filtered_transcript_href(&stable_id, section + 1, event_filter, None)),
    };
    let segments = present_timeline(
        events,
        &row.agent_kind,
        &stable_id,
        section > 1 && matches!(event_filter, "all" | "messages"),
    );
    let event_options = select_options(
        ["all", "messages", "tools", "questions", "turns"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        Some(event_filter),
    );
    Ok(Html(
        SessionTemplate {
            session: row,
            segments,
            show_origin,
            event_options,
            pagination,
        }
        .render()
        .map_err(|error| WebError::template("render transcript", error))?,
    ))
}

async fn turn_history(
    State(state): State<AppState>,
    Path(stable_id): Path<String>,
    Query(query): Query<TurnHistoryQuery>,
) -> Result<Html<String>, WebError> {
    let page = bounded_page(query.page, "validate turn page")?;
    let session = load_session(&state.pool, &stable_id, &state.projects).await?;
    let mut rows = queries::session_turns(
        &state.pool,
        session.id,
        i64::from(page - 1) * TURN_PAGE_SIZE as i64,
    )
    .await
    .map_err(|error| WebError::database("load turn history", error))?;
    let has_next = rows.len() > TURN_PAGE_SIZE;
    rows.truncate(TURN_PAGE_SIZE);
    Ok(Html(
        TurnsTemplate {
            turns: present_turns(rows, &stable_id),
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

async fn load_session(
    pool: &SqlitePool,
    stable_id: &str,
    projects: &crate::projects::ProjectConfig,
) -> Result<SessionRow, WebError> {
    queries::session(pool, stable_id, projects)
        .await
        .map_err(|error| WebError::database("load session", error))?
        .ok_or_else(WebError::not_found)
}

async fn message(
    State(state): State<AppState>,
    Path((stable_id, sequence)): Path<(String, i64)>,
    Query(query): Query<MessageQuery>,
) -> Result<Html<String>, WebError> {
    let part = bounded_page(query.part, "validate message part")?;
    let session = load_session(&state.pool, &stable_id, &state.projects).await?;
    let offset = i64::from(part - 1) * MESSAGE_CHUNK_CHARS;
    let message = queries::message_chunk(&state.pool, session.id, sequence, offset)
        .await
        .map_err(|error| WebError::database("load message part", error))?
        .ok_or_else(WebError::not_found)?;
    if part > 1 && offset >= message.total_chars {
        return Err(WebError::invalid_pagination("validate message part"));
    }
    let preceding = queries::preceding_timeline_event_count(&state.pool, session.id, sequence, 0)
        .await
        .map_err(|error| WebError::database("locate message section", error))?;
    let section = preceding / TRANSCRIPT_SECTION_SIZE as i64 + 1;
    let pagination = Pagination {
        page: part,
        previous_href: (part > 1).then(|| message_href(&stable_id, sequence, part - 1)),
        next_href: (offset + MESSAGE_CHUNK_CHARS < message.total_chars && part < MAX_SESSION_PAGE)
            .then(|| message_href(&stable_id, sequence, part + 1)),
    };
    Ok(Html(
        MessageTemplate {
            content_html: markdown::render(&message.content),
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
    Path((stable_id, sequence)): Path<(String, i64)>,
    Query(query): Query<MessageQuery>,
) -> Result<Html<String>, WebError> {
    tool_detail(state, stable_id, sequence, query.part, false).await
}

async fn tool_result(
    State(state): State<AppState>,
    Path((stable_id, sequence)): Path<(String, i64)>,
    Query(query): Query<MessageQuery>,
) -> Result<Html<String>, WebError> {
    tool_detail(state, stable_id, sequence, query.part, true).await
}

async fn tool_detail(
    state: AppState,
    stable_id: String,
    sequence: i64,
    requested_part: Option<u32>,
    result: bool,
) -> Result<Html<String>, WebError> {
    let part = bounded_page(requested_part, "validate tool detail part")?;
    let session = load_session(&state.pool, &stable_id, &state.projects).await?;
    let offset = i64::from(part - 1) * MESSAGE_CHUNK_CHARS;
    let item = if result {
        queries::tool_result_chunk(&state.pool, session.id, sequence, offset).await
    } else {
        queries::tool_call_chunk(&state.pool, session.id, sequence, offset).await
    }
    .map_err(|error| WebError::database("load tool detail part", error))?
    .ok_or_else(WebError::not_found)?;
    if part > 1 && offset >= item.total_chars {
        return Err(WebError::invalid_pagination("validate tool detail part"));
    }
    let event_order = if result { 2 } else { 1 };
    let preceding =
        queries::preceding_timeline_event_count(&state.pool, session.id, sequence, event_order)
            .await
            .map_err(|error| WebError::database("locate tool timeline section", error))?;
    let section = preceding / TRANSCRIPT_SECTION_SIZE as i64 + 1;
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
            "/usage",
            "/sessions/1",
            "/sessions/1/turns",
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
            ("/usage", StatusCode::OK),
            ("/sessions/1", StatusCode::NOT_FOUND),
            ("/sessions/1/turns", StatusCode::NOT_FOUND),
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
                message_count: 5,
                match_sequence: None,
                match_snippet: None,
            },
            segments: group_transcript(messages),
            event_options: vec![],
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
                event_options: vec![],
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
            from: Some("2026-01-01".into()),
            to: Some("2026-01-31".into()),
            page: Some(2),
        };

        assert_eq!(
            filters.href(3),
            "/sessions?q=needle%20text&host=host%20name&user=user%2Fname&model=model%20%26%2B&status=%E5%AE%8C%E4%BA%86&agent=codex&project=project%20one&archive=active&from=2026-01-01&to=2026-01-31&page=3"
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
