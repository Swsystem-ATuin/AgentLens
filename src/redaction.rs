//! Shared, pre-persistence masking. Policies never contain viewer-dependent rules.
use anyhow::{Context, Result, ensure};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;
use serde_json::Value;
use std::{cell::Cell, collections::HashSet, path::Path};

const MARKER: &str = "[REDACTED]";

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    global: Vec<RuleConfig>,
    #[serde(default)]
    users: Vec<UserConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserConfig {
    id: String,
    sources: Vec<String>,
    rules: Vec<RuleConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleConfig {
    id: String,
    #[serde(flatten)]
    matcher: MatcherConfig,
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum MatcherConfig {
    Exact(String),
    Regex(String),
    Field(String),
}

#[derive(Clone)]
enum Matcher {
    Exact(String),
    Regex(Regex),
    Field(String),
}

#[derive(Clone)]
struct Rule {
    id: String,
    matcher: Matcher,
    count: Cell<u64>,
}

#[derive(Default)]
pub(crate) struct Policy {
    pub(crate) fingerprint: String,
    global: Vec<Rule>,
    users: Vec<(Vec<String>, Vec<Rule>)>,
}

#[derive(Default)]
pub(crate) struct Sanitizer {
    rules: Vec<Rule>,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}

impl Policy {
    pub(crate) fn load(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self::default());
        };
        let metadata = std::fs::metadata(path).context("inspect redaction policy")?;
        ensure!(
            metadata.len() <= 1_048_576,
            "redaction policy exceeds 1 MiB"
        );
        let bytes = std::fs::read(path).context("read redaction policy")?;
        Self::parse(&bytes)
    }

    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        // Do not forward serde/regex diagnostics: they can contain policy secrets.
        ensure!(bytes.len() <= 1_048_576, "redaction policy exceeds 1 MiB");
        let config: Config = serde_json::from_slice(bytes)
            .map_err(|_| anyhow::anyhow!("invalid redaction policy JSON or rule shape"))?;
        let mut ids = HashSet::new();
        let mut total = 0;
        let mut compile = |configs: Vec<RuleConfig>| -> Result<Vec<Rule>> {
            configs
                .into_iter()
                .map(|config| {
                    total += 1;
                    ensure!(total <= 1000, "redaction policy exceeds 1000 rules");
                    ensure!(
                        valid_id(&config.id) && ids.insert(config.id.clone()),
                        "redaction rule IDs must be unique safe identifiers"
                    );
                    let matcher = match config.matcher {
                        MatcherConfig::Exact(value) => {
                            ensure!(!value.is_empty(), "redaction exact value must not be empty");
                            Matcher::Exact(value)
                        }
                        MatcherConfig::Field(value) => {
                            ensure!(!value.is_empty(), "redaction field name must not be empty");
                            Matcher::Field(value)
                        }
                        MatcherConfig::Regex(value) => {
                            let regex = RegexBuilder::new(&value)
                                .size_limit(1_048_576)
                                .dfa_size_limit(1_048_576)
                                .build()
                                .map_err(|_| {
                                    anyhow::anyhow!("invalid or oversized redaction regex")
                                })?;
                            ensure!(
                                !regex.is_match(""),
                                "redaction regex must not match empty text"
                            );
                            Matcher::Regex(regex)
                        }
                    };
                    Ok(Rule {
                        id: config.id,
                        matcher,
                        count: Cell::new(0),
                    })
                })
                .collect()
        };
        let global = compile(config.global)?;
        let mut users = Vec::new();
        let mut user_ids = HashSet::new();
        for user in config.users {
            ensure!(
                valid_id(&user.id) && user_ids.insert(user.id),
                "redaction user IDs must be unique safe identifiers"
            );
            ensure!(
                !user.sources.is_empty()
                    && user.sources.iter().all(|s| s.starts_with("src_")
                        && s.len() == 68
                        && s[4..]
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))),
                "redaction users require explicit source keys"
            );
            users.push((user.sources, compile(user.rules)?));
        }
        Ok(Self {
            fingerprint: blake3::hash(bytes).to_hex().to_string(),
            global,
            users,
        })
    }

    pub(crate) async fn validate_index(&self, pool: &sqlx::SqlitePool) -> Result<()> {
        let mut connection = pool.acquire().await?;
        self.validate_connection(&mut connection).await
    }

    pub(crate) async fn validate_connection(
        &self,
        connection: &mut sqlx::SqliteConnection,
    ) -> Result<()> {
        let previous: Option<String> =
            sqlx::query_scalar("SELECT value FROM index_metadata WHERE key='redaction_policy'")
                .fetch_optional(&mut *connection)
                .await?;
        let populated: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM source_files) OR EXISTS(SELECT 1 FROM sessions)",
        )
        .fetch_one(&mut *connection)
        .await?;
        ensure!(
            populated == 0 || previous.as_deref().unwrap_or("") == self.fingerprint,
            "redaction policy changed; stop the web service and run rebuild before serving this index"
        );
        Ok(())
    }

    pub(crate) fn for_source(&self, source: &str) -> Sanitizer {
        let mut rules = self.global.clone();
        for (sources, extra) in &self.users {
            if sources.iter().any(|s| s == source) {
                rules.extend(extra.clone());
            }
        }
        Sanitizer { rules }
    }
}

impl Sanitizer {
    pub(crate) fn active(&self) -> bool {
        !self.rules.is_empty()
    }

    pub(crate) fn counts(&self) -> impl Iterator<Item = (&str, u64)> {
        self.rules
            .iter()
            .map(|rule| (rule.id.as_str(), rule.count.get()))
    }

    fn hit(rule: &Rule, count: usize) {
        rule.count
            .set(rule.count.get().saturating_add(count as u64));
    }

    fn plain(&self, text: &str, secrets: &[String]) -> String {
        let mut spans = Vec::new();
        for rule in &self.rules {
            match &rule.matcher {
                Matcher::Exact(value) => {
                    let hits: Vec<_> = text
                        .match_indices(value)
                        .map(|(start, value)| (start, start + value.len()))
                        .collect();
                    Self::hit(rule, hits.len());
                    spans.extend(hits);
                }
                Matcher::Regex(regex) => {
                    let hits: Vec<_> = regex
                        .find_iter(text)
                        .filter(|m| !m.is_empty())
                        .map(|m| (m.start(), m.end()))
                        .collect();
                    Self::hit(rule, hits.len());
                    spans.extend(hits);
                }
                Matcher::Field(_) => {}
            }
        }
        for value in secrets.iter().filter(|v| !v.is_empty()) {
            spans.extend(
                text.match_indices(value)
                    .map(|(start, value)| (start, start + value.len())),
            );
        }
        spans.sort_unstable();
        let mut result = String::new();
        let mut end = 0;
        for (start, next) in spans {
            if start >= end {
                result.push_str(&text[end..start]);
                result.push_str(MARKER);
            }
            end = end.max(next);
        }
        result.push_str(&text[end..]);
        result
    }

    fn collect(&self, value: &Value, secrets: &mut Vec<String>, depth: usize) {
        if depth >= 32 {
            return;
        }
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    if self.rules.iter().any(|r| matches!(&r.matcher, Matcher::Field(name) if key.eq_ignore_ascii_case(name))) {
                    Self::strings(value, secrets);
                }
                    self.collect(value, secrets, depth + 1);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.collect(item, secrets, depth + 1);
                }
            }
            Value::String(text) => {
                if let Ok(nested) = serde_json::from_str::<Value>(text) {
                    self.collect(&nested, secrets, depth + 1);
                }
            }
            _ => {}
        }
    }

    fn strings(value: &Value, secrets: &mut Vec<String>) {
        match value {
            Value::String(text) => {
                if !text.is_empty() {
                    secrets.push(text.clone());
                }
            }
            Value::Array(items) => {
                for item in items {
                    Self::strings(item, secrets);
                }
            }
            Value::Object(map) => {
                for item in map.values() {
                    Self::strings(item, secrets);
                }
            }
            Value::Number(n) => secrets.push(n.to_string()),
            _ => {}
        }
    }

    fn walk(&self, value: &mut Value, secrets: &[String], depth: usize) {
        if depth >= 32 {
            *value = Value::String(MARKER.into());
            return;
        }
        match value {
            Value::Object(map) => {
                let old = std::mem::take(map);
                for (key, mut value) in old {
                    let mut field = false;
                    for rule in &self.rules {
                        if matches!(&rule.matcher, Matcher::Field(name) if key.eq_ignore_ascii_case(name))
                        {
                            Self::hit(rule, 1);
                            field = true;
                        }
                    }
                    if field {
                        value = Value::String(MARKER.into());
                    } else {
                        self.walk(&mut value, secrets, depth + 1);
                    }
                    map.insert(self.plain(&key, secrets), value);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.walk(item, secrets, depth + 1);
                }
            }
            Value::String(text) => {
                *text = self.encode(text, secrets, depth + 1);
            }
            Value::Number(_) | Value::Bool(_) => {
                let original = value.to_string();
                let masked = self.plain(&original, secrets);
                if masked != original {
                    *value = Value::String(masked);
                }
            }
            Value::Null => {}
        }
    }

    /// Field values found in raw JSON are also removed from normalized copies.
    pub(crate) fn secrets(&self, raw: &str) -> Vec<String> {
        let mut secrets = Vec::new();
        if self.active()
            && let Ok(value) = serde_json::from_str::<Value>(raw)
        {
            self.collect(&value, &mut secrets, 0);
        }
        secrets.sort();
        secrets.dedup();
        secrets
    }

    pub(crate) fn text(&self, text: &str, secrets: &[String]) -> String {
        if !self.active() {
            return text.to_owned();
        }
        self.encode(text, secrets, 0)
    }

    fn encode(&self, text: &str, secrets: &[String], depth: usize) -> String {
        if depth >= 32 || secrets.iter().any(|secret| secret == text) {
            return MARKER.to_owned();
        }
        if let Ok(mut value) = serde_json::from_str::<Value>(text) {
            let mut all = secrets.to_vec();
            self.collect(&value, &mut all, depth);
            // Match serialized spans too, including patterns crossing JSON syntax.
            let masked = self.plain(text, &all);
            if masked != text {
                let Ok(reparsed) = serde_json::from_str(&masked) else {
                    return MARKER.to_owned();
                };
                value = reparsed;
            }
            self.walk(&mut value, &all, depth);
            value.to_string()
        } else if text.trim_start().starts_with(['{', '[']) {
            MARKER.to_owned()
        } else {
            self.plain(text, secrets)
        }
    }

    pub(crate) fn optional(&self, text: Option<String>, secrets: &[String]) -> Option<String> {
        text.map(|s| self.text(&s, secrets))
    }

    /// Internal locators cannot be rewritten without breaking discovery/identity.
    /// Refuse the import instead of persisting a matching sensitive locator.
    pub(crate) fn check_locator(&self, text: &str) -> Result<()> {
        ensure!(
            self.plain(text, &[]) == text,
            "redaction rule matches an internal source locator or identity; use a non-sensitive source layout/identity before rebuilding"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_and_user_rules_are_additive_and_source_scoped() {
        let source = format!("src_{}", "a".repeat(64));
        let policy = Policy::parse(serde_json::json!({
            "global":[{"id":"global-secret","type":"exact","value":"global-private"}],
            "users":[{"id":"owner","sources":[source],"rules":[{"id":"user-secret","type":"regex","value":"user-private-[0-9]+"}]}]
        }).to_string().as_bytes()).unwrap();
        assert_eq!(
            policy
                .for_source(&source)
                .text("global-private user-private-12 ordinary", &[]),
            "[REDACTED] [REDACTED] ordinary"
        );
        assert_eq!(
            policy
                .for_source("another-source")
                .text("global-private user-private-12 ordinary", &[]),
            "[REDACTED] user-private-12 ordinary"
        );
    }

    #[test]
    fn structured_and_encoded_json_fields_mask_normalized_copies() {
        let policy = Policy::parse(
            br#"{"global":[{"id":"credentials","type":"field","value":"password"}]}"#,
        )
        .unwrap();
        let sanitizer = policy.for_source("");
        let raw = serde_json::json!({"unknown":{"PaSsWoRd":"synthetic-credential"},"arguments":"{\"password\":\"nested-credential\",\"ordinary\":true}","copy":"synthetic-credential"}).to_string();
        let secrets = sanitizer.secrets(&raw);
        let result = sanitizer.text(&raw, &secrets);
        assert!(!result.contains("synthetic-credential"));
        assert!(!result.contains("nested-credential"));
        assert!(result.contains("ordinary"));
        assert_eq!(
            sanitizer.text("synthetic-credential nested-credential ordinary", &secrets),
            "[REDACTED] [REDACTED] ordinary"
        );
    }

    #[test]
    fn invalid_policies_do_not_echo_sensitive_values() {
        for bytes in [
            br#"{"global":[{"id":"secret","type":"regex","value":"PRIVATE(["}]}"#.as_slice(),
            br#"{"global":[{"id":"secret","type":"exact","value":""}]}"#,
            br#"{"typo":"PRIVATE"}"#,
        ] {
            let error = Policy::parse(bytes)
                .err()
                .expect("invalid policy must fail");
            assert!(!format!("{error:#}").contains("PRIVATE"));
        }
    }

    #[test]
    fn overlapping_rules_do_not_rescan_replacements_or_leak_suffixes() {
        let policy = Policy::parse(br#"{"global":[{"id":"a","type":"exact","value":"abc"},{"id":"b","type":"exact","value":"bcde"}]}"#).unwrap();
        assert_eq!(
            policy.for_source("").text("abcde ordinary", &[]),
            "[REDACTED] ordinary"
        );
    }
}

#[cfg(test)]
mod partial_tests {
    use super::*;

    #[test]
    fn partial_structured_content_is_omitted_and_unicode_is_preserved() {
        let policy = Policy::parse(br#"{"global":[{"id":"fields","type":"field","value":"password"},{"id":"exact","type":"exact","value":"private"}]}"#).unwrap();
        let sanitizer = policy.for_source("");
        assert_eq!(
            sanitizer.text("{\"password\":\"unfinished-secret", &[]),
            MARKER
        );
        let nested =
            serde_json::json!({"arguments":"{\"password\":\"unfinished-secret"}).to_string();
        assert!(!sanitizer.text(&nested, &[]).contains("unfinished-secret"));
        assert_eq!(
            sanitizer.text("ordinary café private 日本語", &[]),
            "ordinary café [REDACTED] 日本語"
        );
    }
}

#[cfg(test)]
mod structured_span_tests {
    use super::*;

    #[test]
    fn whole_json_valued_fields_also_remove_the_normalized_object() {
        let policy =
            Policy::parse(br#"{"global":[{"id":"arguments","type":"field","value":"arguments"}]}"#)
                .unwrap();
        let sanitizer = policy.for_source("");
        let arguments = r#"{"ordinary":"sensitive-field-value"}"#;
        let raw = serde_json::json!({"arguments":arguments}).to_string();
        let secrets = sanitizer.secrets(&raw);
        assert_eq!(sanitizer.text(arguments, &secrets), MARKER);
        assert!(
            !sanitizer
                .text(&raw, &secrets)
                .contains("sensitive-field-value")
        );
    }

    #[test]
    fn literal_spans_crossing_json_syntax_are_removed() {
        let policy =
            Policy::parse(br#"{"global":[{"id":"span","type":"exact","value":"a\",\"b"}]}"#)
                .unwrap();
        let sanitizer = policy.for_source("");
        let raw = r#"["a","b"]"#;
        assert!(!sanitizer.text(raw, &[]).contains("a\",\"b"));
        let nested = serde_json::json!({"encoded":raw}).to_string();
        let result = sanitizer.text(&nested, &[]);
        assert!(!result.contains("a\\\",\\\"b"));
    }
}
