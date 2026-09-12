# Source adapter boundaries

Agent Lens stores generic agents, instances, sessions, messages, tool calls, and usage events. Source formats are translated before database orchestration or presentation rules consume them.

The adapter boundary lives under `src/sources/`:

- `mod.rs` defines source-neutral discovery results, normalized record variants, source layouts, and message-presentation dispatch.
- `codex.rs` owns the `.codex` host/user and direct-home contracts, active/archive locations, symlink-aware discovery, parser version, two-pass JSONL reading, session metadata and lifecycle reduction, message/tool/usage interpretation, Codex-only hidden-model policy, and recognized setup/notice markers.
- `ingest.rs` owns rebuildable-index concerns: transaction and savepoint boundaries, checkpoints, instance/session upserts, normalized SQL writes, replacement rollback, pruning, and report totals. It receives normalized records and does not inspect Codex JSON fields.
- `web.rs` queries the indexed `agents.kind` and asks the source dispatcher for optional message presentation. Unknown kinds receive ordinary open message rendering. Codex-only model suppression is conditional on that same kind, so similarly named models from other agents remain visible.

Codex is still the only configured discovery adapter. Adding another source requires its own module and explicit dispatch rather than adding foreign raw-field checks to ingestion or web code. Unknown raw fields and record types remain local non-fatal omissions, and useful raw JSON remains attached to normalized records where the schema supports it.

## Compatibility and verification

This refactor does not change normalized parsing semantics, so the Codex parser version remains 2 and existing indexes require neither refresh nor rebuild. Existing synthetic regression coverage verifies discovery failures, symlinks, malformed and partial lines, schema variants, lifecycle ordering, replacement rollback, tool pairing, usage, rendered initial/resumed transcripts, and ordinary conversation. Additional cross-source regressions verify that Codex setup markers and `codex-auto-review` suppression do not affect an unknown agent kind.

The ingest path was remeasured with the existing synthetic 60,000-message, 65.2 MB rollout. Three warm-cache full rebuilds used separate SQLite outputs and the same source. The pre-adapter bounded importer (commit `409f1e3`; later R1.2/R1.3 commits did not change ingestion) had a 6.652-second median and 14,540–14,820 KiB sampled peak RSS. The adapter implementation had a 6.276-second median and 14,896–14,964 KiB peak RSS. The elapsed ranges overlapped and sampled memory remained within about 1% at the upper bound, showing no material throughput or memory regression. These local warm-cache measurements are not general latency guarantees.
