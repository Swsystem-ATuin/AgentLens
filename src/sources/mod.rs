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
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        cached_input_tokens: Option<i64>,
        total_tokens: Option<i64>,
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
