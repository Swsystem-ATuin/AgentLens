use crate::projects::ProjectConfig;
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};

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
    pub(super) last_activity: Option<String>,
    pub(super) model: Option<String>,
    pub(super) status: Option<String>,
    pub(super) cwd: Option<String>,
    pub(super) project: Option<String>,
    pub(super) archive_state: String,
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
    pub(super) agent: Option<&'a str>,
    pub(super) project: Option<&'a str>,
    pub(super) archive: Option<&'a str>,
    pub(super) from: Option<&'a str>,
    pub(super) to: Option<&'a str>,
}

pub(super) struct FilterValues {
    pub(super) hosts: Vec<String>,
    pub(super) users: Vec<String>,
    pub(super) models: Vec<String>,
    pub(super) statuses: Vec<String>,
    pub(super) agents: Vec<String>,
    pub(super) projects: Vec<String>,
    pub(super) archives: Vec<String>,
}

pub(super) async fn sessions(
    pool: &SqlitePool,
    filters: &SessionFilters<'_>,
    projects: &ProjectConfig,
    offset: i64,
) -> Result<Vec<SessionRow>, sqlx::Error> {
    let model_exclusion = crate::sources::model_exclusion();
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT s.id,s.stable_id,a.kind agent_kind,s.external_id,s.title,s.started_at,COALESCE(s.ended_at,s.started_at) last_activity,s.model,s.status,s.cwd,",
    );
    push_project_case(&mut query, projects);
    query.push(
        " project,CASE WHEN sf.kind='archived_sessions' THEN 'archived' WHEN sf.kind IS NULL THEN 'unknown' ELSE 'active' END archive_state,h.name hostname,u.os_user,(SELECT count(*) FROM messages m WHERE m.session_id=s.id) message_count FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id LEFT JOIN source_files sf ON sf.id=s.source_file_id WHERE (a.kind<>",
    );
    query
        .push_bind(model_exclusion.source_kind)
        .push(" OR s.model IS NULL OR s.model NOT LIKE ")
        .push_bind(model_exclusion.pattern)
        .push(")");
    push_optional_equality(&mut query, "h.name", filters.host);
    push_optional_equality(&mut query, "u.os_user", filters.user);
    push_optional_equality(&mut query, "s.model", filters.model);
    push_optional_equality(&mut query, "s.status", filters.status);
    push_optional_equality(&mut query, "a.kind", filters.agent);
    query
        .push(" AND (")
        .push_bind(filters.project)
        .push(" IS NULL OR (");
    push_project_case(&mut query, projects);
    query.push(")=").push_bind(filters.project).push(")");
    query
        .push(" AND (")
        .push_bind(filters.archive)
        .push(" IS NULL OR CASE WHEN sf.kind='archived_sessions' THEN 'archived' WHEN sf.kind IS NULL THEN 'unknown' ELSE 'active' END=")
        .push_bind(filters.archive)
        .push(")");
    push_date_boundary(&mut query, filters.from, projects.timezone(), ">=");
    push_date_boundary(&mut query, filters.to, projects.timezone(), "<=");
    query
        .push(" ORDER BY julianday(COALESCE(s.ended_at,s.started_at)) DESC,s.stable_id DESC LIMIT ")
        .push_bind((SESSION_PAGE_SIZE + 1) as i64)
        .push(" OFFSET ")
        .push_bind(offset);
    query.build_query_as::<SessionRow>().fetch_all(pool).await
}

pub(super) async fn filter_values(
    pool: &SqlitePool,
    include_origins: bool,
    projects: &ProjectConfig,
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
    let agents = sqlx::query_scalar(
        "SELECT DISTINCT a.kind FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE (a.kind<>? OR s.model IS NULL OR s.model NOT LIKE ?) ORDER BY a.kind",
    )
    .bind(model_exclusion.source_kind)
    .bind(model_exclusion.pattern)
    .fetch_all(pool)
    .await?;
    let archives = sqlx::query_scalar(
        "SELECT DISTINCT CASE WHEN sf.kind='archived_sessions' THEN 'archived' WHEN sf.kind IS NULL THEN 'unknown' ELSE 'active' END FROM sessions s LEFT JOIN source_files sf ON sf.id=s.source_file_id JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE (a.kind<>? OR s.model IS NULL OR s.model NOT LIKE ?) ORDER BY 1",
    )
    .bind(model_exclusion.source_kind)
    .bind(model_exclusion.pattern)
    .fetch_all(pool)
    .await?;
    let mut project_query = QueryBuilder::<Sqlite>::new("SELECT DISTINCT project FROM (SELECT ");
    push_project_case(&mut project_query, projects);
    project_query
        .push(" project FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id WHERE (a.kind<>")
        .push_bind(model_exclusion.source_kind)
        .push(" OR s.model IS NULL OR s.model NOT LIKE ")
        .push_bind(model_exclusion.pattern)
        .push(") ) WHERE project IS NOT NULL ORDER BY project");
    let project_values = project_query
        .build_query_scalar::<String>()
        .fetch_all(pool)
        .await?;
    Ok(FilterValues {
        hosts,
        users,
        models,
        statuses,
        agents,
        projects: project_values,
        archives,
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
    projects: &ProjectConfig,
) -> Result<Option<SessionRow>, sqlx::Error> {
    let model_exclusion = crate::sources::model_exclusion();
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT s.id,s.stable_id,a.kind agent_kind,s.external_id,s.title,s.started_at,COALESCE(s.ended_at,s.started_at) last_activity,s.model,s.status,s.cwd,",
    );
    push_project_case(&mut query, projects);
    query
        .push(" project,CASE WHEN sf.kind='archived_sessions' THEN 'archived' WHEN sf.kind IS NULL THEN 'unknown' ELSE 'active' END archive_state,h.name hostname,u.os_user,(SELECT count(*) FROM messages m WHERE m.session_id=s.id) message_count FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id LEFT JOIN source_files sf ON sf.id=s.source_file_id WHERE s.stable_id=")
        .push_bind(stable_id)
        .push(" AND (a.kind<>")
        .push_bind(model_exclusion.source_kind)
        .push(" OR s.model IS NULL OR s.model NOT LIKE ")
        .push_bind(model_exclusion.pattern)
        .push(")");
    query
        .build_query_as::<SessionRow>()
        .fetch_optional(pool)
        .await
}

fn push_optional_equality<'a>(
    query: &mut QueryBuilder<'a, Sqlite>,
    column: &str,
    value: Option<&'a str>,
) {
    query
        .push(" AND (")
        .push_bind(value)
        .push(" IS NULL OR ")
        .push(column)
        .push("=")
        .push_bind(value)
        .push(")");
}

fn push_date_boundary<'a>(
    query: &mut QueryBuilder<'a, Sqlite>,
    value: Option<&'a str>,
    timezone: &'a str,
    comparison: &str,
) {
    query
        .push(" AND (")
        .push_bind(value)
        .push(" IS NULL OR date(COALESCE(s.ended_at,s.started_at),")
        .push_bind(timezone)
        .push(") ")
        .push(comparison)
        .push(" ")
        .push_bind(value)
        .push(")");
}

fn push_project_case<'a>(query: &mut QueryBuilder<'a, Sqlite>, projects: &'a ProjectConfig) {
    if projects.rules().is_empty() {
        query.push("NULL");
        return;
    }
    query.push("CASE");
    for rule in projects.rules() {
        let path_expression = if rule.windows {
            "rtrim(lower(replace(s.cwd,'\\','/')),'/')"
        } else {
            "rtrim(s.cwd,'/')"
        };
        let prefix = format!("{}/", rule.path);
        query
            .push(" WHEN h.name=")
            .push_bind(&rule.host)
            .push(" AND (")
            .push(path_expression)
            .push("=")
            .push_bind(&rule.path)
            .push(" OR substr(")
            .push(path_expression)
            .push(",1,length(")
            .push_bind(prefix.clone())
            .push("))=")
            .push_bind(prefix)
            .push(") THEN ")
            .push_bind(&rule.label);
    }
    query.push(" END");
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
