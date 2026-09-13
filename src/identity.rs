fn digest(prefix: &[u8], parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(prefix);
    for part in parts {
        hasher.update(&(part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hasher.finalize().to_hex().to_string()
}

pub(crate) fn source_id(agent_kind: &str, host: &str, user: &str, source_home: &str) -> String {
    format!(
        "src_{}",
        digest(
            b"agent-lens-source-v1\0",
            &[agent_kind, host, user, source_home]
        )
    )
}

pub(crate) fn session_id(source_id: &str, native_id: &str) -> String {
    format!(
        "ses_{}",
        digest(b"agent-lens-session-v1\0", &[source_id, native_id])
    )
}

pub(crate) fn source_file_id(source_id: &str, kind: &str, path: &str) -> String {
    format!(
        "file_{}",
        digest(b"agent-lens-source-file-v1\0", &[source_id, kind, path])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_are_stable_and_source_scoped() {
        let first_source = source_id("codex", "host-a", "user", "/copy/a/.codex");
        assert_eq!(
            first_source,
            source_id("codex", "host-a", "user", "/copy/a/.codex")
        );
        let other_source = source_id("codex", "host-b", "user", "/copy/b/.codex");
        assert_ne!(first_source, other_source);
        assert_ne!(
            session_id(&first_source, "same-native-id"),
            session_id(&other_source, "same-native-id")
        );
        assert_eq!(
            source_file_id(&first_source, "sessions", "nested/one.jsonl"),
            source_file_id(&first_source, "sessions", "nested/one.jsonl")
        );
        assert_ne!(
            source_file_id(&first_source, "sessions", "nested/one.jsonl"),
            source_file_id(&first_source, "sessions", "nested/two.jsonl")
        );
        assert_ne!(
            source_id("ab", "c", "", ""),
            source_id("a", "bc", "", ""),
            "length prefixes prevent ambiguous field concatenation"
        );
    }
}
