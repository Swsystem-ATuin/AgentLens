use super::queries::{
    DiagnosticEvidenceRow, DiagnosticTotalsRow, FilterValues, PressureRow, RecentUsageRow,
    SessionFilters, TimelineRow, TurnRow, UsageGroupRow, UsageTotalsRow,
};
use std::collections::HashMap;

pub(super) struct TimelineContent {
    pub(super) html: String,
    pub(super) anchor: String,
    pub(super) is_primary: bool,
    pub(super) continuation_href: Option<String>,
}

pub(super) struct TimelineItem {
    pub(super) event_type: String,
    pub(super) anchor: String,
    pub(super) order_label: String,
    pub(super) timestamp: Option<String>,
    pub(super) role: String,
    pub(super) title: String,
    pub(super) state: Option<String>,
    pub(super) call_id: Option<String>,
    pub(super) initially_open: bool,
    pub(super) contents: Vec<TimelineContent>,
}

#[cfg(test)]
pub(super) fn present_timeline(
    rows: Vec<TimelineRow>,
    source_kind: &str,
    stable_id: &str,
    conversation_started: bool,
) -> Vec<TimelineItem> {
    present_timeline_with_aliases(
        rows,
        source_kind,
        stable_id,
        conversation_started,
        &HashMap::new(),
    )
}

pub(super) fn present_timeline_with_aliases(
    rows: Vec<TimelineRow>,
    source_kind: &str,
    stable_id: &str,
    mut conversation_started: bool,
    generated_image_aliases: &HashMap<String, String>,
) -> Vec<TimelineItem> {
    let mut items: Vec<TimelineItem> = Vec::new();
    for row in rows {
        if row.event_type == "message" {
            let role = row.role.as_deref().unwrap_or("unknown");
            let presentation = crate::sources::classify_message(
                source_kind,
                role,
                &row.content,
                !conversation_started,
            );
            if !conversation_started
                && role == "user"
                && presentation
                    .as_ref()
                    .is_none_or(|presentation| presentation.initially_open)
            {
                conversation_started = true;
            }
            let anchor = format!("event-{}-message", row.sequence);
            let mut content = TimelineContent {
                html: if source_kind == crate::sources::codex::KIND {
                    super::markdown::render_with_generated_images(
                        &row.content,
                        stable_id,
                        generated_image_aliases,
                    )
                } else {
                    super::markdown::render(&row.content)
                },
                anchor: anchor.clone(),
                is_primary: true,
                continuation_href: (row.total_chars > super::queries::MESSAGE_PREVIEW_CHARS)
                    .then(|| format!("/sessions/{stable_id}/messages/{}", row.sequence)),
            };
            if role == "assistant"
                && presentation.is_none()
                && let Some(previous) = items.last_mut()
                && previous.event_type == "message"
                && previous.role == "assistant"
                && previous.initially_open
            {
                content.is_primary = false;
                previous.contents.push(content);
                continue;
            }
            items.push(TimelineItem {
                event_type: "message".into(),
                anchor,
                order_label: format!("#{}", row.sequence),
                timestamp: row.timestamp,
                role: role.to_owned(),
                title: presentation
                    .as_ref()
                    .map(|presentation| presentation.title.to_owned())
                    .unwrap_or_else(|| role.to_owned()),
                state: None,
                call_id: None,
                initially_open: presentation
                    .as_ref()
                    .is_none_or(|presentation| presentation.initially_open),
                contents: vec![content],
            });
            continue;
        }

        if row.event_type == "diagnostic_event" {
            let title = if row.tool_name.as_deref() == Some("pressure") {
                "Context and rate observation"
            } else {
                "Accounting observation"
            };
            let anchor = format!("event-{}-diagnostics", row.sequence);
            items.push(TimelineItem {
                event_type: row.event_type,
                anchor: anchor.clone(),
                order_label: format!("#{}", row.sequence),
                timestamp: row.timestamp,
                role: "diagnostic".into(),
                title: title.into(),
                state: row.outcome,
                call_id: None,
                initially_open: true,
                contents: vec![TimelineContent {
                    html: super::markdown::render_plain(if row.content.is_empty() {
                        "No context, rate-limit, or accounting values were available."
                    } else {
                        &row.content
                    }),
                    anchor,
                    is_primary: true,
                    continuation_href: None,
                }],
            });
            continue;
        }

        if row.event_type == "turn_event" {
            let kind = row.tool_name.as_deref().unwrap_or("unknown");
            let title = match kind {
                "started" => "Turn started",
                "requested_model" if row.outcome.as_deref() == Some("requested model changed") => {
                    "Requested model changed"
                }
                "requested_model" => "Requested model",
                "completed" => "Turn completed",
                "interrupted" => "Turn interrupted",
                "user_activity" if row.outcome.as_deref() == Some("resumed") => "Turn resumed",
                "user_activity" => "User turn activity",
                "served_model_fallback" => "Observed served-model fallback",
                _ => "Turn evidence",
            };
            let anchor = format!("event-{}-turns", row.sequence);
            items.push(TimelineItem {
                event_type: row.event_type,
                anchor: anchor.clone(),
                order_label: format!("#{}", row.sequence),
                timestamp: row.timestamp,
                role: "turn".into(),
                title: title.into(),
                state: row.outcome,
                call_id: row.call_id,
                initially_open: matches!(kind, "interrupted" | "served_model_fallback"),
                contents: vec![TimelineContent {
                    html: super::markdown::render_plain(if row.content.is_empty() {
                        "Boundary evidence only."
                    } else {
                        &row.content
                    }),
                    anchor,
                    is_primary: true,
                    continuation_href: None,
                }],
            });
            continue;
        }

        let is_request = matches!(row.event_type.as_str(), "tool_call" | "question");
        let is_question = matches!(row.event_type.as_str(), "question" | "answer");
        let tool_name = row.tool_name.as_deref().unwrap_or("unknown tool");
        let title = match row.event_type.as_str() {
            "question" => "Question".to_owned(),
            "answer" => "Answer".to_owned(),
            "tool_result" if row.tool_name.is_none() => "Orphan tool result".to_owned(),
            "tool_result" => format!("Tool result: {tool_name}"),
            _ => format!("Tool request: {tool_name}"),
        };
        let state = if is_request {
            Some(format!(
                "requested · {}",
                match row.outcome.as_deref() {
                    Some("failed") => "failed",
                    Some("output observed") => "output observed",
                    _ => "outcome unknown",
                }
            ))
        } else {
            Some(
                match row.outcome.as_deref() {
                    Some("failed") => "failed",
                    _ => "output observed",
                }
                .to_owned(),
            )
        };
        let detail_kind = if is_request { "tools" } else { "results" };
        let empty = if is_request {
            "No arguments recorded."
        } else {
            "No output body recorded."
        };
        let anchor = format!("event-{}-{detail_kind}", row.sequence);
        items.push(TimelineItem {
            event_type: row.event_type.clone(),
            anchor: anchor.clone(),
            order_label: if row.sequence_known {
                format!("#{}", row.sequence)
            } else {
                format!("near #{} · exact legacy order unavailable", row.sequence)
            },
            timestamp: row.timestamp,
            role: if is_question { "user-input" } else { "tool" }.into(),
            title,
            state,
            call_id: row.call_id,
            initially_open: is_question,
            contents: vec![TimelineContent {
                html: super::markdown::render_plain(if row.content.is_empty() {
                    empty
                } else {
                    &row.content
                }),
                anchor,
                is_primary: true,
                continuation_href: (row.total_chars > super::queries::MESSAGE_PREVIEW_CHARS)
                    .then(|| format!("/sessions/{stable_id}/{detail_kind}/{}", row.sequence)),
            }],
        });
    }
    items
}

pub(super) struct FilterOption {
    pub(super) value: String,
    pub(super) selected: bool,
}

pub(super) struct FilterOptions {
    pub(super) hosts: Vec<FilterOption>,
    pub(super) users: Vec<FilterOption>,
    pub(super) models: Vec<FilterOption>,
    pub(super) statuses: Vec<FilterOption>,
    pub(super) agents: Vec<FilterOption>,
    pub(super) projects: Vec<FilterOption>,
    pub(super) archives: Vec<FilterOption>,
}

impl FilterOptions {
    pub(super) fn new(values: FilterValues, selected: &SessionFilters<'_>) -> Self {
        Self {
            hosts: select_options(values.hosts, selected.host),
            users: select_options(values.users, selected.user),
            models: select_options(values.models, selected.model),
            statuses: select_options(values.statuses, selected.status),
            agents: select_options(values.agents, selected.agent),
            projects: select_options(values.projects, selected.project),
            archives: select_options(values.archives, selected.archive),
        }
    }
}

pub(super) fn select_options(values: Vec<String>, selected: Option<&str>) -> Vec<FilterOption> {
    values
        .into_iter()
        .map(|value| FilterOption {
            selected: selected == Some(value.as_str()),
            value,
        })
        .collect()
}

pub(super) struct Pagination {
    pub(super) page: u32,
    pub(super) previous_href: Option<String>,
    pub(super) next_href: Option<String>,
}

pub(super) struct UsageTotalsView {
    pub(super) input_tokens: String,
    pub(super) output_tokens: String,
    pub(super) cached_input_tokens: String,
    pub(super) reasoning_output_tokens: String,
    pub(super) total_tokens: String,
    pub(super) cache_ratio: String,
    pub(super) session_count: i64,
    pub(super) observation_count: i64,
    pub(super) unattributed_count: i64,
    pub(super) reset_count: i64,
    pub(super) duplicate_count: i64,
    pub(super) missing_count: i64,
    pub(super) inconsistent_count: i64,
    pub(super) latest_at: String,
}

pub(super) struct UsageGroupView {
    pub(super) period: String,
    pub(super) dimension: String,
    pub(super) session_href: Option<String>,
    pub(super) input_tokens: String,
    pub(super) output_tokens: String,
    pub(super) cached_input_tokens: String,
    pub(super) reasoning_output_tokens: String,
    pub(super) total_tokens: String,
    pub(super) cache_ratio: String,
    pub(super) session_count: i64,
    pub(super) observation_count: i64,
    pub(super) anomaly_count: i64,
}

pub(super) struct RecentUsageView {
    pub(super) timestamp: String,
    pub(super) session_href: String,
    pub(super) title: String,
    pub(super) model: String,
    pub(super) input_tokens: String,
    pub(super) output_tokens: String,
    pub(super) cached_input_tokens: String,
    pub(super) reasoning_output_tokens: String,
    pub(super) total_tokens: String,
    pub(super) anomalous: bool,
}

pub(super) struct DiagnosticTotalsView {
    pub(super) interruption_count: i64,
    pub(super) incomplete_count: i64,
    pub(super) accounting_anomaly_count: i64,
    pub(super) observed_failure_count: i64,
}

impl From<DiagnosticTotalsRow> for DiagnosticTotalsView {
    fn from(row: DiagnosticTotalsRow) -> Self {
        Self {
            interruption_count: row.interruption_count,
            incomplete_count: row.incomplete_count,
            accounting_anomaly_count: row.accounting_anomaly_count,
            observed_failure_count: row.observed_failure_count,
        }
    }
}

pub(super) struct PressureView {
    pub(super) timestamp: String,
    pub(super) title: String,
    pub(super) agent_kind: String,
    pub(super) hostname: String,
    pub(super) os_user: String,
    pub(super) context: String,
    pub(super) context_state: &'static str,
    pub(super) scope: String,
    pub(super) primary: String,
    pub(super) primary_state: &'static str,
    pub(super) secondary: String,
    pub(super) secondary_state: &'static str,
    pub(super) reached: String,
    pub(super) evidence_href: String,
}

pub(super) struct DiagnosticEvidenceView {
    pub(super) timestamp: String,
    pub(super) title: String,
    pub(super) href: String,
}

pub(super) fn present_pressure(rows: Vec<PressureRow>) -> Vec<PressureView> {
    rows.into_iter()
        .map(|row| {
            let context = match (row.context_tokens, row.context_window) {
                (Some(tokens), Some(window)) if window > 0 => format!(
                    "{} / {} ({:.1}%)",
                    format_tokens(Some(tokens)),
                    format_tokens(Some(window)),
                    tokens as f64 * 100.0 / window as f64
                ),
                (None, Some(window)) => {
                    format!("Window {}; usage unavailable", format_tokens(Some(window)))
                }
                (Some(tokens), None) => {
                    format!("Used {}; window unavailable", format_tokens(Some(tokens)))
                }
                _ => "Unavailable".into(),
            };
            let context_state = match (row.context_tokens, row.context_window) {
                (None, None) => "unavailable",
                (Some(tokens), _) if tokens < 0 => "inconsistent observation",
                (_, Some(window)) if window <= 0 => "inconsistent observation",
                (Some(tokens), Some(window)) if tokens > window => "inconsistent observation",
                _ => freshness(row.observation_age_seconds),
            };
            let primary_state = rate_freshness(
                row.primary_used_percent,
                row.observation_age_seconds,
                row.primary_reset_remaining_seconds,
            );
            let secondary_state = rate_freshness(
                row.secondary_used_percent,
                row.observation_age_seconds,
                row.secondary_reset_remaining_seconds,
            );
            let position = row.evidence_position;
            let sequence = row.source_sequence;
            let mut scope = row
                .rate_limit_name
                .or(row.rate_limit_id)
                .unwrap_or_else(|| "Unspecified limit".into());
            if let Some(plan_type) = row.plan_type {
                scope.push_str(" · plan ");
                scope.push_str(&plan_type);
            }
            PressureView {
                timestamp: row.timestamp.unwrap_or_else(|| "Unavailable".into()),
                title: row.title,
                agent_kind: row.agent_kind,
                hostname: row.hostname,
                os_user: row.os_user,
                context,
                context_state,
                scope,
                primary: format_rate_window(
                    row.primary_used_percent,
                    row.primary_window_minutes,
                    row.primary_resets_at,
                    row.primary_resets_in_seconds,
                ),
                primary_state,
                secondary: format_rate_window(
                    row.secondary_used_percent,
                    row.secondary_window_minutes,
                    row.secondary_resets_at,
                    row.secondary_resets_in_seconds,
                ),
                secondary_state,
                reached: row
                    .rate_limit_reached_type
                    .unwrap_or_else(|| "Not reported".into()),
                evidence_href: diagnostic_event_href(&row.stable_id, sequence, position),
            }
        })
        .collect()
}

pub(super) fn present_diagnostic_evidence(
    rows: Vec<DiagnosticEvidenceRow>,
    kind: &str,
) -> Vec<DiagnosticEvidenceView> {
    rows.into_iter()
        .filter_map(|row| {
            let href = match kind {
                "incomplete" => {
                    let ordinal = row.ordinal?;
                    let page = row.position / super::queries::TURN_PAGE_SIZE as i64 + 1;
                    format!(
                        "/sessions/{}/turns?page={page}#turn-{ordinal}",
                        row.stable_id
                    )
                }
                "interruption" => turn_event_href(&row.stable_id, row.sequence?, row.position),
                "accounting" => diagnostic_event_href(&row.stable_id, row.sequence?, row.position),
                "failure" => {
                    let sequence = row.sequence?;
                    let section = row.position / super::queries::TRANSCRIPT_SECTION_SIZE as i64 + 1;
                    filtered_transcript_href(
                        &row.stable_id,
                        section as u32,
                        "tools",
                        Some(&format!("event-{sequence}-result")),
                    )
                }
                _ => return None,
            };
            Some(DiagnosticEvidenceView {
                timestamp: row
                    .timestamp
                    .unwrap_or_else(|| "Timestamp unavailable".into()),
                title: row.title,
                href,
            })
        })
        .collect()
}

fn diagnostic_event_href(stable_id: &str, sequence: i64, position: i64) -> String {
    let section = position / super::queries::TRANSCRIPT_SECTION_SIZE as i64 + 1;
    filtered_transcript_href(
        stable_id,
        section as u32,
        "diagnostics",
        Some(&format!("event-{sequence}-diagnostics")),
    )
}

fn freshness(age_seconds: Option<i64>) -> &'static str {
    match age_seconds {
        Some(0..=900) => "recent observation",
        Some(_) => "stale observation",
        None => "timestamp unavailable",
    }
}

fn rate_freshness(
    used_percent: Option<f64>,
    age_seconds: Option<i64>,
    reset_remaining_seconds: Option<i64>,
) -> &'static str {
    if used_percent.is_none() {
        "unavailable"
    } else if used_percent.is_some_and(|percent| !(0.0..=100.0).contains(&percent)) {
        "inconsistent observation"
    } else if reset_remaining_seconds.is_some_and(|remaining| remaining <= 0) {
        "expired observation"
    } else {
        freshness(age_seconds)
    }
}

fn format_rate_window(
    used_percent: Option<f64>,
    window_minutes: Option<i64>,
    resets_at: Option<i64>,
    resets_in_seconds: Option<i64>,
) -> String {
    let Some(used_percent) = used_percent else {
        return "Unavailable".into();
    };
    let window = window_minutes.map_or_else(
        || "window unavailable".into(),
        |minutes| format!("{minutes} minute window"),
    );
    let reset = resets_at.map_or_else(
        || {
            resets_in_seconds.map_or_else(
                || "reset unavailable".into(),
                |seconds| format!("reported reset in {seconds}s"),
            )
        },
        |timestamp| format!("reset Unix {timestamp}"),
    );
    format!("{used_percent:.1}% used · {window} · {reset}")
}

pub(super) struct TurnView {
    pub(super) ordinal: i64,
    pub(super) turn_id: String,
    pub(super) start_timestamp: String,
    pub(super) end_timestamp: String,
    pub(super) start_kind: String,
    pub(super) outcome: String,
    pub(super) resumed: bool,
    pub(super) requested_model: String,
    pub(super) observed_model: String,
    pub(super) duration: String,
    pub(super) tool_request_count: i64,
    pub(super) tool_result_count: i64,
    pub(super) failed_tool_count: i64,
    pub(super) unknown_tool_count: i64,
    pub(super) start_href: Option<String>,
    pub(super) end_href: Option<String>,
    pub(super) requested_model_href: Option<String>,
    pub(super) observed_model_href: Option<String>,
}

pub(super) fn present_turns(rows: Vec<TurnRow>, stable_id: &str) -> Vec<TurnView> {
    rows.into_iter()
        .map(|row| TurnView {
            ordinal: row.ordinal,
            turn_id: row.turn_id.unwrap_or_else(|| "Unavailable".into()),
            start_timestamp: row
                .start_timestamp
                .unwrap_or_else(|| "Opening evidence unavailable".into()),
            end_timestamp: row
                .end_timestamp
                .unwrap_or_else(|| "Terminal evidence unavailable".into()),
            start_kind: match row.start_kind.as_str() {
                "started" => "task started",
                "requested_model" => "turn context",
                "user_activity" => "user activity",
                _ => "unknown opening",
            }
            .into(),
            outcome: row.outcome,
            resumed: row.resumed,
            requested_model: row.requested_model.unwrap_or_else(|| "Unavailable".into()),
            observed_model: row.observed_model.unwrap_or_else(|| "Not observed".into()),
            duration: row
                .duration_seconds
                .map(format_duration)
                .unwrap_or_else(|| "Unavailable".into()),
            tool_request_count: row.tool_request_count,
            tool_result_count: row.tool_result_count,
            failed_tool_count: row.failed_tool_count,
            unknown_tool_count: row.unknown_tool_count,
            start_href: row
                .start_sequence
                .map(|sequence| turn_event_href(stable_id, sequence, row.start_position)),
            end_href: row
                .end_sequence
                .map(|sequence| turn_event_href(stable_id, sequence, row.end_position)),
            requested_model_href: row
                .requested_model_sequence
                .map(|sequence| turn_event_href(stable_id, sequence, row.requested_model_position)),
            observed_model_href: row
                .observed_model_sequence
                .map(|sequence| turn_event_href(stable_id, sequence, row.observed_model_position)),
        })
        .collect()
}

fn turn_event_href(stable_id: &str, sequence: i64, position: i64) -> String {
    let section = position / super::queries::TRANSCRIPT_SECTION_SIZE as i64 + 1;
    filtered_transcript_href(
        stable_id,
        section as u32,
        "turns",
        Some(&format!("event-{sequence}-turns")),
    )
}

fn format_duration(seconds: i64) -> String {
    let hours = seconds / 3_600;
    let minutes = seconds % 3_600 / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

pub(super) fn present_usage_totals(row: UsageTotalsRow) -> UsageTotalsView {
    UsageTotalsView {
        input_tokens: format_metric(
            row.input_tokens,
            row.input_available_count,
            row.observation_count,
        ),
        output_tokens: format_metric(
            row.output_tokens,
            row.output_available_count,
            row.observation_count,
        ),
        cached_input_tokens: format_metric(
            row.cached_input_tokens,
            row.cached_available_count,
            row.observation_count,
        ),
        reasoning_output_tokens: format_metric(
            row.reasoning_output_tokens,
            row.reasoning_available_count,
            row.observation_count,
        ),
        total_tokens: format_metric(
            row.total_tokens,
            row.total_available_count,
            row.observation_count,
        ),
        cache_ratio: cache_ratio(
            row.cached_input_tokens,
            row.input_tokens,
            row.cached_available_count == row.observation_count
                && row.input_available_count == row.observation_count,
        ),
        session_count: row.session_count,
        observation_count: row.observation_count,
        unattributed_count: row.unattributed_count,
        reset_count: row.reset_count,
        duplicate_count: row.duplicate_count,
        missing_count: row.missing_count,
        inconsistent_count: row.inconsistent_count,
        latest_at: row.latest_at.unwrap_or_else(|| "Unavailable".to_owned()),
    }
}

pub(super) fn present_usage_groups(rows: Vec<UsageGroupRow>) -> Vec<UsageGroupView> {
    rows.into_iter()
        .map(|row| UsageGroupView {
            period: row.period,
            dimension: row.dimension,
            session_href: row.stable_id.map(|id| format!("/sessions/{id}")),
            input_tokens: format_metric(
                row.input_tokens,
                row.input_available_count,
                row.observation_count,
            ),
            output_tokens: format_metric(
                row.output_tokens,
                row.output_available_count,
                row.observation_count,
            ),
            cached_input_tokens: format_metric(
                row.cached_input_tokens,
                row.cached_available_count,
                row.observation_count,
            ),
            reasoning_output_tokens: format_metric(
                row.reasoning_output_tokens,
                row.reasoning_available_count,
                row.observation_count,
            ),
            total_tokens: format_metric(
                row.total_tokens,
                row.total_available_count,
                row.observation_count,
            ),
            cache_ratio: cache_ratio(
                row.cached_input_tokens,
                row.input_tokens,
                row.cached_available_count == row.observation_count
                    && row.input_available_count == row.observation_count,
            ),
            session_count: row.session_count,
            observation_count: row.observation_count,
            anomaly_count: row.anomaly_count,
        })
        .collect()
}

pub(super) fn present_recent_usage(rows: Vec<RecentUsageRow>) -> Vec<RecentUsageView> {
    rows.into_iter()
        .map(|row| RecentUsageView {
            timestamp: row.timestamp,
            session_href: format!("/sessions/{}", row.stable_id),
            title: row.title,
            model: row.model,
            input_tokens: format_tokens(row.input_tokens),
            output_tokens: format_tokens(row.output_tokens),
            cached_input_tokens: format_tokens(row.cached_input_tokens),
            reasoning_output_tokens: format_tokens(row.reasoning_output_tokens),
            total_tokens: format_tokens(row.total_tokens),
            anomalous: row.anomalous,
        })
        .collect()
}

fn format_tokens(value: Option<i64>) -> String {
    let Some(value) = value else {
        return "Unavailable".to_owned();
    };
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(character);
    }
    formatted
}

fn format_metric(value: Option<i64>, available: i64, observations: i64) -> String {
    let formatted = format_tokens(value);
    if value.is_some() && available < observations {
        format!("{formatted} (partial)")
    } else {
        formatted
    }
}

fn cache_ratio(cached: Option<i64>, input: Option<i64>, complete: bool) -> String {
    match (cached, input) {
        (Some(cached), Some(input)) if input > 0 => {
            let ratio = format!("{:.1}%", cached as f64 * 100.0 / input as f64);
            if complete {
                ratio
            } else {
                format!("{ratio} (partial)")
            }
        }
        _ => "Unavailable".to_owned(),
    }
}

pub(super) fn usage_href(
    period: &str,
    group: &str,
    from: Option<&str>,
    to: Option<&str>,
    page: u32,
) -> String {
    let mut pairs = vec![
        format!("period={}", percent_encode(period)),
        format!("group={}", percent_encode(group)),
    ];
    for (name, value) in [("from", from), ("to", to)] {
        if let Some(value) = value {
            pairs.push(format!("{name}={}", percent_encode(value)));
        }
    }
    pairs.push(format!("page={page}"));
    format!("/usage?{}", pairs.join("&"))
}

pub(super) fn sessions_href(filters: &SessionFilters<'_>, page: u32) -> String {
    let mut pairs = Vec::new();
    for (name, value) in [
        ("q", filters.search),
        ("host", filters.host),
        ("user", filters.user),
        ("model", filters.model),
        ("status", filters.status),
        ("agent", filters.agent),
        ("project", filters.project),
        ("archive", filters.archive),
        ("from", filters.from),
        ("to", filters.to),
    ] {
        if let Some(value) = value {
            pairs.push(format!("{name}={}", percent_encode(value)));
        }
    }
    pairs.push(format!("page={page}"));
    format!("/sessions?{}", pairs.join("&"))
}

pub(super) fn filtered_transcript_href(
    stable_id: &str,
    section: u32,
    event_filter: &str,
    anchor: Option<&str>,
) -> String {
    let event = if event_filter == "all" {
        String::new()
    } else {
        format!("&event={}", percent_encode(event_filter))
    };
    let anchor = anchor.map_or_else(String::new, |anchor| format!("#{anchor}"));
    format!("/sessions/{stable_id}?section={section}{event}{anchor}")
}

pub(super) fn message_href(stable_id: &str, sequence: i64, part: u32) -> String {
    format!("/sessions/{stable_id}/messages/{sequence}?part={part}")
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
