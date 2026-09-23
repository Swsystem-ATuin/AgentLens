use anyhow::{Result, ensure};
use std::{
    fs::Metadata,
    path::{Path, PathBuf},
};

pub(crate) mod codex;
pub(crate) mod gemini;
pub(crate) mod hermes;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Adapter {
    Codex,
    Gemini,
    Hermes,
}

impl Adapter {
    pub(crate) fn kind(self) -> &'static str {
        match self {
            Self::Codex => codex::KIND,
            Self::Gemini => gemini::KIND,
            Self::Hermes => hermes::KIND,
        }
    }

    pub(crate) fn display_name(self) -> &'static str {
        match self {
            Self::Codex => codex::DISPLAY_NAME,
            Self::Gemini => gemini::DISPLAY_NAME,
            Self::Hermes => hermes::DISPLAY_NAME,
        }
    }

    pub(crate) fn parser_version(self) -> i64 {
        match self {
            Self::Codex => codex::PARSER_VERSION,
            Self::Gemini => gemini::PARSER_VERSION,
            Self::Hermes => hermes::PARSER_VERSION,
        }
    }

    pub(crate) fn source_rank(self, kind: &str) -> usize {
        match self {
            Self::Codex => codex::SOURCE_AREAS
                .iter()
                .position(|candidate| *candidate == kind),
            Self::Gemini => gemini::SOURCE_AREAS
                .iter()
                .position(|candidate| *candidate == kind),
            Self::Hermes => hermes::SOURCE_AREAS
                .iter()
                .position(|candidate| *candidate == kind),
        }
        .unwrap_or(usize::MAX)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SourceLayout {
    HostUser,
    DirectHome,
}

pub(crate) struct DiscoveredHome {
    pub(crate) adapter: Adapter,
    pub(crate) host: String,
    pub(crate) user: String,
    pub(crate) path: PathBuf,
}

pub(crate) struct SessionSummary {
    pub(crate) metadata: Metadata,
    pub(crate) content_hash: String,
    pub(crate) external_id: String,
    pub(crate) started_at: Option<String>,
    pub(crate) ended_at: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) status: &'static str,
    pub(crate) raw_metadata: String,
    pub(crate) derived_title: Option<String>,
    pub(crate) malformed_lines: usize,
    pub(crate) parse_error: Option<String>,
}

pub(crate) struct DiscoveredSources {
    pub(crate) layout: SourceLayout,
    pub(crate) database_layout: &'static str,
    pub(crate) homes: Vec<DiscoveredHome>,
}

pub(crate) fn discover(root: &Path) -> Result<DiscoveredSources> {
    ensure!(
        codex::optional_directory(root)?,
        "source root is missing; refresh cancelled"
    );
    if root.file_name() == Some(std::ffi::OsStr::new(".codex")) {
        return codex::discover(root);
    }
    if root.file_name() == Some(std::ffi::OsStr::new(".gemini")) {
        return gemini::discover(root);
    }
    if root.file_name() == Some(std::ffi::OsStr::new(".hermes")) {
        return hermes::discover(root);
    }
    let mut codex = codex::discover(root)?;
    let gemini = gemini::discover(root)?;
    let hermes = hermes::discover(root)?;
    codex.homes.extend(gemini.homes);
    codex.homes.extend(hermes.homes);
    Ok(codex)
}

pub(crate) fn home_belongs_to_root(
    adapter: Adapter,
    home: &Path,
    root: &Path,
    layout: SourceLayout,
) -> bool {
    match layout {
        SourceLayout::DirectHome => home == root,
        SourceLayout::HostUser => home.strip_prefix(root).is_ok_and(|relative| {
            relative.components().count() == 3
                && relative.file_name()
                    == Some(std::ffi::OsStr::new(match adapter {
                        Adapter::Codex => ".codex",
                        Adapter::Gemini => ".gemini",
                        Adapter::Hermes => ".hermes",
                    }))
        }),
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
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

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct LimitWindow {
    pub(crate) used_percent: Option<f64>,
    pub(crate) window_minutes: Option<i64>,
    pub(crate) resets_at: Option<i64>,
    pub(crate) resets_in_seconds: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PressureSnapshot {
    pub(crate) context_window: Option<i64>,
    pub(crate) context_tokens: Option<i64>,
    pub(crate) rate_limit_id: Option<String>,
    pub(crate) rate_limit_name: Option<String>,
    pub(crate) primary: LimitWindow,
    pub(crate) secondary: LimitWindow,
    pub(crate) plan_type: Option<String>,
    pub(crate) rate_limit_reached_type: Option<String>,
}

pub(crate) enum NormalizedRecord {
    Message {
        timestamp: Option<String>,
        role: String,
        content: String,
        opens_turn: bool,
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
        pressure: Box<PressureSnapshot>,
        raw_json: String,
    },
    TurnEvent {
        timestamp: Option<String>,
        kind: &'static str,
        turn_id: Option<String>,
        requested_model: Option<String>,
        served_model: Option<String>,
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
        gemini::KIND => gemini::classify_message(role),
        hermes::KIND => hermes::classify_message(role),
        _ => None,
    }
}
