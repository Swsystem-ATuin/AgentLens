# Hermes Agent adapter

Hermes Agent homes are discovered beneath:

```text
data/{hostname}/{os_user}/.hermes/
```

A single `.hermes/` directory can also be supplied directly. Directory symbolic links are supported under the same read-only source contract as the other adapters. The adapter reads three kinds of evidence:

- `state.db`, the canonical Hermes session and message store;
- `profiles/*/state.db`, the isolated canonical stores for named profiles;
- `cron/output/*/*.md`, completed scheduler run artifacts, including metadata-only audit records; and
- `sessions/request_dump_*.json`, provider-failure request snapshots.

This contract was verified against Hermes schema version 23 and the upstream [session-storage](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/developer-guide/session-storage.md) and [cron internals](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/developer-guide/cron-internals.md) documentation on 2026-09-23. The SQLite reader checks required columns and selects optional columns only when present, so additive fields and older schemas remain localized to the adapter.

## Canonical sessions

Each `state.db` is opened read-only and held in one deferred SQLite transaction. This produces a consistent snapshot while Hermes continues writing in WAL mode and does not copy, checkpoint, or mutate the source database. Agent Lens creates one source checkpoint for the database and one indexed activity for each Hermes session. Named-profile identities include their profile scope so equal native IDs cannot collide.

Agent Lens retains session source, title, model, lifecycle timestamps and reason, working directory, parent identity, profile, archive/pin state, messages, tool calls/results, and aggregate usage. Transport routing, user/chat identifiers, model configuration, system prompts, billing endpoints, handoff errors, encrypted reasoning, and unknown columns are not copied into SQLite.

Active messages and non-destructive compaction archives (`active=1 OR compacted=1`) are retained. Rewound rows (`active=0, compacted=0`) and `display_kind=hidden` rows are omitted. Unknown fields and unsupported message roles are ignored without aborting the database import. User messages open turns; system messages normalize to developer messages; assistant tool calls pair with later tool-role results through their call IDs.

Hermes records cache reads and writes separately from uncached input. Agent Lens reports total input as uncached input plus cache reads plus cache writes, reports cache reads in its cached-input field, and adds total input to output for the overall token total. Cache-write detail has no separate normalized field. Missing or zero aggregate usage remains unavailable.

The source checkpoint is a BLAKE3 digest of the selected logical session and message projection, not the mutable `state.db` file bytes. A WAL checkpoint or page-layout rewrite therefore does not cause a false reimport, while any selected transcript or metadata change does. The digest pass and normalized reads share the same SQLite snapshot. Session summaries are retained in memory, but message bodies are streamed during hashing and loaded one session at a time during normalization.

## Cron output activities

Hermes installations can keep scheduler execution lifecycle in `cron/executions.db` rather than creating `source='cron'` rows in `state.db`. Run documents are persisted separately under `cron/output/<job-id>/<local-timestamp>.md`. Agent Lens imports each Markdown artifact as one completed Hermes activity, uses its first level-one heading as the bounded title when available, and retains the complete artifact as an assistant message. It distinguishes body output from documents containing only the leading heading and scheduler metadata block; unknown metadata labels are tolerated.

The sessions browser hides metadata-only cron artifacts and canonical `source='cron'` sessions without non-empty assistant or tool output by default. This includes silent script ticks and monitor `no_change` records. They remain indexed as audit evidence, retain direct transcript URLs, and appear when “All cron runs” is selected. Ordinary Hermes sessions and cron runs with body output are unaffected.

The filename timestamp has no UTC offset, so Agent Lens preserves it as a zone-unspecified source timestamp. `cron/executions.db` and `cron/jobs.json` are not currently indexed: failed or running executions without an output artifact therefore remain unavailable, and a renamed job does not rewrite historical titles.

## Request-failure snapshots

Each `request_dump_*.json` remains a separate request-attempt activity. Its stable external identity combines the recorded Hermes session ID with a digest of the dump filename so retries do not overwrite one another. Agent Lens retains the snapshot timestamp, requested model, supported OpenAI Responses or Anthropic message/tool shapes, and a fixed failure notice derived from an allowlist of reason codes.

Request dumps can contain plaintext credentials and provider responses. Agent Lens never copies request headers, URLs, error objects, response text, encrypted reasoning, or arbitrary unknown fields. Normalized raw JSON is reconstructed from selected transcript, tool, model, timestamp, and allowlisted failure-reason fields. Message, tool, canonical-session, and cron-output bodies are transcript evidence and can themselves contain private material; protect the raw home and both Agent Lens databases accordingly.

## Refresh and rebuild

After publishing a complete `.hermes` home, run:

```bash
cargo run -- refresh --data data
```

For a directly supplied home, use `cargo run -- refresh --data /path/to/.hermes`. Hermes parser version 3 causes existing sources to reimport once, including cron output classification, without requiring an Agent Lens schema migration or full rebuild. Switching an existing direct-home index between agent types still requires `rebuild`.

Malformed databases, unsupported required schemas, invalid UTF-8 cron outputs, and malformed request snapshots are localized source errors. A failed replacement retains its previous indexed projection and retries on the next refresh.

## Performance review

Pre-change and post-change release binaries were measured against the same warm, read-only Hermes home and separate disposable indexes on 2026-09-23. Shell timing includes startup, import, integrity validation, and staged publication; no transcript values were printed. Peak resident memory was not available from the host tooling.

| Adapter | Files | Sessions | Messages | Tool calls | Elapsed |
| --- | ---: | ---: | ---: | ---: | ---: |
| Request snapshots only | 33 | 33 | 142 | 231 | 5.204 s |
| Canonical state, cron outputs, and snapshots | 305 | 420 | 1,323 | 3,919 | 6.828 s |

The expanded importer processed about 12.7 times as many sessions and 17 times as many tool calls with a 31.2% elapsed-time increase in this single warm-cache run. A following unchanged refresh logically hashed the canonical database, byte-hashed the other 304 artifacts, skipped all 305 sources, and imported no sessions. These local single-run measurements are not cold-storage or general latency guarantees.

Parser version 3 was measured separately with 4,000 generated cron artifacts across 2,000 jobs: half contained only scheduler metadata and half contained synthetic body output. The 538,000-byte source tree was rebuilt three alternating times with release binaries immediately before and after classification. The baseline median was 2.929 seconds (2.341–2.953); version 3 was 2.816 seconds (2.173–2.855). The ranges overlap, so the measurement establishes no material throughput regression. Adding one boolean to raw metadata increased the disposable index from 5,910,528 to 6,017,024 bytes (1.8%). Source reads, normalized row counts, and transaction boundaries are unchanged; classification makes one allocation-free pass over content already loaded for title and message import. The default list query retains the existing activity-order index and stops after 51 visible rows. No private source was read.
