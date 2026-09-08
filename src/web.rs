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
}

struct TranscriptSegment {
    sequence: i64,
    timestamp: Option<String>,
    role: String,
    paragraphs: Vec<String>,
    title: String,
    initially_open: bool,
}

fn group_transcript(messages: Vec<MessageRow>) -> Vec<TranscriptSegment> {
    let mut segments: Vec<TranscriptSegment> = Vec::new();
    let mut conversation_started = false;
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
            continue;
        }
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
}

#[derive(Template)]
#[template(path = "session.html")]
struct SessionTemplate {
    session: SessionRow,
    segments: Vec<TranscriptSegment>,
}

pub async fn serve(pool: SqlitePool, bind: &str) -> Result<()> {
    let app = Router::new()
        .route(
            "/",
            get(|| async { axum::response::Redirect::temporary("/sessions") }),
        )
        .route("/sessions", get(sessions))
        .route("/sessions/{id}", get(session))
        .layer(TraceLayer::new_for_http())
        .with_state(AppState { pool });
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
    let options = load_filter_options(&state.pool, &filters).await?;
    let rows = sqlx::query_as::<_, SessionRow>(r#"
        SELECT s.id,s.external_id,s.started_at,s.ended_at,s.model,s.status,s.cwd,h.name hostname,u.os_user,
               (SELECT count(*) FROM messages m WHERE m.session_id=s.id) message_count
        FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id
        WHERE (? IS NULL OR h.name=?) AND (? IS NULL OR u.os_user=?) AND (? IS NULL OR s.model=?) AND (? IS NULL OR s.status=?)
        ORDER BY s.started_at DESC
    "#).bind(&filters.host).bind(&filters.host).bind(&filters.user).bind(&filters.user).bind(&filters.model).bind(&filters.model).bind(&filters.status).bind(&filters.status).fetch_all(&state.pool).await?;
    Ok(Html(
        SessionsTemplate {
            sessions: rows,
            options,
        }
        .render()?,
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
}

async fn load_filter_options(pool: &SqlitePool, filters: &Filters) -> Result<FilterOptions> {
    let hosts = sqlx::query_scalar("SELECT name FROM hosts ORDER BY name")
        .fetch_all(pool)
        .await?;
    let users = sqlx::query_scalar("SELECT DISTINCT os_user FROM users ORDER BY os_user")
        .fetch_all(pool)
        .await?;
    let models = sqlx::query_scalar(
        "SELECT DISTINCT model FROM sessions WHERE model IS NOT NULL AND model <> '' ORDER BY model",
    )
    .fetch_all(pool)
    .await?;
    let statuses = sqlx::query_scalar(
        "SELECT DISTINCT status FROM sessions WHERE status IS NOT NULL AND status <> '' ORDER BY status",
    )
    .fetch_all(pool)
    .await?;

    Ok(FilterOptions {
        hosts: select_options(hosts, filters.host.as_deref()),
        users: select_options(users, filters.user.as_deref()),
        models: select_options(models, filters.model.as_deref()),
        statuses: select_options(statuses, filters.status.as_deref()),
    })
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
) -> Result<Html<String>, WebError> {
    let row = sqlx::query_as::<_, SessionRow>(r#"
        SELECT s.id,s.external_id,s.started_at,s.ended_at,s.model,s.status,s.cwd,h.name hostname,u.os_user,
               (SELECT count(*) FROM messages m WHERE m.session_id=s.id) message_count
        FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id WHERE s.id=?
    "#).bind(id).fetch_optional(&state.pool).await?.ok_or_else(|| WebError::not_found("session not found"))?;
    let messages = sqlx::query_as::<_, MessageRow>(
        "SELECT sequence,timestamp,role,content FROM messages WHERE session_id=? ORDER BY sequence",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Html(
        SessionTemplate {
            session: row,
            segments: group_transcript(messages),
        }
        .render()?,
    ))
}

struct WebError {
    status: StatusCode,
    source: anyhow::Error,
}
impl WebError {
    fn not_found(message: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            source: anyhow::anyhow!(message.to_owned()),
        }
    }
}
impl<E: Into<anyhow::Error>> From<E> for WebError {
    fn from(e: E) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            source: e.into(),
        }
    }
}
impl IntoResponse for WebError {
    fn into_response(self) -> axum::response::Response {
        tracing::error!(error=%self.source, "request failed");
        (self.status, self.source.to_string()).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ("developer", "<collaboration_mode>", "Collaboration mode"),
            ("developer", "<model_switch>", "Model switch"),
        ];
        for (role, prefix, title) in markers {
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
            },
            MessageRow {
                sequence: 11,
                timestamp: Some("2026-01-01T00:00:01Z".into()),
                role: "assistant".into(),
                content: "first reply".into(),
            },
            MessageRow {
                sequence: 12,
                timestamp: Some("2026-01-01T00:00:02Z".into()),
                role: "assistant".into(),
                content: "second reply".into(),
            },
            MessageRow {
                sequence: 13,
                timestamp: None,
                role: "user".into(),
                content: "follow-up".into(),
            },
        ];

        let segments = group_transcript(messages);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[1].role, "assistant");
        assert_eq!(segments[1].paragraphs, ["first reply", "second reply"]);
        assert_eq!(segments[2].paragraphs, ["follow-up"]);
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
        };

        filters.normalize();

        assert!(filters.host.is_none());
        assert!(filters.user.is_none());
        assert!(filters.model.is_none());
        assert!(filters.status.is_none());
    }

    fn message(sequence: i64, role: &str, content: &str) -> MessageRow {
        MessageRow {
            sequence,
            timestamp: None,
            role: role.into(),
            content: content.into(),
        }
    }
}
