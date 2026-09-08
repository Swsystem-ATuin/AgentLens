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

#[derive(Deserialize, Default)]
struct Filters {
    host: Option<String>,
    user: Option<String>,
    model: Option<String>,
    status: Option<String>,
}

#[derive(Template)]
#[template(path = "sessions.html")]
struct SessionsTemplate {
    sessions: Vec<SessionRow>,
    filters: Filters,
}

#[derive(Template)]
#[template(path = "session.html")]
struct SessionTemplate {
    session: SessionRow,
    messages: Vec<MessageRow>,
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
    Query(filters): Query<Filters>,
) -> Result<Html<String>, WebError> {
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
            filters,
        }
        .render()?,
    ))
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
            messages,
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
    fn transcript_defaults_configuration_closed_and_conversation_open() {
        let messages = (0..3)
            .map(|sequence| MessageRow {
                sequence,
                timestamp: None,
                role: if sequence == 2 { "user" } else { "developer" }.into(),
                content: format!("redacted {sequence}"),
            })
            .collect();
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
                message_count: 3,
            },
            messages,
        }
        .render()
        .expect("template should render");

        assert!(html.contains("<strong>Initial agent configuration</strong>"));
        assert!(html.contains("<strong>Session environment</strong>"));
        assert_eq!(html.matches("<details class=\"message").count(), 3);
        assert_eq!(html.matches(" open>").count(), 1);
    }
}
