use sqlx::{FromRow, SqlitePool};

#[derive(FromRow)]
struct CountRow {
    agent: String,
    value: String,
    count: i64,
}

#[derive(FromRow)]
struct SourceRow {
    agent: String,
    state: String,
    freshness: String,
    count: i64,
}

#[derive(FromRow)]
struct IngestRow {
    status: String,
    reason: String,
    count: i64,
}

#[derive(FromRow)]
struct LatestIngestRow {
    status: String,
    reason: String,
    duration_ms: Option<i64>,
    files_seen: i64,
    errors: i64,
    finished_timestamp: Option<i64>,
}

#[derive(FromRow)]
struct UsageRow {
    agent: String,
    model_family: String,
    session_count: i64,
    input_tokens: Option<i64>,
    input_available: i64,
    output_tokens: Option<i64>,
    output_available: i64,
    cached_tokens: Option<i64>,
    cached_available: i64,
    reasoning_tokens: Option<i64>,
    reasoning_available: i64,
    total_tokens: Option<i64>,
    total_available: i64,
}

pub(crate) async fn render(pool: &SqlitePool) -> Result<String, sqlx::Error> {
    let exclusion = crate::sources::model_exclusion();
    // All metric families must describe one committed SQLite snapshot. Using a
    // single read transaction prevents a concurrent refresh from splitting a
    // scrape across the old and new projections.
    let mut transaction = pool.begin().await?;
    let sessions = sqlx::query_as::<_, CountRow>(
        "SELECT CASE a.kind WHEN 'codex' THEN 'codex' WHEN 'gemini' THEN 'gemini' ELSE 'other' END agent,CASE s.status WHEN 'complete' THEN 'complete' WHEN 'aborted' THEN 'aborted' WHEN 'unknown' THEN 'unknown' ELSE 'other' END value,count(*) count FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE a.kind<>? OR lower(COALESCE(s.model,'')) NOT LIKE ? ESCAPE '\\' GROUP BY agent,value ORDER BY agent,value",
    )
    .bind(exclusion.source_kind)
    .bind(exclusion.pattern)
    .fetch_all(&mut *transaction)
    .await?;
    let sources = sqlx::query_as::<_, SourceRow>(
        "SELECT CASE a.kind WHEN 'codex' THEN 'codex' WHEN 'gemini' THEN 'gemini' ELSE 'other' END agent,CASE sf.parse_status WHEN 'ok' THEN 'ok' WHEN 'partial' THEN 'partial' WHEN 'error' THEN 'error' ELSE 'other' END state,CASE WHEN unixepoch(sf.observed_at) IS NULL THEN 'unavailable' WHEN unixepoch('now')-unixepoch(sf.observed_at)<-300 THEN 'clock_skew' WHEN unixepoch('now')-unixepoch(sf.observed_at)<=900 THEN 'recent' ELSE 'stale' END freshness,count(*) count FROM source_files sf JOIN agent_instances i ON i.id=sf.instance_id JOIN agents a ON a.id=i.agent_id GROUP BY agent,state,freshness ORDER BY agent,state,freshness",
    )
    .fetch_all(&mut *transaction)
    .await?;
    let tools = sqlx::query_as::<_, CountRow>(
        "WITH visible AS (SELECT s.id,CASE a.kind WHEN 'codex' THEN 'codex' WHEN 'gemini' THEN 'gemini' ELSE 'other' END agent FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE a.kind<>? OR lower(COALESCE(s.model,'')) NOT LIKE ? ESCAPE '\\'),outcomes AS (SELECT r.session_id,CASE r.outcome WHEN 'failed' THEN 'failed' WHEN 'observed' THEN 'observed' ELSE 'other' END outcome FROM tool_results r UNION ALL SELECT c.session_id,'unknown' FROM tool_calls c WHERE NOT EXISTS(SELECT 1 FROM tool_results r WHERE r.tool_call_id=c.id)) SELECT v.agent,o.outcome value,count(*) count FROM outcomes o JOIN visible v ON v.id=o.session_id GROUP BY v.agent,o.outcome ORDER BY v.agent,o.outcome",
    )
    .bind(exclusion.source_kind)
    .bind(exclusion.pattern)
    .fetch_all(&mut *transaction)
    .await?;
    let turns = sqlx::query_as::<_, CountRow>(
        "SELECT CASE a.kind WHEN 'codex' THEN 'codex' WHEN 'gemini' THEN 'gemini' ELSE 'other' END agent,CASE t.outcome WHEN 'complete' THEN 'complete' WHEN 'interrupted' THEN 'interrupted' WHEN 'unclosed' THEN 'unclosed' ELSE 'other' END value,count(*) count FROM turns t JOIN sessions s ON s.id=t.session_id JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE a.kind<>? OR lower(COALESCE(s.model,'')) NOT LIKE ? ESCAPE '\\' GROUP BY agent,value ORDER BY agent,value",
    )
    .bind(exclusion.source_kind)
    .bind(exclusion.pattern)
    .fetch_all(&mut *transaction)
    .await?;
    let ingest_runs = sqlx::query_as::<_, IngestRow>(
        "SELECT CASE status WHEN 'complete' THEN 'complete' WHEN 'running' THEN 'running' WHEN 'failed' THEN 'failed' ELSE 'unknown' END status,CASE failure_reason WHEN 'source_discovery' THEN 'source_discovery' WHEN 'source_layout_mismatch' THEN 'source_layout_mismatch' WHEN 'source_traversal' THEN 'source_traversal' WHEN 'ingest_failed' THEN 'ingest_failed' ELSE 'none' END reason,count(*) count FROM ingest_runs GROUP BY status,reason ORDER BY status,reason",
    )
    .fetch_all(&mut *transaction)
    .await?;
    let latest_ingest = sqlx::query_as::<_, LatestIngestRow>(
        "SELECT CASE status WHEN 'complete' THEN 'complete' WHEN 'running' THEN 'running' WHEN 'failed' THEN 'failed' ELSE 'unknown' END status,CASE failure_reason WHEN 'source_discovery' THEN 'source_discovery' WHEN 'source_layout_mismatch' THEN 'source_layout_mismatch' WHEN 'source_traversal' THEN 'source_traversal' WHEN 'ingest_failed' THEN 'ingest_failed' ELSE 'none' END reason,CASE WHEN finished_at IS NULL THEN NULL ELSE CAST((julianday(finished_at)-julianday(started_at))*86400000 AS INTEGER) END duration_ms,files_seen,errors,unixepoch(finished_at) finished_timestamp FROM ingest_runs ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(&mut *transaction)
    .await?;
    let usage = sqlx::query_as::<_, UsageRow>(
        "WITH visible AS (SELECT s.id,CASE a.kind WHEN 'codex' THEN 'codex' WHEN 'gemini' THEN 'gemini' ELSE 'other' END agent,CASE WHEN NULLIF(s.model,'') IS NULL THEN 'unknown' WHEN a.kind='gemini' AND lower(s.model) LIKE 'gemini-%' THEN 'gemini' WHEN a.kind='codex' AND lower(s.model) LIKE 'gpt-%' THEN 'gpt' WHEN a.kind='codex' AND lower(s.model) GLOB 'o[134]-*' THEN 'o-series' WHEN a.kind='codex' AND lower(s.model) LIKE 'codex-%' THEN 'codex' ELSE 'other' END model_family FROM sessions s JOIN agent_instances i ON i.id=s.instance_id JOIN agents a ON a.id=i.agent_id WHERE a.kind<>? OR lower(COALESCE(s.model,'')) NOT LIKE ? ESCAPE '\\') SELECT v.agent,v.model_family,count(*) session_count,sum(su.input_tokens) input_tokens,sum(su.input_tokens IS NOT NULL) input_available,sum(su.output_tokens) output_tokens,sum(su.output_tokens IS NOT NULL) output_available,sum(su.cached_input_tokens) cached_tokens,sum(su.cached_input_tokens IS NOT NULL) cached_available,sum(su.reasoning_output_tokens) reasoning_tokens,sum(su.reasoning_output_tokens IS NOT NULL) reasoning_available,sum(su.total_tokens) total_tokens,sum(su.total_tokens IS NOT NULL) total_available FROM visible v JOIN session_usage su ON su.session_id=v.id GROUP BY v.agent,v.model_family ORDER BY v.agent,v.model_family",
    )
    .bind(exclusion.source_kind)
    .bind(exclusion.pattern)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.rollback().await?;

    let mut output = String::with_capacity(16 * 1024);
    metric_header(
        &mut output,
        "agentlens_indexed_sessions",
        "Visible sessions in the current disposable index.",
    );
    for row in sessions {
        sample(
            &mut output,
            "agentlens_indexed_sessions",
            &[("agent", &row.agent), ("status", &row.value)],
            row.count,
        );
    }

    metric_header(
        &mut output,
        "agentlens_indexed_source_files",
        "Source files in the current disposable index by projection and observation state.",
    );
    for row in sources {
        sample(
            &mut output,
            "agentlens_indexed_source_files",
            &[
                ("agent", &row.agent),
                ("state", &row.state),
                ("freshness", &row.freshness),
            ],
            row.count,
        );
    }

    render_counts(
        &mut output,
        "agentlens_indexed_tool_outcomes",
        "Observed tool result and missing-result counts in the current disposable index.",
        "outcome",
        tools,
    );
    render_counts(
        &mut output,
        "agentlens_indexed_turn_outcomes",
        "Turn outcome counts in the current disposable index.",
        "outcome",
        turns,
    );

    metric_header(
        &mut output,
        "agentlens_ingest_runs",
        "Ingest run records retained in the current disposable index.",
    );
    for row in ingest_runs {
        sample(
            &mut output,
            "agentlens_ingest_runs",
            &[("status", &row.status), ("reason", &row.reason)],
            row.count,
        );
    }
    metric_header(
        &mut output,
        "agentlens_last_ingest_info",
        "Identity of the latest ingest state in the current disposable index.",
    );
    if let Some(latest) = latest_ingest.as_ref() {
        sample(
            &mut output,
            "agentlens_last_ingest_info",
            &[("status", &latest.status), ("reason", &latest.reason)],
            1,
        );
    }
    scalar(
        &mut output,
        "agentlens_last_ingest_duration_seconds",
        "Duration of the latest finished ingest in seconds.",
        latest_ingest
            .as_ref()
            .and_then(|latest| latest.duration_ms)
            .map(|value| value as f64 / 1_000.0),
    );
    scalar(
        &mut output,
        "agentlens_last_ingest_files",
        "Source files seen by the latest ingest.",
        latest_ingest
            .as_ref()
            .map(|latest| latest.files_seen as f64),
    );
    scalar(
        &mut output,
        "agentlens_last_ingest_errors",
        "File errors recorded by the latest ingest.",
        latest_ingest.as_ref().map(|latest| latest.errors as f64),
    );
    scalar(
        &mut output,
        "agentlens_last_ingest_finished_timestamp_seconds",
        "Unix timestamp when the latest ingest finished.",
        latest_ingest
            .as_ref()
            .and_then(|latest| latest.finished_timestamp)
            .map(|value| value as f64),
    );

    metric_header(
        &mut output,
        "agentlens_indexed_usage_tokens",
        "Known cumulative token totals in the current disposable index.",
    );
    for row in &usage {
        for (kind, tokens) in [
            ("input", row.input_tokens),
            ("output", row.output_tokens),
            ("cached_input", row.cached_tokens),
            ("reasoning_output", row.reasoning_tokens),
            ("total", row.total_tokens),
        ] {
            let labels = [
                ("agent", row.agent.as_str()),
                ("model_family", row.model_family.as_str()),
                ("kind", kind),
            ];
            if let Some(tokens) = tokens {
                sample(
                    &mut output,
                    "agentlens_indexed_usage_tokens",
                    &labels,
                    tokens,
                );
            }
        }
    }
    metric_header(
        &mut output,
        "agentlens_indexed_usage_sessions",
        "Sessions with available or unavailable cumulative token totals.",
    );
    for row in usage {
        for (kind, available) in [
            ("input", row.input_available),
            ("output", row.output_available),
            ("cached_input", row.cached_available),
            ("reasoning_output", row.reasoning_available),
            ("total", row.total_available),
        ] {
            let labels = [
                ("agent", row.agent.as_str()),
                ("model_family", row.model_family.as_str()),
                ("kind", kind),
            ];
            sample(
                &mut output,
                "agentlens_indexed_usage_sessions",
                &[
                    labels[0],
                    labels[1],
                    labels[2],
                    ("availability", "available"),
                ],
                available,
            );
            sample(
                &mut output,
                "agentlens_indexed_usage_sessions",
                &[
                    labels[0],
                    labels[1],
                    labels[2],
                    ("availability", "unavailable"),
                ],
                row.session_count - available,
            );
        }
    }
    Ok(output)
}

fn render_counts(
    output: &mut String,
    name: &str,
    help: &str,
    dimension: &str,
    rows: Vec<CountRow>,
) {
    metric_header(output, name, help);
    for row in rows {
        sample(
            output,
            name,
            &[("agent", &row.agent), (dimension, &row.value)],
            row.count,
        );
    }
}

fn metric_header(output: &mut String, name: &str, help: &str) {
    output.push_str("# HELP ");
    output.push_str(name);
    output.push(' ');
    output.push_str(help);
    output.push('\n');
    output.push_str("# TYPE ");
    output.push_str(name);
    output.push_str(" gauge\n");
}

fn scalar(output: &mut String, name: &str, help: &str, value: Option<f64>) {
    metric_header(output, name, help);
    if let Some(value) = value {
        output.push_str(name);
        output.push(' ');
        output.push_str(&value.to_string());
        output.push('\n');
    }
}

fn sample(output: &mut String, name: &str, labels: &[(&str, &str)], value: i64) {
    output.push_str(name);
    output.push('{');
    for (index, (key, value)) in labels.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        output.push_str(key);
        output.push_str("=\"");
        escape_label(output, value);
        output.push('"');
    }
    output.push_str("} ");
    output.push_str(&value.to_string());
    output.push('\n');
}

fn escape_label(output: &mut String, value: &str) {
    for character in value.chars() {
        match character {
            '\\' => output.push_str("\\\\"),
            '"' => output.push_str("\\\""),
            '\n' => output.push_str("\\n"),
            _ => output.push(character),
        }
    }
}
