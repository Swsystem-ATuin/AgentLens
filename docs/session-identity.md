# Session identity and titles

AgentLens URLs use an application-defined public session ID, never the disposable SQLite `sessions.id` row number. The ID is a prefixed BLAKE3 digest over length-delimited identity fields:

1. A source identity is derived from the agent kind, host, operating-system user, and configured logical source-home path.
2. A session identity is derived from that source identity and the agent-native session ID.

The result is reproducible when the same logical source is rebuilt, including when a directory symlink is retargeted behind the same path. The same native ID in two source homes produces different public IDs. Relocating or renaming a configured source home intentionally creates a new source identity and therefore new URLs.

The SQLite `source_key` and `stable_id` columns are projections and are safe to rebuild from raw sources. Existing indexes are upgraded and backfilled automatically when opened.

## Title precedence

For each rollout-discovered Codex session, the displayed title uses the first available value:

1. A valid `thread_name` for the matching `id` in optional `.codex/session_index.jsonl`. If an ID appears more than once, the last valid entry wins.
2. The first ordinary user message in the rollout, after excluding recognized setup blocks and notices, normalized to single spaces and limited to 80 Unicode characters.
3. The agent-native session ID.

Advisory names are normalized to single spaces and limited to 160 Unicode characters. Empty names or names containing non-whitespace control characters are invalid. Malformed JSON, partial trailing records, missing fields, unknown fields, and read failures are ignored locally; later valid records remain usable.

Rollouts remain authoritative for discovery. `session_index.jsonl` can rename an indexed session but cannot create one. Every refresh reapplies advisory names even when all rollout files are unchanged, so changing or removing the optional index updates titles without forcing transcript reimport.

For Gemini CLI, a valid recording `summary` is the advisory title, followed by the first user message and then the native session ID. Gemini titles use the same whitespace normalization, 80-character conversation/summary bound, and control-character rejection. The source kind remains part of identity, so a Gemini chat and Codex rollout may safely share the same native ID.

## Ingestion and performance

Identity hashing retains only fixed-size digest state. Conversation fallback construction stops at its character limit. The optional advisory file is read one record at a time and each matching title is applied with the existing `(instance_id, external_id)` session index; it is not retained as a file-sized application map. Refresh performs one indexed fallback-title update per discovered source instance and one indexed lookup per valid advisory record. Existing transaction and per-rollout savepoint boundaries are unchanged.

The existing generated 60,000-message, 65,220,094-byte single-rollout corpus was rebuilt three alternating times with the bounded-ingestion binary from commit `409f1e3` and with this change. The baseline median was 6.072 seconds (6.070–6.390) with 14,820–14,868 KiB sampled peak RSS. This implementation's median was 6.242 seconds (6.156–6.331) with 14,796–14,992 KiB sampled peak RSS. The roughly 2.8% median elapsed difference and overlapping memory ranges show no material resource regression in this warm-cache local measurement. The corpus was generated placeholder data and contained no private content or advisory index; polling may miss brief memory peaks.

After upgrading, run:

```bash
cargo run -- refresh --data data
```

The schema migration itself needs no rebuild. Parser version 3 causes existing rollouts to be reimported once so conversation-derived fallbacks are available. Restart a running server to use stable-ID routes and display the new titles.
