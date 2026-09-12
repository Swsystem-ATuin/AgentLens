use super::queries::{FilterValues, MessageRow};

pub(super) struct TranscriptSegment {
    pub(super) sequence: i64,
    pub(super) timestamp: Option<String>,
    pub(super) role: String,
    pub(super) paragraphs: Vec<String>,
    pub(super) truncated_sequences: Vec<i64>,
    pub(super) title: String,
    pub(super) initially_open: bool,
}

pub(super) fn group_transcript_section(
    messages: Vec<MessageRow>,
    source_kind: &str,
    mut conversation_started: bool,
) -> Vec<TranscriptSegment> {
    let mut segments: Vec<TranscriptSegment> = Vec::new();
    for message in messages {
        let presentation = crate::sources::classify_message(
            source_kind,
            &message.role,
            &message.content,
            !conversation_started,
        );
        if !conversation_started
            && message.role == "user"
            && presentation
                .as_ref()
                .is_none_or(|presentation| presentation.initially_open)
        {
            conversation_started = true;
        }
        if message.role == "assistant"
            && presentation.is_none()
            && let Some(previous) = segments.last_mut()
            && previous.role == "assistant"
            && previous.initially_open
        {
            previous.paragraphs.push(message.content);
            if message.content_truncated {
                previous.truncated_sequences.push(message.sequence);
            }
            continue;
        }
        let truncated_sequences = message
            .content_truncated
            .then_some(message.sequence)
            .into_iter()
            .collect();
        segments.push(TranscriptSegment {
            sequence: message.sequence,
            timestamp: message.timestamp,
            title: presentation
                .as_ref()
                .map(|presentation| presentation.title.to_owned())
                .unwrap_or_else(|| message.role.clone()),
            initially_open: presentation
                .as_ref()
                .is_none_or(|presentation| presentation.initially_open),
            role: message.role,
            paragraphs: vec![message.content],
            truncated_sequences,
        });
    }
    segments
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
}

impl FilterOptions {
    pub(super) fn new(
        values: FilterValues,
        host: Option<&str>,
        user: Option<&str>,
        model: Option<&str>,
        status: Option<&str>,
    ) -> Self {
        Self {
            hosts: select_options(values.hosts, host),
            users: select_options(values.users, user),
            models: select_options(values.models, model),
            statuses: select_options(values.statuses, status),
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

pub(super) fn sessions_href(
    host: Option<&str>,
    user: Option<&str>,
    model: Option<&str>,
    status: Option<&str>,
    page: u32,
) -> String {
    let mut pairs = Vec::new();
    for (name, value) in [
        ("host", host),
        ("user", user),
        ("model", model),
        ("status", status),
    ] {
        if let Some(value) = value {
            pairs.push(format!("{name}={}", percent_encode(value)));
        }
    }
    pairs.push(format!("page={page}"));
    format!("/sessions?{}", pairs.join("&"))
}

pub(super) fn transcript_href(id: i64, section: u32) -> String {
    format!("/sessions/{id}?section={section}")
}

pub(super) fn message_href(id: i64, sequence: i64, part: u32) -> String {
    format!("/sessions/{id}/messages/{sequence}?part={part}")
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
