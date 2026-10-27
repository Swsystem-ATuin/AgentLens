# Import masking

Agent Lens can mask transcript data **before** writing it to the disposable SQLite index. All adapters use the same sanitizer. Global rules always apply; additional user rules apply to explicitly assigned source keys, regardless of who imports or views those sources. Access grants do not establish ownership. In this first version an operator maintains the policy file; there is no account settings editor or automatic source-owner assignment.

## Policy

Store the JSON file outside `data/` and copied agent homes, preferably in an operator-only directory with mode `0600`. It can contain sensitive literals, so do not commit it or include it in support reports. Rules support exact case-sensitive text, Rust regex patterns (without backreferences or look-around), and ASCII case-insensitive JSON field names. Every rule needs a unique ID containing only letters, digits, underscores, or hyphens. IDs appear in audit records, so use non-sensitive labels.

```json
{
  "global": [
    {"id": "password-fields", "type": "field", "value": "password"},
    {"id": "token-fields", "type": "field", "value": "api_key"},
    {"id": "private-key-header", "type": "regex", "value": "-----BEGIN [A-Z ]*PRIVATE KEY-----[\\s\\S]*?-----END [A-Z ]*PRIVATE KEY-----"}
  ],
  "users": [
    {
      "id": "owner-a",
      "sources": ["src_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"],
      "rules": [
        {"id": "owner-a-project", "type": "exact", "value": "synthetic-private-project"}
      ]
    }
  ]
}
```

Replace the example source key with a real key from the authenticated Source health view or the `agent-lens account permissions --username USER` command. User IDs here are policy labels, not account usernames. A source can receive rules from multiple users; all apply. An empty sources list is rejected. No user exception can disable a global rule. Rules have no custom replacements: matches become `[REDACTED]`. Matches are found against original text and overlapping spans are merged, so replacement text does not become input to later rules.

Policies are limited to 1 MiB and 1,000 rules, compiled regex size is bounded, unknown configuration fields are rejected, and malformed rules fail before import. Error messages do not echo policy contents.

## Commands and policy changes

Use the same policy for rebuild, refresh, and serve:

```sh
export AGENT_LENS_REDACTION_FILE=/etc/agent-lens/redaction.json
agent-lens --database /var/lib/agent-lens/agent-lens.sqlite rebuild --data /var/lib/agent-lens/data
agent-lens --database /var/lib/agent-lens/agent-lens.sqlite refresh --data /var/lib/agent-lens/data
agent-lens --database /var/lib/agent-lens/agent-lens.sqlite serve
```

Alternatively pass the global option `--redaction-file /etc/agent-lens/redaction.json` before the subcommand. The packaged systemd and SysV jobs inherit `AGENT_LENS_REDACTION_FILE` from their shared environment file.

**Stop the web service and refresh automation before enabling or changing a policy.** Validate the file by running a staged rebuild, then restart both with the same configuration. Keep services stopped if the rebuild fails. Do not edit policies while the web process is running: it validates the policy at startup, not on every request. Review the [rebuild publication requirements](rebuild.md).

A fingerprint of the exact policy bytes is recorded in `index_metadata`. Refresh and server startup refuse a populated index whose policy differs, including removal of the policy setting. Whitespace-only edits also require rebuild. A rebuild applies the new rules to unchanged sources using a new staging database, including a fresh full-text search index. This deliberately avoids updating sensitive values in place and leaving them in SQLite free pages or FTS segments. A failed rebuild does not sanitize the old index; leave it offline.

No policy is enabled by default. Explicitly configure rules before treating imports as protected. Applying a policy to an existing index requires a rebuild; its schema additions are created automatically. Audit rows in `redaction_audit` record ingest run, source key, policy fingerprint, rule ID, and match count, never values or excerpts. Counts include matching retained/normalized copies and overlap, not unique secrets; field counts describe matching JSON keys. Unchanged sources have zero new replacements. Omitted malformed structures and depth-limited structures do not count as rule matches.

## Coverage and boundaries

Masking covers messages, tool inputs/results, retained JSON keys and values, session metadata, workspace directories, model/usage text, and turn evidence. JSON nested inside string values is processed too. Field rules remove matching values from normalized copies within the **same record**; they do not learn secrets for use in other records. Use an exact/regex rule when a known value must be removed everywhere. Numeric and boolean JSON values can also be replaced; normalized numeric usage accounting is retained.

Malformed text that begins with `{` or `[` is omitted when masking is active rather than storing an uninspectable structured payload. If masking across JSON syntax would make a structured value invalid, that value is omitted. Nested structures beyond depth 32 are omitted. Other ordinary conversation remains intact. Masked titles are derived from sanitized conversation before truncation; pre-truncated advisory and source titles are not used for protected sources. Sessions without an eligible user message use their native ID as a title.

Internal host/user labels, source-home paths, source-file paths, and native session/call/turn identities are necessary for discovery, refresh, and pairing. They are **not rewritten**. If a text rule matches one, import refuses that source (or aborts discovery for a matching locator), rather than writing the matching value. Use non-sensitive source layouts and native identities. Field rules target payload fields rather than operational locators. Fixed diagnostic codes replace parser details for protected sources. Failed sources are omitted from a new rebuild; inspect the import's error/partial counts before enabling service.

Original raw files remain untouched and can still contain sensitive content. This feature protects the imported text index; it does not sanitize copied agent homes, image files or other original artifacts, operational identity metadata outside the index, historical backups, or previously created database/WAL files. Existing copies must be retired under your storage/backup retention process; replacing a database is not a physical secure erase. To avoid retaining raw sensitive data at all, a collector must sanitize before copying it into Agent Lens's source storage.

Rules do not guarantee detection of all credentials or personal data. This version does not include automatic credential detection, remote detection services, reversible tokenization, or per-viewer masking.

## Verification and performance

Synthetic automated regressions cover global/user rule composition and isolation, invalid policy diagnostics, overlapping matches, structured and encoded fields, malformed content, Unicode, long-secret title truncation, immutable raw sources, Gemini and both Hermes import paths, policy-change/removal refusal, search, on-disk SQLite content, and initial/resumed transcript HTML.

Three alternating warm-cache rebuilds per variant used separate SQLite outputs and a generated 13,568,983-byte Codex rollout with 20,000 alternating user/assistant messages. Each message contained ordinary synthetic text, a matching placeholder literal, and 500 padding characters. The enabled policy had one matching exact rule, one non-matching regex, and one non-matching field rule. Measurements compare the pre-change local debug executable with the final debug executable; these are unoptimized local measurements, not production throughput guarantees. Elapsed time includes staged publication and integrity validation. Peak RSS is the child process maximum reported by Linux `wait4`. No private development source was read.

| Variant | Median elapsed | Elapsed range | Peak RSS range |
| --- | ---: | ---: | ---: |
| Before masking support | 7.404 s | 7.231–7.623 s | 29,016–29,692 KiB |
| Support present, policy disabled | 8.278 s | 8.200–8.339 s | 30,924–31,108 KiB |
| Three rules enabled | 14.164 s | 12.265–14.215 s | 33,200–34,148 KiB |

The disabled variant was 11.8% slower at the median; enabled masking was 91.3% slower than the original executable (71.1% slower than the new disabled variant). Median peak RSS increased by about 1.6 MiB disabled and 4.4 MiB enabled relative to baseline. The cost is material and should be considered when scheduling large rebuilds. Regex complexity, number of rules, JSON nesting, and match density affect the result; release builds and other adapters were not benchmarked for this change.

Source scan/checksum I/O and the outer transaction/per-source savepoints are unchanged. Policy compilation happens once before import. The added database work is two policy-validation reads and one policy fingerprint write per run, one audit insert per applicable rule/source, and one safe-title update per protected session. Sanitization parses and reconstructs JSON, scans decoded and serialized text, and allocates match spans and field-value copies for the current record; it does not retain transcript bodies across records or reread source files once per rule. Memory remains tied to the largest source record/session already loaded by each adapter plus the configured rules, rather than accumulating sanitized conversations for the whole import.
