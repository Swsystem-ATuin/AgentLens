use serde::Serialize;

use crate::web::queries::PostmortemRow;

#[derive(Serialize)]
pub(crate) struct PostmortemReport {
    format_version: u8,
    redaction: Redaction,
    session: SessionFacts,
    turns: TurnFacts,
    usage: UsageFacts,
    models: ModelFacts,
    tools: ToolFacts,
    pressure: PressureFacts,
    evidence_gaps: EvidenceGaps,
    reason_codes: Vec<ReasonCode>,
}

#[derive(Serialize)]
struct Redaction {
    profile: &'static str,
    excluded: [&'static str; 9],
}

#[derive(Serialize)]
struct SessionFacts {
    stable_id: String,
    agent_kind: String,
    indexed_status: String,
    source_projection_status: String,
}

#[derive(Serialize)]
struct TurnFacts {
    observed: i64,
    completed: i64,
    interrupted: i64,
    unclosed: i64,
    terminal_only: i64,
    resumed: i64,
}

#[derive(Serialize)]
struct UsageFacts {
    input_tokens: Option<i64>,
    cached_input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    reasoning_output_tokens: Option<i64>,
    reported_total_tokens: Option<i64>,
    observations: i64,
    counter_resets: i64,
    missing_observations: i64,
    inconsistent_observations: i64,
}

#[derive(Serialize)]
struct ModelFacts {
    requested_model_observations: i64,
    requested_model_changes: i64,
    observed_served_model_fallbacks: i64,
}

#[derive(Serialize)]
struct ToolFacts {
    requests: i64,
    results_observed: i64,
    failures_observed: i64,
    outcomes_unknown: i64,
}

#[derive(Serialize)]
struct PressureFacts {
    context_observations: i64,
    rate_limit_observations: i64,
}

#[derive(Serialize)]
struct EvidenceGaps {
    context_observations_missing_values: i64,
    rate_limit_observations_missing_values: i64,
    operational_events_missing_valid_timestamps: i64,
}

#[derive(Serialize)]
struct ReasonCode {
    code: &'static str,
    count: i64,
    explanation: &'static str,
}

impl PostmortemReport {
    pub(crate) fn from_row(row: PostmortemRow) -> Self {
        let mut reason_codes = Vec::with_capacity(14);
        push_reason(
            &mut reason_codes,
            "turn_interrupted",
            row.interrupted_turn_count,
            "The source recorded interruption; this does not establish task failure.",
        );
        push_reason(
            &mut reason_codes,
            "turn_unclosed",
            row.unclosed_turn_count,
            "An opening lacked a later terminal boundary; this is not proof of current activity.",
        );
        push_reason(
            &mut reason_codes,
            "turn_opening_missing",
            row.terminal_only_turn_count,
            "A terminal boundary was retained without observed opening evidence.",
        );
        push_reason(
            &mut reason_codes,
            "requested_model_changed",
            row.requested_model_change_count,
            "Structured requested-model evidence changed within a turn.",
        );
        push_reason(
            &mut reason_codes,
            "served_model_fallback_observed",
            row.served_model_fallback_count,
            "The source explicitly recorded a served-model fallback.",
        );
        push_reason(
            &mut reason_codes,
            "usage_counter_reset",
            row.usage_reset_count,
            "A cumulative usage counter decreased and restarted delta accounting.",
        );
        push_reason(
            &mut reason_codes,
            "usage_values_missing",
            row.usage_missing_count,
            "A usage observation omitted one or more required cumulative values.",
        );
        push_reason(
            &mut reason_codes,
            "usage_values_inconsistent",
            row.usage_inconsistent_count,
            "Reported token totals or subset relationships were internally inconsistent.",
        );
        push_reason(
            &mut reason_codes,
            "tool_failure_observed",
            row.observed_tool_failure_count,
            "A tool result carried an explicit failure marker.",
        );
        push_reason(
            &mut reason_codes,
            "tool_outcome_unknown",
            row.unknown_tool_outcome_count,
            "A tool request had no paired observed result.",
        );
        push_reason(
            &mut reason_codes,
            "context_evidence_missing",
            row.context_gap_count,
            "A usage event lacked a complete context-used/window pair.",
        );
        push_reason(
            &mut reason_codes,
            "rate_evidence_missing",
            row.rate_gap_count,
            "A usage event contained neither primary nor secondary rate percentage.",
        );
        push_reason(
            &mut reason_codes,
            "timestamp_evidence_missing",
            row.timestamp_gap_count,
            "An operational event lacked a valid timestamp.",
        );
        if row.source_status != "ok" {
            push_reason(
                &mut reason_codes,
                "source_projection_not_complete",
                1,
                "The indexed source projection was not complete or its status was unavailable.",
            );
        }

        Self {
            format_version: 1,
            redaction: Redaction {
                profile: "aggregate-only",
                excluded: [
                    "prompts and messages",
                    "source code",
                    "tool arguments and outputs",
                    "raw source records",
                    "titles and native session IDs",
                    "filesystem paths and projects",
                    "host and operating-system user names",
                    "credentials and account identifiers",
                    "user annotations",
                ],
            },
            session: SessionFacts {
                stable_id: row.stable_id,
                agent_kind: fixed_agent_kind(&row.agent_kind).into(),
                indexed_status: fixed_session_status(&row.session_status).into(),
                source_projection_status: fixed_source_status(&row.source_status).into(),
            },
            turns: TurnFacts {
                observed: row.turn_count,
                completed: row.completed_turn_count,
                interrupted: row.interrupted_turn_count,
                unclosed: row.unclosed_turn_count,
                terminal_only: row.terminal_only_turn_count,
                resumed: row.resumed_turn_count,
            },
            usage: UsageFacts {
                input_tokens: row.input_tokens,
                cached_input_tokens: row.cached_input_tokens,
                output_tokens: row.output_tokens,
                reasoning_output_tokens: row.reasoning_output_tokens,
                reported_total_tokens: row.total_tokens,
                observations: row.usage_observation_count,
                counter_resets: row.usage_reset_count,
                missing_observations: row.usage_missing_count,
                inconsistent_observations: row.usage_inconsistent_count,
            },
            models: ModelFacts {
                requested_model_observations: row.requested_model_observation_count,
                requested_model_changes: row.requested_model_change_count,
                observed_served_model_fallbacks: row.served_model_fallback_count,
            },
            tools: ToolFacts {
                requests: row.tool_request_count,
                results_observed: row.tool_result_count,
                failures_observed: row.observed_tool_failure_count,
                outcomes_unknown: row.unknown_tool_outcome_count,
            },
            pressure: PressureFacts {
                context_observations: row.context_observation_count,
                rate_limit_observations: row.rate_observation_count,
            },
            evidence_gaps: EvidenceGaps {
                context_observations_missing_values: row.context_gap_count,
                rate_limit_observations_missing_values: row.rate_gap_count,
                operational_events_missing_valid_timestamps: row.timestamp_gap_count,
            },
            reason_codes,
        }
    }

    pub(crate) fn json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub(crate) fn markdown(&self) -> String {
        let mut output = format!(
            "# Redacted AgentLens postmortem\n\n\
             Format version: {}  \n\
             Redaction profile: {}\n\n\
             ## Session\n\n\
             - Stable ID: {}\n\
             - Agent kind: {}\n\
             - Indexed status: {}\n\
             - Source projection: {}\n\n\
             ## Turn outcomes\n\n\
             - Observed: {}\n\
             - Completed: {}\n\
             - Interrupted: {}\n\
             - Unclosed: {}\n\
             - Terminal-only: {}\n\
             - Resumed: {}\n\n\
             ## Usage\n\n\
             - Input tokens: {}\n\
             - Cached input tokens: {}\n\
             - Output tokens: {}\n\
             - Reasoning output tokens: {}\n\
             - Reported total tokens: {}\n\
             - Observations: {}\n\
             - Counter resets: {}\n\
             - Missing observations: {}\n\
             - Inconsistent observations: {}\n\n\
             ## Model evidence\n\n\
             - Requested-model observations: {}\n\
             - Requested-model changes: {}\n\
             - Observed served-model fallbacks: {}\n\n\
             ## Tool outcomes\n\n\
             - Requests: {}\n\
             - Results observed: {}\n\
             - Failures observed: {}\n\
             - Outcomes unknown: {}\n\n\
             ## Pressure evidence\n\n\
             - Complete context observations: {}\n\
             - Rate-limit observations: {}\n\n\
             ## Known evidence gaps\n\n\
             - Context events missing values: {}\n\
             - Rate events missing values: {}\n\
             - Operational events missing valid timestamps: {}\n\n\
             ## Reason codes\n\n",
            self.format_version,
            self.redaction.profile,
            markdown_text(&self.session.stable_id),
            markdown_text(&self.session.agent_kind),
            markdown_text(&self.session.indexed_status),
            markdown_text(&self.session.source_projection_status),
            self.turns.observed,
            self.turns.completed,
            self.turns.interrupted,
            self.turns.unclosed,
            self.turns.terminal_only,
            self.turns.resumed,
            optional_number(self.usage.input_tokens),
            optional_number(self.usage.cached_input_tokens),
            optional_number(self.usage.output_tokens),
            optional_number(self.usage.reasoning_output_tokens),
            optional_number(self.usage.reported_total_tokens),
            self.usage.observations,
            self.usage.counter_resets,
            self.usage.missing_observations,
            self.usage.inconsistent_observations,
            self.models.requested_model_observations,
            self.models.requested_model_changes,
            self.models.observed_served_model_fallbacks,
            self.tools.requests,
            self.tools.results_observed,
            self.tools.failures_observed,
            self.tools.outcomes_unknown,
            self.pressure.context_observations,
            self.pressure.rate_limit_observations,
            self.evidence_gaps.context_observations_missing_values,
            self.evidence_gaps.rate_limit_observations_missing_values,
            self.evidence_gaps
                .operational_events_missing_valid_timestamps,
        );
        if self.reason_codes.is_empty() {
            output.push_str("- None observed.\n");
        } else {
            for reason in &self.reason_codes {
                output.push_str(&format!(
                    "- `{}` ({}): {}\n",
                    reason.code, reason.count, reason.explanation
                ));
            }
        }
        output.push_str("\n## Excluded by redaction\n\n");
        for field in self.redaction.excluded {
            output.push_str(&format!("- {field}\n"));
        }
        output
    }
}

fn push_reason(
    reasons: &mut Vec<ReasonCode>,
    code: &'static str,
    count: i64,
    explanation: &'static str,
) {
    if count > 0 {
        reasons.push(ReasonCode {
            code,
            count,
            explanation,
        });
    }
}

fn optional_number(value: Option<i64>) -> String {
    value.map_or_else(|| "Unavailable".into(), |value| value.to_string())
}

fn markdown_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '<'
                | '>'
                | '('
                | ')'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '|'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn fixed_agent_kind(value: &str) -> &'static str {
    if value == crate::sources::codex::KIND {
        crate::sources::codex::KIND
    } else {
        "other"
    }
}

fn fixed_session_status(value: &str) -> &'static str {
    match value {
        "complete" => "complete",
        "aborted" => "aborted",
        _ => "unknown",
    }
}

fn fixed_source_status(value: &str) -> &'static str {
    match value {
        "ok" => "ok",
        "partial" => "partial",
        "error" => "error",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_values_cannot_inject_structure() {
        assert_eq!(
            markdown_text("[x](javascript:bad)<tag>"),
            "\\[x\\]\\(javascript:bad\\)\\<tag\\>"
        );
    }
}
