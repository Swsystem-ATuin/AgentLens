use sqlx::{FromRow, SqlitePool};

pub(super) const SESSION_PAGE_SIZE: usize = 50;
pub(super) const TRANSCRIPT_SECTION_SIZE: usize = 50;
pub(super) const MESSAGE_PREVIEW_CHARS: i64 = 2_048;
pub(super) const MESSAGE_CHUNK_CHARS: i64 = 16_384;

#[derive(FromRow)]
pub(super) struct SessionRow {
    pub(super) id: i64,
    pub(super) stable_id: String,
    pub(super) agent_kind: String,
    pub(super) external_id: String,
    pub(super) title: String,
    pub(super) started_at: Option<String>,
    pub(super) ended_at: Option<String>,
    pub(super) model: Option<String>,
    pub(super) status: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) hostname: String,
    pub(super) os_user: String,
    pub(super) message_count: i64,
}

#[derive(FromRow)]
pub(super) struct MessageRow {
    pub(super) sequence: i64,
    pub(super) timestamp: Option<String>,
    pub(super) role: String,
    pub(super) content: String,
    pub(super) content_truncated: bool,
}

#[derive(FromRow)]
pub(super) struct MessageChunk {
    pub(super) sequence: i64,
    pub(super) timestamp: Option<String>,
    pub(super) role: String,
    pub(super) content: String,
    pub(super) total_chars: i64,
}

pub(super) struct SessionFilters<'a> {
    pub(super) host: Option<&'a str>,
    pub(super) user: Option<&'a str>,
    pub(super) model: Option<&'a str>,
    pub(super) status: Option<&'a str>,
}

pub(super) struct FilterValues {
    pub(super) hosts: Vec<String>,
    pub(super) users: Vec<String>,
    pub(super) models: Vec<String>,
    pub(super) statuses: Vec<String>,
}

pub(super) async fn sessions(
    pool: &SqlitePool,
    filters: &SessionFilters<'_>,
    offset: i64,
) -> Result<Vec<SessionRow>, sqlx::Error> {
    let model_exclusion = crate::sources::model_exclusion();
    sqlx::query_as::<_, SessionRow>(r#"
        SELECT s.id,s.stable_id,a.kind agent_kind,s.external_id,s.title,s.started_at,s.ended_at,s.model,s.status,s.cwd,h.name hostname,u.os_user,
               (SELECT count(*) FROM messages m WHERE m.session_id=s.id) message_count
        FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id
        WHERE (a.kind<>? OR s.model IS NULL OR s.model NOT LIKE ?) AND (? IS NULL OR h.name=?) AND (? IS NULL OR u.os_user=?) AND (? IS NULL OR s.model=?) AND (? IS NULL OR s.status=?)
        ORDER BY s.started_at DESC, s.id DESC
        LIMIT ? OFFSET ?
    "#).bind(model_exclusion.source_kind).bind(model_exclusion.pattern).bind(filters.host).bind(filters.host).bind(filters.user).bind(filters.user).bind(filters.model).bind(filters.model).bind(filters.status).bind(filters.status).bind((SESSION_PAGE_SIZE + 1) as i64).bind(offset).fetch_all(pool).await
}

pub(super) async fn filter_values(
    pool: &SqlitePool,
    include_origins: bool,
) -> Result<FilterValues, sqlx::Error> {
    let (hosts, users) = if include_origins {
        (
            sqlx::query_scalar("SELECT name FROM hosts ORDER BY name")
                .fetch_all(pool)
                .await?,
            sqlx::query_scalar("SELECT DISTINCT os_user FROM users ORDER BY os_user")
                .fetch_all(pool)
                .await?,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let model_exclusion = crate::sources::model_exclusion();
    let models = sqlx::query_scalar(
        "SELECT DISTINCT s.model FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE s.model IS NOT NULL AND s.model <> '' AND (a.kind<>? OR s.model NOT LIKE ?) ORDER BY s.model",
    )
    .bind(model_exclusion.source_kind)
    .bind(model_exclusion.pattern)
    .fetch_all(pool)
    .await?;
    let statuses = sqlx::query_scalar(
        "SELECT DISTINCT status FROM sessions WHERE status IS NOT NULL AND status <> '' ORDER BY status",
    )
    .fetch_all(pool)
    .await?;
    Ok(FilterValues {
        hosts,
        users,
        models,
        statuses,
    })
}

pub(super) async fn show_origins(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
    let layout: Option<String> = sqlx::query_scalar(
        "SELECT source_layout FROM ingest_runs WHERE status='complete' ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    Ok(layout.as_deref() != Some("direct-codex-home"))
}

pub(super) async fn session(
    pool: &SqlitePool,
    stable_id: &str,
) -> Result<Option<SessionRow>, sqlx::Error> {
    let model_exclusion = crate::sources::model_exclusion();
    sqlx::query_as::<_, SessionRow>(r#"
        SELECT s.id,s.stable_id,a.kind agent_kind,s.external_id,s.title,s.started_at,s.ended_at,s.model,s.status,s.cwd,h.name hostname,u.os_user,
               (SELECT count(*) FROM messages m WHERE m.session_id=s.id) message_count
        FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id WHERE s.stable_id=? AND (a.kind<>? OR s.model IS NULL OR s.model NOT LIKE ?)
    "#).bind(stable_id).bind(model_exclusion.source_kind).bind(model_exclusion.pattern).fetch_optional(pool).await
}

pub(super) async fn transcript_section(
    pool: &SqlitePool,
    session_id: i64,
    offset: i64,
) -> Result<Vec<MessageRow>, sqlx::Error> {
    sqlx::query_as::<_, MessageRow>(
        "SELECT sequence,timestamp,role,substr(content,1,?) content,length(content)>? content_truncated \
         FROM messages WHERE session_id=? ORDER BY sequence LIMIT ? OFFSET ?",
    )
    .bind(MESSAGE_PREVIEW_CHARS)
    .bind(MESSAGE_PREVIEW_CHARS)
    .bind(session_id)
    .bind((TRANSCRIPT_SECTION_SIZE + 1) as i64)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub(super) async fn message_chunk(
    pool: &SqlitePool,
    session_id: i64,
    sequence: i64,
    offset: i64,
) -> Result<Option<MessageChunk>, sqlx::Error> {
    sqlx::query_as::<_, MessageChunk>(
        "SELECT sequence,timestamp,role,substr(content,? + 1,?) content,length(content) total_chars \
         FROM messages WHERE session_id=? AND sequence=?",
    )
    .bind(offset)
    .bind(MESSAGE_CHUNK_CHARS)
    .bind(session_id)
    .bind(sequence)
    .fetch_optional(pool)
    .await
}

pub(super) async fn preceding_message_count(
    pool: &SqlitePool,
    session_id: i64,
    sequence: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM messages WHERE session_id=? AND sequence<?")
        .bind(session_id)
        .bind(sequence)
        .fetch_one(pool)
        .await
}
