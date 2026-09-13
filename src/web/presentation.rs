use super::queries::{
    FilterValues, RecentUsageRow, SessionFilters, TimelineRow, UsageGroupRow, UsageTotalsRow,
};

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

pub(super) fn present_timeline(
    rows: Vec<TimelineRow>,
    source_kind: &str,
    stable_id: &str,
    mut conversation_started: bool,
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
                html: super::markdown::render(&row.content),
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
