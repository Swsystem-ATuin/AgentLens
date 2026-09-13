use std::path::PathBuf;

pub(crate) mod codex;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceLayout {
    HostUser,
    DirectHome,
}

pub(crate) struct DiscoveredHome {
    pub(crate) host: String,
    pub(crate) user: String,
    pub(crate) path: PathBuf,
}

pub(crate) struct DiscoveredSources {
    pub(crate) layout: SourceLayout,
    pub(crate) database_layout: &'static str,
    pub(crate) homes: Vec<DiscoveredHome>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct TokenUsage {
    pub(crate) input_tokens: Option<i64>,
    pub(crate) output_tokens: Option<i64>,
    pub(crate) cached_input_tokens: Option<i64>,
    pub(crate) reasoning_output_tokens: Option<i64>,
    pub(crate) total_tokens: Option<i64>,
}

impl TokenUsage {
    pub(crate) fn has_missing_core(&self) -> bool {
        self.input_tokens.is_none()
            || self.output_tokens.is_none()
            || self.cached_input_tokens.is_none()
            || self.total_tokens.is_none()
    }

    pub(crate) fn is_inconsistent(&self) -> bool {
        let invalid = [
            self.input_tokens,
            self.output_tokens,
            self.cached_input_tokens,
            self.reasoning_output_tokens,
            self.total_tokens,
        ]
        .into_iter()
        .flatten()
        .any(|value| value < 0);
        let cached_invalid = matches!((self.cached_input_tokens, self.input_tokens), (Some(cached), Some(input)) if cached > input);
        let reasoning_invalid = matches!((self.reasoning_output_tokens, self.output_tokens), (Some(reasoning), Some(output)) if reasoning > output);
        let total_invalid = match (self.input_tokens, self.output_tokens, self.total_tokens) {
            (Some(input), Some(output), Some(total)) => input
                .checked_add(output)
                .is_none_or(|expected| expected != total),
            _ => false,
        };
        invalid || cached_invalid || reasoning_invalid || total_invalid
    }
}

pub(crate) enum NormalizedRecord {
    Message {
        timestamp: Option<String>,
        role: String,
        content: String,
        raw_json: String,
    },
    ToolCall {
        timestamp: Option<String>,
        call_id: Option<String>,
        tool_name: Option<String>,
        arguments: Option<String>,
        kind: &'static str,
        raw_json: String,
    },
    ToolOutput {
        timestamp: Option<String>,
        call_id: Option<String>,
        output: Option<String>,
        outcome: &'static str,
        raw_json: String,
    },
    Usage {
        timestamp: Option<String>,
        cumulative: Option<TokenUsage>,
        per_turn: Option<TokenUsage>,
        raw_json: String,
    },
}

pub(crate) struct MessagePresentation {
    pub(crate) title: &'static str,
    pub(crate) initially_open: bool,
}

pub(crate) struct ModelExclusion {
    pub(crate) source_kind: &'static str,
    pub(crate) pattern: &'static str,
}

pub(crate) fn model_exclusion() -> ModelExclusion {
    ModelExclusion {
        source_kind: codex::KIND,
        pattern: codex::HIDDEN_MODEL_PATTERN,
    }
}

pub(crate) fn classify_message(
    source_kind: &str,
    role: &str,
    content: &str,
    before_conversation: bool,
) -> Option<MessagePresentation> {
    match source_kind {
        codex::KIND => codex::classify_message(role, content, before_conversation),
        _ => None,
    }
}
