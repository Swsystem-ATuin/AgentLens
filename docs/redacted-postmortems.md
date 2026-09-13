# Redacted postmortem exports

Each visible session page links to authenticated JSON and Markdown downloads:

```text
/sessions/{stable_id}/postmortem/json
/sessions/{stable_id}/postmortem/md
```

The endpoints use the same HTTP Basic authentication, hidden-session policy, and `Cache-Control: no-store` response policy as the rest of AgentLens. Unsupported formats and hidden or unknown stable IDs return `404`. Downloads use fixed filenames so identifying values are not copied into response headers.

## Export contract

Format version 1 uses the fixed `aggregate-only` redaction profile. JSON contains `format_version`, the redaction declaration, session facts, aggregate turn/usage/model/tool/pressure facts, evidence-gap counts, and a bounded list of reason codes. Markdown presents the same facts; unavailable nullable usage totals appear as `null` in JSON and `Unavailable` in Markdown. Given the same indexed projection, both representations are byte-for-byte deterministic: they include no export time or inferred live state.

The only session-identifying value included is AgentLens's pseudonymous stable ID. It is required to associate the report with the same source-scoped session after a complete index rebuild. Agent kind is restricted to `codex` or `other`; indexed session and source states are also allowlisted. Model names, native session IDs, titles, projects, timestamps, host names, operating-system user names, paths, and account identifiers are not exported.

The export explicitly excludes prompts and messages, source code, tool arguments and outputs, raw source records, filesystem paths, credentials, and user annotations. Aggregate token and event counts can still be sensitive; review a report before sharing it outside the intended audience.

Reason codes are selected from a fixed set of at most 14 values. They cover observed interruptions, unclosed or terminal-only turns, requested-model changes, explicitly observed served-model fallback, usage resets/missing/inconsistent values, explicit tool failure or unknown outcome, missing pressure/timestamp evidence, and an incomplete source projection. Counts and explanations describe recorded evidence only. They do not establish task failure, current activity, quality, or provider allowance.

## Reconstruction and state

Reports are computed on request from normalized records derived from immutable raw sources. They are not written into copied agent homes or stored as authoritative artifacts. Deleting and rebuilding the disposable SQLite index from the same sources reconstructs the report and stable ID.

No schema migration, refresh, or rebuild is introduced by this export. Older indexes need the parser-version 7 refresh described in the turn and diagnostics documentation before all prerequisite evidence is available. Restart the web server after upgrading the application.

User annotations are intentionally absent. If annotation support is added, annotations must live in durable application-owned state rather than the disposable index or copied source homes, and any future export must opt into and redact them explicitly.

Synthetic regression coverage places adversarial strings in agent metadata, prompts, source code, tool bodies, paths, host/user fields, model data, credentials, and raw records. It verifies that neither representation discloses those values, that output is deterministic and bounded, that the download links render, and that authentication and hidden-session policies apply.
