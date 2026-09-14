use crate::projects::ProjectConfig;
use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};

pub(super) const SESSION_PAGE_SIZE: usize = 50;
pub(super) const TRANSCRIPT_SECTION_SIZE: usize = 50;
pub(super) const USAGE_PAGE_SIZE: usize = 100;
pub(super) const RECENT_USAGE_LIMIT: i64 = 20;
pub(super) const TURN_PAGE_SIZE: usize = 50;
pub(super) const DIAGNOSTIC_EVIDENCE_LIMIT: i64 = 20;
pub(super) const PRESSURE_SNAPSHOT_LIMIT: i64 = 50;
pub(super) const MESSAGE_PREVIEW_CHARS: i64 = 2_048;
pub(super) const MESSAGE_CHUNK_CHARS: i64 = 16_384;
const SEARCH_CANDIDATES_PER_SESSION: i64 = 8;
const SEARCH_SNIPPET_CHARS: i64 = 512;

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
    pub(super) match_sequence: Option<i64>,
    pub(super) match_snippet: Option<String>,
}

#[derive(FromRow)]
pub(super) struct LiveStatusRow {
    pub(super) revision: i64,
    pub(super) last_successful_import: Option<String>,
    pub(super) source_observed_at: Option<String>,
    pub(super) source_age_seconds: Option<i64>,
    pub(super) source_status: String,
}

#[cfg(test)]
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

#[derive(FromRow)]
pub(super) struct TimelineRow {
    pub(super) event_type: String,
    pub(super) sequence: i64,
    pub(super) sequence_known: bool,
    pub(super) timestamp: Option<String>,
    pub(super) role: Option<String>,
    pub(super) content: String,
    pub(super) total_chars: i64,
    pub(super) tool_name: Option<String>,
    pub(super) call_id: Option<String>,
    pub(super) outcome: Option<String>,
}

#[derive(FromRow)]
pub(super) struct ToolChunk {
    pub(super) sequence: i64,
    pub(super) sequence_known: bool,
    pub(super) timestamp: Option<String>,
    pub(super) call_id: Option<String>,
    pub(super) tool_name: Option<String>,
    pub(super) kind: String,
    pub(super) content: String,
    pub(super) total_chars: i64,
    pub(super) outcome: String,
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
    pub(super) search: Option<&'a str>,
}

#[derive(FromRow)]
pub(super) struct SearchMatchRow {
    pub(super) session_id: i64,
    pub(super) sequence: i64,
    pub(super) role: String,
    pub(super) content_prefix: String,
    pub(super) snippet: String,
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

pub(super) struct UsageFilters<'a> {
    pub(super) period: &'a str,
    pub(super) group: &'a str,
    pub(super) from: Option<&'a str>,
    pub(super) to: Option<&'a str>,
}

#[derive(FromRow)]
pub(super) struct UsageTotalsRow {
    pub(super) input_tokens: Option<i64>,
    pub(super) output_tokens: Option<i64>,
    pub(super) cached_input_tokens: Option<i64>,
    pub(super) reasoning_output_tokens: Option<i64>,
    pub(super) total_tokens: Option<i64>,
    pub(super) input_available_count: i64,
    pub(super) output_available_count: i64,
    pub(super) cached_available_count: i64,
    pub(super) reasoning_available_count: i64,
    pub(super) total_available_count: i64,
    pub(super) session_count: i64,
    pub(super) observation_count: i64,
    pub(super) unattributed_count: i64,
    pub(super) reset_count: i64,
    pub(super) duplicate_count: i64,
    pub(super) missing_count: i64,
    pub(super) inconsistent_count: i64,
    pub(super) latest_at: Option<String>,
}

#[derive(FromRow)]
pub(super) struct UsageGroupRow {
    pub(super) period: String,
    pub(super) dimension: String,
    pub(super) stable_id: Option<String>,
    pub(super) input_tokens: Option<i64>,
    pub(super) output_tokens: Option<i64>,
    pub(super) cached_input_tokens: Option<i64>,
    pub(super) reasoning_output_tokens: Option<i64>,
    pub(super) total_tokens: Option<i64>,
    pub(super) input_available_count: i64,
    pub(super) output_available_count: i64,
    pub(super) cached_available_count: i64,
    pub(super) reasoning_available_count: i64,
    pub(super) total_available_count: i64,
    pub(super) session_count: i64,
    pub(super) observation_count: i64,
    pub(super) anomaly_count: i64,
}

#[derive(FromRow)]
pub(super) struct RecentUsageRow {
    pub(super) timestamp: String,
    pub(super) stable_id: String,
    pub(super) title: String,
    pub(super) model: String,
    pub(super) input_tokens: Option<i64>,
    pub(super) output_tokens: Option<i64>,
    pub(super) cached_input_tokens: Option<i64>,
    pub(super) reasoning_output_tokens: Option<i64>,
    pub(super) total_tokens: Option<i64>,
    pub(super) anomalous: bool,
}

#[derive(FromRow)]
pub(super) struct TurnRow {
    pub(super) ordinal: i64,
    pub(super) turn_id: Option<String>,
    pub(super) start_sequence: Option<i64>,
    pub(super) start_timestamp: Option<String>,
    pub(super) start_kind: String,
    pub(super) end_sequence: Option<i64>,
    pub(super) end_timestamp: Option<String>,
    pub(super) outcome: String,
    pub(super) resumed: bool,
    pub(super) requested_model: Option<String>,
    pub(super) requested_model_sequence: Option<i64>,
    pub(super) observed_model: Option<String>,
    pub(super) observed_model_sequence: Option<i64>,
    pub(super) duration_seconds: Option<i64>,
    pub(super) tool_request_count: i64,
    pub(super) tool_result_count: i64,
    pub(super) failed_tool_count: i64,
    pub(super) unknown_tool_count: i64,
    pub(super) start_position: i64,
    pub(super) end_position: i64,
    pub(super) requested_model_position: i64,
    pub(super) observed_model_position: i64,
}

#[derive(FromRow)]
pub(super) struct DiagnosticTotalsRow {
    pub(super) interruption_count: i64,
    pub(super) incomplete_count: i64,
    pub(super) accounting_anomaly_count: i64,
    pub(super) observed_failure_count: i64,
}

#[derive(FromRow)]
pub(super) struct PressureRow {
    pub(super) stable_id: String,
    pub(super) title: String,
    pub(super) agent_kind: String,
    pub(super) hostname: String,
    pub(super) os_user: String,
    pub(super) source_sequence: i64,
    pub(super) timestamp: Option<String>,
    pub(super) context_window: Option<i64>,
    pub(super) context_tokens: Option<i64>,
    pub(super) rate_limit_id: Option<String>,
    pub(super) rate_limit_name: Option<String>,
    pub(super) primary_used_percent: Option<f64>,
    pub(super) primary_window_minutes: Option<i64>,
    pub(super) primary_resets_at: Option<i64>,
    pub(super) primary_resets_in_seconds: Option<i64>,
    pub(super) secondary_used_percent: Option<f64>,
    pub(super) secondary_window_minutes: Option<i64>,
    pub(super) secondary_resets_at: Option<i64>,
    pub(super) secondary_resets_in_seconds: Option<i64>,
    pub(super) plan_type: Option<String>,
    pub(super) rate_limit_reached_type: Option<String>,
    pub(super) evidence_position: i64,
    pub(super) observation_age_seconds: Option<i64>,
    pub(super) primary_reset_remaining_seconds: Option<i64>,
    pub(super) secondary_reset_remaining_seconds: Option<i64>,
}

#[derive(FromRow)]
pub(super) struct DiagnosticEvidenceRow {
    pub(super) stable_id: String,
    pub(super) title: String,
    pub(super) timestamp: Option<String>,
    pub(super) sequence: Option<i64>,
    pub(super) ordinal: Option<i64>,
    pub(super) position: i64,
}

#[derive(FromRow)]
pub(crate) struct PostmortemRow {
    pub(crate) stable_id: String,
    pub(crate) agent_kind: String,
    pub(crate) session_status: String,
    pub(crate) source_status: String,
    pub(crate) turn_count: i64,
    pub(crate) completed_turn_count: i64,
    pub(crate) interrupted_turn_count: i64,
    pub(crate) unclosed_turn_count: i64,
    pub(crate) terminal_only_turn_count: i64,
    pub(crate) resumed_turn_count: i64,
    pub(crate) requested_model_observation_count: i64,
    pub(crate) requested_model_change_count: i64,
    pub(crate) served_model_fallback_count: i64,
    pub(crate) input_tokens: Option<i64>,
    pub(crate) output_tokens: Option<i64>,
    pub(crate) cached_input_tokens: Option<i64>,
    pub(crate) reasoning_output_tokens: Option<i64>,
    pub(crate) total_tokens: Option<i64>,
    pub(crate) usage_observation_count: i64,
    pub(crate) usage_reset_count: i64,
    pub(crate) usage_missing_count: i64,
    pub(crate) usage_inconsistent_count: i64,
    pub(crate) tool_request_count: i64,
    pub(crate) tool_result_count: i64,
    pub(crate) observed_tool_failure_count: i64,
    pub(crate) unknown_tool_outcome_count: i64,
    pub(crate) context_observation_count: i64,
    pub(crate) context_gap_count: i64,
    pub(crate) rate_observation_count: i64,
    pub(crate) rate_gap_count: i64,
    pub(crate) timestamp_gap_count: i64,
}

pub(super) async fn sessions(
    pool: &SqlitePool,
    filters: &SessionFilters<'_>,
    projects: &ProjectConfig,
    access: &crate::auth::AccessScope,
    offset: i64,
) -> Result<Vec<SessionRow>, sqlx::Error> {
    let model_exclusion = crate::sources::model_exclusion();
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT s.id,s.stable_id,a.kind agent_kind,s.external_id,s.title,s.started_at,COALESCE(s.ended_at,s.started_at) last_activity,s.model,s.status,s.cwd,",
    );
    push_project_case(&mut query, projects);
    query.push(
        " project,CASE WHEN sf.kind='archived_sessions' THEN 'archived' WHEN sf.kind IS NULL THEN 'unknown' ELSE 'active' END archive_state,h.name hostname,u.os_user,(SELECT count(*) FROM messages m WHERE m.session_id=s.id) message_count,NULL match_sequence,NULL match_snippet FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id LEFT JOIN source_files sf ON sf.id=s.source_file_id WHERE (a.kind<>",
    );
    query
        .push_bind(model_exclusion.source_kind)
        .push(" OR s.model IS NULL OR s.model NOT LIKE ")
        .push_bind(model_exclusion.pattern)
        .push(")");
    push_source_access(&mut query, access);
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
    if let Some(search) = filters.search {
        query
            .push(" AND (instr(lower(COALESCE(s.title,'')),lower(")
            .push_bind(search)
            .push("))>0 OR instr(lower(s.external_id),lower(")
            .push_bind(search)
            .push("))>0 OR instr(lower(COALESCE((");
        push_project_case(&mut query, projects);
        query
            .push("),'')),lower(")
            .push_bind(search)
            .push("))>0 OR s.id IN (SELECT session_id FROM message_search WHERE message_search MATCH ")
            .push_bind(crate::search::fts_query(search))
            .push("))");
    }
    query
        .push(" ORDER BY julianday(COALESCE(s.ended_at,s.started_at)) DESC,s.stable_id DESC LIMIT ")
        .push_bind((SESSION_PAGE_SIZE + 1) as i64)
        .push(" OFFSET ")
        .push_bind(offset);
    query.build_query_as::<SessionRow>().fetch_all(pool).await
}

pub(super) async fn search_matches(
    pool: &SqlitePool,
    session_ids: &[i64],
    search: &str,
) -> Result<Vec<SearchMatchRow>, sqlx::Error> {
    if session_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = QueryBuilder::<Sqlite>::new(
        "WITH candidates AS (SELECT session_id,sequence,role,substr(content,1,",
    );
    query
        .push_bind(MESSAGE_PREVIEW_CHARS)
        .push(") content_prefix,substr(snippet(message_search,0,char(30),char(31),' … ',24),1,")
        .push_bind(SEARCH_SNIPPET_CHARS)
        .push(") snippet,rank FROM message_search WHERE message_search MATCH ")
        .push_bind(crate::search::fts_query(search))
        .push(" AND session_id IN (");
    let mut separated = query.separated(",");
    for session_id in session_ids {
        separated.push_bind(session_id);
    }
    separated.push_unseparated(") ), ranked AS (SELECT session_id,sequence,role,content_prefix,snippet,row_number() OVER (PARTITION BY session_id ORDER BY rank,sequence) position FROM candidates) SELECT session_id,sequence,role,content_prefix,snippet FROM ranked WHERE position<=");
    query
        .push_bind(SEARCH_CANDIDATES_PER_SESSION)
        .push(" ORDER BY session_id,position");
    query
        .build_query_as::<SearchMatchRow>()
        .fetch_all(pool)
        .await
}

pub(super) async fn filter_values(
    pool: &SqlitePool,
    include_origins: bool,
    projects: &ProjectConfig,
    access: &crate::auth::AccessScope,
) -> Result<FilterValues, sqlx::Error> {
    let (hosts, users) = if include_origins {
        let mut hosts = QueryBuilder::<Sqlite>::new(
            "SELECT DISTINCT h.name FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id WHERE 1=1",
        );
        push_source_access(&mut hosts, access);
        hosts.push(" ORDER BY h.name");
        let mut users = QueryBuilder::<Sqlite>::new(
            "SELECT DISTINCT u.os_user FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN users u ON u.id=i.user_id WHERE 1=1",
        );
        push_source_access(&mut users, access);
        users.push(" ORDER BY u.os_user");
        (
            hosts.build_query_scalar::<String>().fetch_all(pool).await?,
            users.build_query_scalar::<String>().fetch_all(pool).await?,
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let model_exclusion = crate::sources::model_exclusion();
    let mut models = QueryBuilder::<Sqlite>::new(
        "SELECT DISTINCT s.model FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE s.model IS NOT NULL AND s.model<>'' AND (a.kind<>",
    );
    models
        .push_bind(model_exclusion.source_kind)
        .push(" OR s.model NOT LIKE ")
        .push_bind(model_exclusion.pattern)
        .push(")");
    push_source_access(&mut models, access);
    models.push(" ORDER BY s.model");
    let models = models
        .build_query_scalar::<String>()
        .fetch_all(pool)
        .await?;
    let mut statuses = QueryBuilder::<Sqlite>::new(
        "SELECT DISTINCT s.status FROM sessions s JOIN agent_instances i ON i.id=s.instance_id WHERE s.status IS NOT NULL AND s.status<>''",
    );
    push_source_access(&mut statuses, access);
    statuses.push(" ORDER BY s.status");
    let statuses = statuses
        .build_query_scalar::<String>()
        .fetch_all(pool)
        .await?;
    let mut agents = QueryBuilder::<Sqlite>::new(
        "SELECT DISTINCT a.kind FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE (a.kind<>",
    );
    agents
        .push_bind(model_exclusion.source_kind)
        .push(" OR s.model IS NULL OR s.model NOT LIKE ")
        .push_bind(model_exclusion.pattern)
        .push(")");
    push_source_access(&mut agents, access);
    agents.push(" ORDER BY a.kind");
    let agents = agents
        .build_query_scalar::<String>()
        .fetch_all(pool)
        .await?;
    let mut archives = QueryBuilder::<Sqlite>::new(
        "SELECT DISTINCT CASE WHEN sf.kind='archived_sessions' THEN 'archived' WHEN sf.kind IS NULL THEN 'unknown' ELSE 'active' END FROM sessions s LEFT JOIN source_files sf ON sf.id=s.source_file_id JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE (a.kind<>",
    );
    archives
        .push_bind(model_exclusion.source_kind)
        .push(" OR s.model IS NULL OR s.model NOT LIKE ")
        .push_bind(model_exclusion.pattern)
        .push(")");
    push_source_access(&mut archives, access);
    archives.push(" ORDER BY 1");
    let archives = archives
        .build_query_scalar::<String>()
        .fetch_all(pool)
        .await?;
    let mut project_query = QueryBuilder::<Sqlite>::new("SELECT DISTINCT project FROM (SELECT ");
    push_project_case(&mut project_query, projects);
    project_query
        .push(" project FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id WHERE (a.kind<>")
        .push_bind(model_exclusion.source_kind)
        .push(" OR s.model IS NULL OR s.model NOT LIKE ")
        .push_bind(model_exclusion.pattern)
        .push(")");
    push_source_access(&mut project_query, access);
    project_query.push(") WHERE project IS NOT NULL ORDER BY project");
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

pub(super) async fn usage_totals(
    pool: &SqlitePool,
    filters: &UsageFilters<'_>,
    projects: &ProjectConfig,
    access: &crate::auth::AccessScope,
) -> Result<UsageTotalsRow, sqlx::Error> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "WITH scoped AS (SELECT u.* FROM usage_events u JOIN sessions s ON s.id=u.session_id JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE ",
    );
    push_visible_model(&mut query);
    push_source_access(&mut query, access);
    query.push("), attributed AS (SELECT * FROM scoped u WHERE datetime(u.timestamp) IS NOT NULL");
    push_usage_date_boundaries(&mut query, filters, projects.timezone());
    query.push(
        ") SELECT SUM(delta_input_tokens) input_tokens,SUM(delta_output_tokens) output_tokens,SUM(delta_cached_input_tokens) cached_input_tokens,SUM(delta_reasoning_output_tokens) reasoning_output_tokens,SUM(delta_total_tokens) total_tokens,COUNT(delta_input_tokens) input_available_count,COUNT(delta_output_tokens) output_available_count,COUNT(delta_cached_input_tokens) cached_available_count,COUNT(delta_reasoning_output_tokens) reasoning_available_count,COUNT(delta_total_tokens) total_available_count,COUNT(DISTINCT session_id) session_count,COUNT(*) observation_count,(SELECT count(*) FROM scoped WHERE datetime(timestamp) IS NULL) unattributed_count,COALESCE(SUM(has_reset),0) reset_count,COALESCE(SUM(is_duplicate),0) duplicate_count,COALESCE(SUM(has_missing),0) missing_count,COALESCE(SUM(has_inconsistent),0) inconsistent_count,MAX(timestamp) latest_at FROM attributed",
    );
    query
        .build_query_as::<UsageTotalsRow>()
        .fetch_one(pool)
        .await
}

pub(super) async fn usage_groups(
    pool: &SqlitePool,
    filters: &UsageFilters<'_>,
    projects: &ProjectConfig,
    access: &crate::auth::AccessScope,
    offset: i64,
) -> Result<Vec<UsageGroupRow>, sqlx::Error> {
    let mut query = QueryBuilder::<Sqlite>::new("SELECT ");
    push_usage_period(&mut query, filters.period, projects.timezone());
    query.push(" period,");
    push_usage_dimension(&mut query, filters.group, projects);
    query.push(" dimension,");
    if filters.group == "session" {
        query.push("s.stable_id");
    } else {
        query.push("NULL");
    }
    query.push(
        " stable_id,SUM(u.delta_input_tokens) input_tokens,SUM(u.delta_output_tokens) output_tokens,SUM(u.delta_cached_input_tokens) cached_input_tokens,SUM(u.delta_reasoning_output_tokens) reasoning_output_tokens,SUM(u.delta_total_tokens) total_tokens,COUNT(u.delta_input_tokens) input_available_count,COUNT(u.delta_output_tokens) output_available_count,COUNT(u.delta_cached_input_tokens) cached_available_count,COUNT(u.delta_reasoning_output_tokens) reasoning_available_count,COUNT(u.delta_total_tokens) total_available_count,COUNT(DISTINCT s.id) session_count,COUNT(*) observation_count,SUM(CASE WHEN u.has_reset OR u.has_missing OR u.has_inconsistent THEN 1 ELSE 0 END) anomaly_count FROM usage_events u JOIN sessions s ON s.id=u.session_id JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id JOIN users usr ON usr.id=i.user_id JOIN hosts h ON h.id=usr.host_id WHERE datetime(u.timestamp) IS NOT NULL AND ",
    );
    push_visible_model(&mut query);
    push_source_access(&mut query, access);
    push_usage_date_boundaries(&mut query, filters, projects.timezone());
    query.push(" GROUP BY period,dimension,stable_id ORDER BY period DESC,total_tokens DESC,dimension LIMIT ")
        .push_bind((USAGE_PAGE_SIZE + 1) as i64)
        .push(" OFFSET ")
        .push_bind(offset);
    query
        .build_query_as::<UsageGroupRow>()
        .fetch_all(pool)
        .await
}

pub(super) async fn recent_usage(
    pool: &SqlitePool,
    filters: &UsageFilters<'_>,
    projects: &ProjectConfig,
    access: &crate::auth::AccessScope,
) -> Result<Vec<RecentUsageRow>, sqlx::Error> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT u.timestamp,s.stable_id,COALESCE(s.title,s.external_id) title,COALESCE(NULLIF(s.model,''),'Unknown model') model,u.delta_input_tokens input_tokens,u.delta_output_tokens output_tokens,u.delta_cached_input_tokens cached_input_tokens,u.delta_reasoning_output_tokens reasoning_output_tokens,u.delta_total_tokens total_tokens,(u.has_reset OR u.has_missing OR u.has_inconsistent) anomalous FROM usage_events u JOIN sessions s ON s.id=u.session_id JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE datetime(u.timestamp) IS NOT NULL AND ",
    );
    push_visible_model(&mut query);
    push_source_access(&mut query, access);
    push_usage_date_boundaries(&mut query, filters, projects.timezone());
    query
        .push(" ORDER BY julianday(u.timestamp) DESC,u.id DESC LIMIT ")
        .push_bind(RECENT_USAGE_LIMIT);
    query
        .build_query_as::<RecentUsageRow>()
        .fetch_all(pool)
        .await
}

pub(super) async fn session_turns(
    pool: &SqlitePool,
    session_id: i64,
    offset: i64,
) -> Result<Vec<TurnRow>, sqlx::Error> {
    sqlx::query_as::<_, TurnRow>(
        "SELECT t.ordinal,t.turn_id,t.start_sequence,t.start_timestamp,t.start_kind,t.end_sequence,t.end_timestamp,t.outcome,t.resumed,t.requested_model,t.requested_model_sequence,t.observed_model,t.observed_model_sequence,CASE WHEN unixepoch(t.start_timestamp) IS NOT NULL AND unixepoch(t.end_timestamp)>=unixepoch(t.start_timestamp) THEN unixepoch(t.end_timestamp)-unixepoch(t.start_timestamp) END duration_seconds,(SELECT count(*) FROM tool_calls c WHERE c.session_id=t.session_id AND c.turn_ordinal=t.ordinal) tool_request_count,(SELECT count(*) FROM tool_results r WHERE r.session_id=t.session_id AND r.turn_ordinal=t.ordinal) tool_result_count,(SELECT count(*) FROM tool_results r WHERE r.session_id=t.session_id AND r.turn_ordinal=t.ordinal AND r.outcome='failed') failed_tool_count,(SELECT count(*) FROM tool_calls c WHERE c.session_id=t.session_id AND c.turn_ordinal=t.ordinal AND NOT EXISTS(SELECT 1 FROM tool_results r WHERE r.tool_call_id=c.id)) unknown_tool_count,(SELECT count(*) FROM turn_events e WHERE e.session_id=t.session_id AND e.source_sequence<COALESCE(t.start_sequence,t.end_sequence)) start_position,(SELECT count(*) FROM turn_events e WHERE e.session_id=t.session_id AND e.source_sequence<t.end_sequence) end_position,(SELECT count(*) FROM turn_events e WHERE e.session_id=t.session_id AND e.source_sequence<t.requested_model_sequence) requested_model_position,(SELECT count(*) FROM turn_events e WHERE e.session_id=t.session_id AND e.source_sequence<t.observed_model_sequence) observed_model_position FROM turns t WHERE t.session_id=? ORDER BY t.ordinal DESC LIMIT ? OFFSET ?",
    )
    .bind(session_id)
    .bind((TURN_PAGE_SIZE + 1) as i64)
    .bind(offset)
    .fetch_all(pool)
    .await
}

pub(crate) async fn postmortem(
    pool: &SqlitePool,
    session_id: i64,
) -> Result<PostmortemRow, sqlx::Error> {
    sqlx::query_as(
        "SELECT s.stable_id,a.kind agent_kind,COALESCE(s.status,'unknown') session_status,COALESCE(sf.parse_status,'unknown') source_status, \
         (SELECT count(*) FROM turns t WHERE t.session_id=s.id) turn_count, \
         (SELECT count(*) FROM turns t WHERE t.session_id=s.id AND t.outcome='complete') completed_turn_count, \
         (SELECT count(*) FROM turns t WHERE t.session_id=s.id AND t.outcome='interrupted') interrupted_turn_count, \
         (SELECT count(*) FROM turns t WHERE t.session_id=s.id AND t.outcome='unclosed') unclosed_turn_count, \
         (SELECT count(*) FROM turns t WHERE t.session_id=s.id AND t.start_sequence IS NULL) terminal_only_turn_count, \
         (SELECT count(*) FROM turns t WHERE t.session_id=s.id AND t.resumed) resumed_turn_count, \
         (SELECT count(*) FROM turn_events e WHERE e.session_id=s.id AND e.kind='requested_model') requested_model_observation_count, \
         (SELECT count(*) FROM turn_events e WHERE e.session_id=s.id AND e.is_model_change) requested_model_change_count, \
         (SELECT count(*) FROM turn_events e WHERE e.session_id=s.id AND e.kind='served_model_fallback') served_model_fallback_count, \
         su.input_tokens,su.output_tokens,su.cached_input_tokens,su.reasoning_output_tokens,su.total_tokens, \
         (SELECT count(*) FROM usage_events u WHERE u.session_id=s.id) usage_observation_count, \
         COALESCE(su.reset_count,0) usage_reset_count,COALESCE(su.missing_count,0) usage_missing_count,COALESCE(su.inconsistent_count,0) usage_inconsistent_count, \
         (SELECT count(*) FROM tool_calls c WHERE c.session_id=s.id) tool_request_count, \
         (SELECT count(*) FROM tool_results r WHERE r.session_id=s.id) tool_result_count, \
         (SELECT count(*) FROM tool_results r WHERE r.session_id=s.id AND r.outcome='failed') observed_tool_failure_count, \
         (SELECT count(*) FROM tool_calls c WHERE c.session_id=s.id AND NOT EXISTS(SELECT 1 FROM tool_results r WHERE r.tool_call_id=c.id)) unknown_tool_outcome_count, \
         (SELECT count(*) FROM usage_events u WHERE u.session_id=s.id AND u.context_window IS NOT NULL AND u.context_tokens IS NOT NULL) context_observation_count, \
         (SELECT count(*) FROM usage_events u WHERE u.session_id=s.id AND (u.context_window IS NULL OR u.context_tokens IS NULL)) context_gap_count, \
         (SELECT count(*) FROM usage_events u WHERE u.session_id=s.id AND (u.primary_used_percent IS NOT NULL OR u.secondary_used_percent IS NOT NULL)) rate_observation_count, \
         (SELECT count(*) FROM usage_events u WHERE u.session_id=s.id AND u.primary_used_percent IS NULL AND u.secondary_used_percent IS NULL) rate_gap_count, \
         ((SELECT count(*) FROM turn_events e WHERE e.session_id=s.id AND datetime(e.timestamp) IS NULL)+(SELECT count(*) FROM usage_events u WHERE u.session_id=s.id AND datetime(u.timestamp) IS NULL)+(SELECT count(*) FROM tool_calls c WHERE c.session_id=s.id AND datetime(c.timestamp) IS NULL)+(SELECT count(*) FROM tool_results r WHERE r.session_id=s.id AND datetime(r.timestamp) IS NULL)) timestamp_gap_count \
         FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id LEFT JOIN source_files sf ON sf.id=s.source_file_id LEFT JOIN session_usage su ON su.session_id=s.id WHERE s.id=?",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await
}

pub(super) async fn diagnostic_totals(
    pool: &SqlitePool,
    access: &crate::auth::AccessScope,
) -> Result<DiagnosticTotalsRow, sqlx::Error> {
    let exclusion = crate::sources::model_exclusion();
    sqlx::query_as(
        "WITH visible AS (SELECT s.id FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE (a.kind<>? OR lower(COALESCE(s.model,'')) NOT LIKE ? ESCAPE '\\') AND (? OR i.source_key IN (SELECT value FROM json_each(?)))) SELECT (SELECT count(*) FROM turns t JOIN visible v ON v.id=t.session_id WHERE t.outcome='interrupted') interruption_count,(SELECT count(*) FROM turns t JOIN visible v ON v.id=t.session_id WHERE t.outcome='unclosed') incomplete_count,(SELECT count(*) FROM usage_events u JOIN visible v ON v.id=u.session_id WHERE u.has_reset OR u.has_missing OR u.has_inconsistent) accounting_anomaly_count,(SELECT count(*) FROM tool_results r JOIN visible v ON v.id=r.session_id WHERE r.outcome='failed') observed_failure_count",
    )
    .bind(exclusion.source_kind)
    .bind(exclusion.pattern)
    .bind(access.unrestricted())
    .bind(access.source_keys_json())
    .fetch_one(pool)
    .await
}

pub(super) async fn pressure_snapshots(
    pool: &SqlitePool,
    access: &crate::auth::AccessScope,
) -> Result<Vec<PressureRow>, sqlx::Error> {
    let exclusion = crate::sources::model_exclusion();
    sqlx::query_as(
        "SELECT s.stable_id,COALESCE(s.title,s.external_id) title,a.kind agent_kind,h.name hostname,usr.os_user,u.source_sequence,u.timestamp,u.context_window,u.context_tokens,u.rate_limit_id,u.rate_limit_name,u.primary_used_percent,u.primary_window_minutes,u.primary_resets_at,u.primary_resets_in_seconds,u.secondary_used_percent,u.secondary_window_minutes,u.secondary_resets_at,u.secondary_resets_in_seconds,u.plan_type,u.rate_limit_reached_type,(SELECT count(*) FROM usage_events p WHERE p.session_id=u.session_id AND p.sequence_known AND p.source_sequence<u.source_sequence) evidence_position,unixepoch('now')-unixepoch(u.timestamp) observation_age_seconds,COALESCE(u.primary_resets_at-unixepoch('now'),u.primary_resets_in_seconds-(unixepoch('now')-unixepoch(u.timestamp))) primary_reset_remaining_seconds,COALESCE(u.secondary_resets_at-unixepoch('now'),u.secondary_resets_in_seconds-(unixepoch('now')-unixepoch(u.timestamp))) secondary_reset_remaining_seconds FROM usage_events u JOIN sessions s ON s.id=u.session_id JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id JOIN users usr ON usr.id=i.user_id JOIN hosts h ON h.id=usr.host_id WHERE u.sequence_known AND u.source_sequence IS NOT NULL AND (u.context_window IS NOT NULL OR u.context_tokens IS NOT NULL OR u.primary_used_percent IS NOT NULL OR u.secondary_used_percent IS NOT NULL OR u.rate_limit_id IS NOT NULL OR u.rate_limit_name IS NOT NULL OR u.rate_limit_reached_type IS NOT NULL) AND (a.kind<>? OR lower(COALESCE(s.model,'')) NOT LIKE ? ESCAPE '\\') AND (? OR i.source_key IN (SELECT value FROM json_each(?))) ORDER BY datetime(u.timestamp) IS NULL,datetime(u.timestamp) DESC,s.stable_id,u.source_sequence DESC LIMIT ?",
    )
    .bind(exclusion.source_kind)
    .bind(exclusion.pattern)
    .bind(access.unrestricted())
    .bind(access.source_keys_json())
    .bind(PRESSURE_SNAPSHOT_LIMIT)
    .fetch_all(pool)
    .await
}

pub(super) async fn diagnostic_evidence(
    pool: &SqlitePool,
    kind: &str,
    access: &crate::auth::AccessScope,
) -> Result<Vec<DiagnosticEvidenceRow>, sqlx::Error> {
    let sql = match kind {
        "interruption" => {
            "SELECT s.stable_id,COALESCE(s.title,s.external_id) title,t.end_timestamp timestamp,t.end_sequence sequence,t.ordinal,(SELECT count(*) FROM turn_events e WHERE e.session_id=t.session_id AND e.source_sequence<t.end_sequence) position FROM turns t JOIN sessions s ON s.id=t.session_id JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE t.outcome='interrupted' AND (a.kind<>? OR lower(COALESCE(s.model,'')) NOT LIKE ? ESCAPE '\\') AND (? OR i.source_key IN (SELECT value FROM json_each(?))) ORDER BY datetime(t.end_timestamp) IS NULL,datetime(t.end_timestamp) DESC,s.stable_id,t.ordinal DESC LIMIT ?"
        }
        "incomplete" => {
            "SELECT s.stable_id,COALESCE(s.title,s.external_id) title,t.start_timestamp timestamp,t.start_sequence sequence,t.ordinal,(SELECT count(*) FROM turns newer WHERE newer.session_id=t.session_id AND newer.ordinal>t.ordinal) position FROM turns t JOIN sessions s ON s.id=t.session_id JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE t.outcome='unclosed' AND (a.kind<>? OR lower(COALESCE(s.model,'')) NOT LIKE ? ESCAPE '\\') AND (? OR i.source_key IN (SELECT value FROM json_each(?))) ORDER BY datetime(t.start_timestamp) IS NULL,datetime(t.start_timestamp) DESC,s.stable_id,t.ordinal DESC LIMIT ?"
        }
        "accounting" => {
            "SELECT s.stable_id,COALESCE(s.title,s.external_id) title,u.timestamp,u.source_sequence sequence,NULL ordinal,(SELECT count(*) FROM usage_events p WHERE p.session_id=u.session_id AND p.sequence_known AND p.source_sequence<u.source_sequence) position FROM usage_events u JOIN sessions s ON s.id=u.session_id JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE u.sequence_known AND u.source_sequence IS NOT NULL AND (u.has_reset OR u.has_missing OR u.has_inconsistent) AND (a.kind<>? OR lower(COALESCE(s.model,'')) NOT LIKE ? ESCAPE '\\') AND (? OR i.source_key IN (SELECT value FROM json_each(?))) ORDER BY datetime(u.timestamp) IS NULL,datetime(u.timestamp) DESC,s.stable_id,u.source_sequence DESC LIMIT ?"
        }
        "failure" => {
            "SELECT s.stable_id,COALESCE(s.title,s.external_id) title,r.timestamp,r.source_sequence sequence,NULL ordinal,(SELECT count(*) FROM (SELECT c2.sequence,1 event_order FROM tool_calls c2 WHERE c2.session_id=r.session_id AND c2.kind!='user_input' UNION ALL SELECT r2.source_sequence,2 FROM tool_results r2 LEFT JOIN tool_calls c2 ON c2.id=r2.tool_call_id WHERE r2.session_id=r.session_id AND COALESCE(c2.kind,'tool')!='user_input') WHERE sequence<r.source_sequence OR sequence=r.source_sequence AND event_order<2) position FROM tool_results r JOIN sessions s ON s.id=r.session_id JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE r.outcome='failed' AND r.sequence_known AND (a.kind<>? OR lower(COALESCE(s.model,'')) NOT LIKE ? ESCAPE '\\') AND (? OR i.source_key IN (SELECT value FROM json_each(?))) ORDER BY datetime(r.timestamp) IS NULL,datetime(r.timestamp) DESC,s.stable_id,r.source_sequence DESC LIMIT ?"
        }
        _ => return Ok(Vec::new()),
    };
    let exclusion = crate::sources::model_exclusion();
    sqlx::query_as(sql)
        .bind(exclusion.source_kind)
        .bind(exclusion.pattern)
        .bind(access.unrestricted())
        .bind(access.source_keys_json())
        .bind(DIAGNOSTIC_EVIDENCE_LIMIT)
        .fetch_all(pool)
        .await
}

fn push_visible_model<'a>(query: &mut QueryBuilder<'a, Sqlite>) {
    let exclusion = crate::sources::model_exclusion();
    query
        .push("(a.kind<>")
        .push_bind(exclusion.source_kind)
        .push(" OR s.model IS NULL OR s.model NOT LIKE ")
        .push_bind(exclusion.pattern)
        .push(")");
}

fn push_usage_date_boundaries<'a>(
    query: &mut QueryBuilder<'a, Sqlite>,
    filters: &UsageFilters<'a>,
    timezone: &'a str,
) {
    for (value, comparison) in [(filters.from, ">="), (filters.to, "<=")] {
        query
            .push(" AND (")
            .push_bind(value)
            .push(" IS NULL OR date(u.timestamp,")
            .push_bind(timezone)
            .push(") ")
            .push(comparison)
            .push(" ")
            .push_bind(value)
            .push(")");
    }
}

fn push_usage_period<'a>(query: &mut QueryBuilder<'a, Sqlite>, period: &str, timezone: &'a str) {
    match period {
        "week" => {
            query
                .push("date(u.timestamp,")
                .push_bind(timezone)
                .push(",'-' || ((CAST(strftime('%w',u.timestamp,")
                .push_bind(timezone)
                .push(") AS INTEGER)+6)%7) || ' days')");
        }
        "month" => {
            query
                .push("strftime('%Y-%m',u.timestamp,")
                .push_bind(timezone)
                .push(")");
        }
        _ => {
            query
                .push("date(u.timestamp,")
                .push_bind(timezone)
                .push(")");
        }
    }
}

fn push_usage_dimension<'a>(
    query: &mut QueryBuilder<'a, Sqlite>,
    group: &str,
    projects: &'a ProjectConfig,
) {
    match group {
        "model" => query.push("COALESCE(NULLIF(s.model,''),'Unknown model')"),
        "project" => {
            query.push("COALESCE((");
            push_project_case(query, projects);
            query.push("),'Unassigned project')")
        }
        "host" => query.push("h.name"),
        "user" => query.push("usr.os_user"),
        "agent" => query.push("a.kind"),
        _ => query.push("COALESCE(s.title,s.external_id)"),
    };
}

pub(super) async fn session(
    pool: &SqlitePool,
    stable_id: &str,
    projects: &ProjectConfig,
    access: &crate::auth::AccessScope,
) -> Result<Option<SessionRow>, sqlx::Error> {
    let model_exclusion = crate::sources::model_exclusion();
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT s.id,s.stable_id,a.kind agent_kind,s.external_id,s.title,s.started_at,COALESCE(s.ended_at,s.started_at) last_activity,s.model,s.status,s.cwd,",
    );
    push_project_case(&mut query, projects);
    query
        .push(" project,CASE WHEN sf.kind='archived_sessions' THEN 'archived' WHEN sf.kind IS NULL THEN 'unknown' ELSE 'active' END archive_state,h.name hostname,u.os_user,(SELECT count(*) FROM messages m WHERE m.session_id=s.id) message_count,NULL match_sequence,NULL match_snippet FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id LEFT JOIN source_files sf ON sf.id=s.source_file_id WHERE s.stable_id=")
        .push_bind(stable_id)
        .push(" AND (a.kind<>")
        .push_bind(model_exclusion.source_kind)
        .push(" OR s.model IS NULL OR s.model NOT LIKE ")
        .push_bind(model_exclusion.pattern)
        .push(")");
    push_source_access(&mut query, access);
    query
        .build_query_as::<SessionRow>()
        .fetch_optional(pool)
        .await
}

pub(super) async fn live_status(
    pool: &SqlitePool,
    stable_id: &str,
    access: &crate::auth::AccessScope,
) -> Result<Option<LiveStatusRow>, sqlx::Error> {
    let model_exclusion = crate::sources::model_exclusion();
    sqlx::query_as(
        "SELECT COALESCE(sf.ingest_run_id,0) revision, \
         ir.finished_at last_successful_import, \
         sf.observed_at source_observed_at, \
         CAST(unixepoch('now')-unixepoch(sf.observed_at) AS INTEGER) source_age_seconds, \
         COALESCE(sf.parse_status,'unknown') source_status \
         FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id LEFT JOIN source_files sf ON sf.id=s.source_file_id LEFT JOIN ingest_runs ir ON ir.id=sf.ingest_run_id AND ir.status='complete' \
         WHERE s.stable_id=? AND (a.kind<>? OR s.model IS NULL OR s.model NOT LIKE ?) AND (? OR i.source_key IN (SELECT value FROM json_each(?)))",
    )
    .bind(stable_id)
    .bind(model_exclusion.source_kind)
    .bind(model_exclusion.pattern)
    .bind(access.unrestricted())
    .bind(access.source_keys_json())
    .fetch_optional(pool)
    .await
}

fn push_source_access<'a>(
    query: &mut QueryBuilder<'a, Sqlite>,
    access: &'a crate::auth::AccessScope,
) {
    query
        .push(" AND (")
        .push_bind(access.unrestricted())
        .push(" OR i.source_key IN (SELECT value FROM json_each(")
        .push_bind(access.source_keys_json())
        .push(")))");
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

pub(super) async fn timeline_section(
    pool: &SqlitePool,
    session_id: i64,
    event_filter: &str,
    offset: i64,
) -> Result<Vec<TimelineRow>, sqlx::Error> {
    sqlx::query_as::<_, TimelineRow>(
        "SELECT 'diagnostic_event' event_type,u.source_sequence sequence,u.sequence_known,u.timestamp,NULL role,trim(CASE WHEN u.context_window IS NOT NULL OR u.context_tokens IS NOT NULL THEN 'Context: ' || COALESCE(u.context_tokens,'Unavailable') || ' / ' || COALESCE(u.context_window,'Unavailable') || char(10) ELSE '' END || CASE WHEN u.primary_used_percent IS NOT NULL THEN printf('Primary rate: %.1f%% used',u.primary_used_percent) || char(10) ELSE '' END || CASE WHEN u.secondary_used_percent IS NOT NULL THEN printf('Secondary rate: %.1f%% used',u.secondary_used_percent) || char(10) ELSE '' END || CASE WHEN u.has_reset OR u.has_missing OR u.has_inconsistent THEN 'Accounting flags: ' || trim(CASE WHEN u.has_reset THEN 'reset ' ELSE '' END || CASE WHEN u.has_missing THEN 'missing ' ELSE '' END || CASE WHEN u.has_inconsistent THEN 'inconsistent' ELSE '' END) ELSE '' END) content,length(COALESCE(u.raw_json,'')) total_chars,CASE WHEN u.context_window IS NOT NULL OR u.context_tokens IS NOT NULL OR u.primary_used_percent IS NOT NULL OR u.secondary_used_percent IS NOT NULL OR u.rate_limit_id IS NOT NULL OR u.rate_limit_name IS NOT NULL OR u.rate_limit_reached_type IS NOT NULL THEN 'pressure' ELSE 'accounting' END tool_name,NULL call_id,CASE WHEN u.has_reset OR u.has_missing OR u.has_inconsistent THEN 'review evidence' END outcome,3 event_order \
         FROM usage_events u WHERE u.session_id=? AND u.sequence_known AND u.source_sequence IS NOT NULL AND ?='diagnostics' \
         UNION ALL \
         SELECT 'turn_event' event_type,e.source_sequence sequence,1 sequence_known,e.timestamp,NULL role,CASE e.kind WHEN 'requested_model' THEN 'Requested model: ' || COALESCE(e.requested_model,'Unavailable') WHEN 'served_model_fallback' THEN 'Requested model: ' || COALESCE(e.requested_model,'Unavailable') || char(10) || 'Observed served model: ' || COALESCE(e.served_model,'Unavailable') ELSE '' END content,length(CASE e.kind WHEN 'requested_model' THEN 'Requested model: ' || COALESCE(e.requested_model,'Unavailable') WHEN 'served_model_fallback' THEN 'Requested model: ' || COALESCE(e.requested_model,'Unavailable') || char(10) || 'Observed served model: ' || COALESCE(e.served_model,'Unavailable') ELSE '' END) total_chars,e.kind tool_name,substr(e.turn_id,1,160) call_id,CASE WHEN e.is_resumption THEN 'resumed' WHEN e.is_model_change THEN 'requested model changed' END outcome,-1 event_order \
         FROM turn_events e WHERE e.session_id=? AND ? IN ('all','turns') \
         UNION ALL \
         SELECT 'message' event_type,m.sequence,1 sequence_known,m.timestamp,m.role,substr(m.content,1,?) content,length(m.content) total_chars,NULL tool_name,NULL call_id,NULL outcome,0 event_order \
         FROM messages m WHERE m.session_id=? AND ? IN ('all','messages') \
         UNION ALL \
         SELECT CASE WHEN c.kind='user_input' THEN 'question' ELSE 'tool_call' END,c.sequence,1,c.timestamp,NULL,substr(COALESCE(c.arguments,''),1,?),length(COALESCE(c.arguments,'')),substr(c.tool_name,1,160),substr(c.call_id,1,160), \
           CASE WHEN EXISTS(SELECT 1 FROM tool_results r WHERE r.tool_call_id=c.id AND r.outcome='failed') THEN 'failed' WHEN EXISTS(SELECT 1 FROM tool_results r WHERE r.tool_call_id=c.id) THEN 'output observed' ELSE 'unknown' END,1 \
         FROM tool_calls c WHERE c.session_id=? AND (?='all' OR ?='tools' AND c.kind!='user_input' OR ?='questions' AND c.kind='user_input') \
         UNION ALL \
         SELECT CASE WHEN c.kind='user_input' THEN 'answer' ELSE 'tool_result' END,r.source_sequence,r.sequence_known,r.timestamp,NULL,substr(COALESCE(r.output,''),1,?),length(COALESCE(r.output,'')),substr(c.tool_name,1,160),substr(r.call_id,1,160),r.outcome,2 \
         FROM tool_results r LEFT JOIN tool_calls c ON c.id=r.tool_call_id WHERE r.session_id=? AND (?='all' OR ?='tools' AND COALESCE(c.kind,'tool')!='user_input' OR ?='questions' AND c.kind='user_input') \
         ORDER BY sequence,event_order LIMIT ? OFFSET ?",
    )
    .bind(session_id)
    .bind(event_filter)
    .bind(session_id)
    .bind(event_filter)
    .bind(MESSAGE_PREVIEW_CHARS)
    .bind(session_id)
    .bind(event_filter)
    .bind(MESSAGE_PREVIEW_CHARS)
    .bind(session_id)
    .bind(event_filter)
    .bind(event_filter)
    .bind(event_filter)
    .bind(MESSAGE_PREVIEW_CHARS)
    .bind(session_id)
    .bind(event_filter)
    .bind(event_filter)
    .bind(event_filter)
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

pub(super) async fn preceding_timeline_event_count(
    pool: &SqlitePool,
    session_id: i64,
    sequence: i64,
    event_order: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM ( \
         SELECT source_sequence sequence,-1 event_order FROM turn_events WHERE session_id=? \
         UNION ALL SELECT sequence,0 event_order FROM messages WHERE session_id=? \
         UNION ALL SELECT sequence,1 FROM tool_calls WHERE session_id=? \
         UNION ALL SELECT source_sequence,2 FROM tool_results WHERE session_id=? \
         ) WHERE sequence<? OR sequence=? AND event_order<?",
    )
    .bind(session_id)
    .bind(session_id)
    .bind(session_id)
    .bind(session_id)
    .bind(sequence)
    .bind(sequence)
    .bind(event_order)
    .fetch_one(pool)
    .await
}

pub(super) async fn tool_call_chunk(
    pool: &SqlitePool,
    session_id: i64,
    sequence: i64,
    offset: i64,
) -> Result<Option<ToolChunk>, sqlx::Error> {
    sqlx::query_as::<_, ToolChunk>(
        "SELECT c.sequence,1 sequence_known,c.timestamp,substr(c.call_id,1,160) call_id,substr(c.tool_name,1,160) tool_name,c.kind,substr(COALESCE(c.arguments,''),? + 1,?) content,length(COALESCE(c.arguments,'')) total_chars, \
         CASE WHEN EXISTS(SELECT 1 FROM tool_results r WHERE r.tool_call_id=c.id AND r.outcome='failed') THEN 'failed' WHEN EXISTS(SELECT 1 FROM tool_results r WHERE r.tool_call_id=c.id) THEN 'output observed' ELSE 'unknown' END outcome \
         FROM tool_calls c WHERE c.session_id=? AND c.sequence=?",
    )
    .bind(offset)
    .bind(MESSAGE_CHUNK_CHARS)
    .bind(session_id)
    .bind(sequence)
    .fetch_optional(pool)
    .await
}

pub(super) async fn tool_result_chunk(
    pool: &SqlitePool,
    session_id: i64,
    sequence: i64,
    offset: i64,
) -> Result<Option<ToolChunk>, sqlx::Error> {
    sqlx::query_as::<_, ToolChunk>(
        "SELECT r.source_sequence sequence,r.sequence_known,r.timestamp,substr(r.call_id,1,160) call_id,substr(c.tool_name,1,160) tool_name,COALESCE(c.kind,'tool') kind,substr(COALESCE(r.output,''),? + 1,?) content,length(COALESCE(r.output,'')) total_chars,r.outcome \
         FROM tool_results r LEFT JOIN tool_calls c ON c.id=r.tool_call_id WHERE r.session_id=? AND r.source_sequence=? ORDER BY r.sequence_known DESC,r.id LIMIT 1",
    )
    .bind(offset)
    .bind(MESSAGE_CHUNK_CHARS)
    .bind(session_id)
    .bind(sequence)
    .fetch_optional(pool)
    .await
}
