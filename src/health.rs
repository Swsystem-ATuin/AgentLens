use anyhow::Result;
use sqlx::{FromRow, SqlitePool};

pub(crate) const SOURCE_PAGE_SIZE: usize = 100;

#[derive(FromRow)]
struct RawSourceHealth {
    source_key: String,
    path: String,
    kind: String,
    parse_status: String,
    parse_error: Option<String>,
    observed_at: Option<String>,
    age_seconds: Option<i64>,
    hostname: String,
    os_user: String,
    agent_kind: String,
    stable_id: Option<String>,
}

#[derive(FromRow, Default)]
pub(crate) struct HealthTotals {
    pub(crate) source_count: i64,
    pub(crate) ok_count: i64,
    pub(crate) partial_count: i64,
    pub(crate) error_count: i64,
}

#[derive(FromRow)]
struct RawIngestHealth {
    started_at: String,
    finished_at: Option<String>,
    status: String,
    files_seen: i64,
    errors: i64,
    duration_ms: Option<i64>,
    failure_reason: Option<String>,
}

pub(crate) struct IngestHealth {
    pub(crate) started_at: String,
    pub(crate) finished_at: Option<String>,
    pub(crate) status: &'static str,
    pub(crate) files_seen: i64,
    pub(crate) errors: i64,
    pub(crate) duration_ms: Option<i64>,
    pub(crate) failure_reason: Option<&'static str>,
}

pub(crate) struct SourceHealth {
    pub(crate) source_file_id: String,
    pub(crate) source_id: String,
    pub(crate) kind: String,
    pub(crate) parse_status: &'static str,
    pub(crate) reason: Option<&'static str>,
    pub(crate) observed_at: Option<String>,
    pub(crate) freshness: &'static str,
    pub(crate) hostname: String,
    pub(crate) os_user: String,
    pub(crate) agent_kind: String,
    pub(crate) stable_id: Option<String>,
}

pub(crate) struct HealthSnapshot {
    pub(crate) latest_run: Option<IngestHealth>,
    pub(crate) totals: HealthTotals,
    pub(crate) sources: Vec<SourceHealth>,
}

pub(crate) async fn snapshot(
    pool: &SqlitePool,
    offset: i64,
    page_size: usize,
) -> Result<HealthSnapshot, sqlx::Error> {
    let (latest_run, totals, rows) = tokio::try_join!(
        sqlx::query_as::<_, RawIngestHealth>(
            "SELECT started_at,finished_at,status,files_seen,errors,CASE WHEN finished_at IS NULL THEN NULL ELSE CAST((julianday(finished_at)-julianday(started_at))*86400000 AS INTEGER) END duration_ms,failure_reason FROM ingest_runs ORDER BY id DESC LIMIT 1"
        )
        .fetch_optional(pool),
        sqlx::query_as::<_, HealthTotals>(
            "SELECT count(*) source_count,coalesce(sum(parse_status='ok'),0) ok_count,coalesce(sum(parse_status='partial'),0) partial_count,coalesce(sum(parse_status='error'),0) error_count FROM source_files"
        )
        .fetch_one(pool),
        sqlx::query_as::<_, RawSourceHealth>(
            "SELECT i.source_key,sf.path,sf.kind,sf.parse_status,sf.parse_error,sf.observed_at,CAST((julianday('now')-julianday(sf.observed_at))*86400 AS INTEGER) age_seconds,h.name hostname,u.os_user,a.kind agent_kind,s.stable_id FROM source_files sf JOIN agent_instances i ON i.id=sf.instance_id JOIN agents a ON a.id=i.agent_id JOIN users u ON u.id=i.user_id JOIN hosts h ON h.id=u.host_id LEFT JOIN sessions s ON s.source_file_id=sf.id ORDER BY CASE sf.parse_status WHEN 'error' THEN 0 WHEN 'partial' THEN 1 ELSE 2 END,sf.observed_at,sf.id LIMIT ? OFFSET ?"
        )
        .bind((page_size + 1) as i64)
        .bind(offset)
        .fetch_all(pool),
    )?;
    Ok(HealthSnapshot {
        latest_run: latest_run.map(present_run),
        totals,
        sources: rows.into_iter().map(present_source).collect(),
    })
}

fn present_run(run: RawIngestHealth) -> IngestHealth {
    IngestHealth {
        started_at: run.started_at,
        finished_at: run.finished_at,
        status: match run.status.as_str() {
            "complete" => "complete",
            "running" => "running",
            "failed" => "failed",
            _ => "unknown",
        },
        files_seen: run.files_seen,
        errors: run.errors,
        duration_ms: run.duration_ms,
        failure_reason: run.failure_reason.as_deref().map(|reason| match reason {
            "source_discovery" => "source_discovery",
            "source_layout_mismatch" => "source_layout_mismatch",
            "source_traversal" => "source_traversal",
            "ingest_failed" => "ingest_failed",
            _ => "ingest_failed",
        }),
    }
}

fn present_source(row: RawSourceHealth) -> SourceHealth {
    let parse_status = match row.parse_status.as_str() {
        "ok" => "ok",
        "partial" => "partial",
        "error" => "error",
        _ => "unknown",
    };
    let reason = (parse_status != "ok").then(|| classify_error(row.parse_error.as_deref()));
    let freshness = match row.age_seconds {
        Some(age) if age < -300 => "clock skew",
        Some(age) if age <= 900 => "recent",
        Some(_) => "stale",
        None => "unavailable",
    };
    SourceHealth {
        source_file_id: crate::identity::source_file_id(&row.source_key, &row.kind, &row.path),
        source_id: row.source_key,
        kind: row.kind,
        parse_status,
        reason,
        observed_at: row.observed_at,
        freshness,
        hostname: row.hostname,
        os_user: row.os_user,
        agent_kind: row.agent_kind,
        stable_id: row.stable_id,
    }
}

fn classify_error(detail: Option<&str>) -> &'static str {
    let detail = detail.unwrap_or_default().to_ascii_lowercase();
    if detail.contains("malformed") || detail.contains("partial") || detail.contains("utf-8") {
        "malformed_records"
    } else if detail.contains("duplicate") || detail.contains("conflict") {
        "duplicate_identity"
    } else if detail.contains("changed") || detail.contains("replaced") {
        "source_changed_during_read"
    } else if detail.contains("metadata") || detail.contains("session id") {
        "invalid_session_metadata"
    } else if detail.contains("read")
        || detail.contains("open")
        || detail.contains("inspect")
        || detail.contains("i/o")
        || detail.contains("io error")
    {
        "source_io"
    } else {
        "import_failed"
    }
}

pub(crate) async fn cli_report(pool: &SqlitePool) -> Result<String> {
    let mut snapshot = snapshot(pool, 0, SOURCE_PAGE_SIZE).await?;
    let has_more = snapshot.sources.len() > SOURCE_PAGE_SIZE;
    snapshot.sources.truncate(SOURCE_PAGE_SIZE);
    let mut output = String::new();
    if let Some(run) = snapshot.latest_run {
        output.push_str(&format!(
            "latest ingest: {}; started {}; finished {}; duration {}; files seen {}; errors {}{}\n",
            run.status,
            run.started_at,
            run.finished_at.as_deref().unwrap_or("unfinished"),
            run.duration_ms
                .map(|value| format!("{value} ms"))
                .unwrap_or_else(|| "unavailable".to_owned()),
            run.files_seen,
            run.errors,
            run.failure_reason
                .map(|reason| format!("; reason {reason}"))
                .unwrap_or_default()
        ));
    } else {
        output.push_str("latest ingest: unavailable\n");
    }
    output.push_str(&format!(
        "sources: {}; ok {}; partial {}; error {}\n",
        snapshot.totals.source_count,
        snapshot.totals.ok_count,
        snapshot.totals.partial_count,
        snapshot.totals.error_count
    ));
    for source in snapshot.sources {
        output.push_str(&format!(
            "{} {} {} {}{}\n",
            source.source_file_id,
            source.agent_kind,
            source.parse_status,
            source.freshness,
            source
                .reason
                .map(|reason| format!(" reason={reason}"))
                .unwrap_or_default()
        ));
    }
    if has_more {
        output.push_str("additional sources omitted; use the paged health view\n");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_errors_map_to_a_fixed_public_vocabulary() {
        assert_eq!(
            classify_error(Some("2 malformed lines")),
            "malformed_records"
        );
        assert_eq!(
            classify_error(Some("duplicate session conflict")),
            "duplicate_identity"
        );
        assert_eq!(
            classify_error(Some("file changed while parsing")),
            "source_changed_during_read"
        );
        assert_eq!(
            classify_error(Some("missing session id")),
            "invalid_session_metadata"
        );
        assert_eq!(
            classify_error(Some("failed to open private/path")),
            "source_io"
        );
        assert_eq!(classify_error(Some("PRIVATE SECRET")), "import_failed");
        let run = present_run(RawIngestHealth {
            started_at: "synthetic".into(),
            finished_at: None,
            status: "PRIVATE STATUS".into(),
            files_seen: 0,
            errors: 1,
            duration_ms: None,
            failure_reason: Some("PRIVATE FAILURE".into()),
        });
        assert_eq!(run.status, "unknown");
        assert_eq!(run.failure_reason, Some("ingest_failed"));
    }
}
