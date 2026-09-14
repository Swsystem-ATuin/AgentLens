# Safe full rebuilds

Run a full rebuild when the disposable index needs to be recreated from its raw sources:

```bash
cargo run -- rebuild --data data
cargo run -- rebuild --data /path/to/.codex
cargo run -- rebuild --data /path/to/.hermes
```

The data path may contain a multi-origin `host/user/` hierarchy with `.codex`, `.gemini`, and/or `.hermes` homes, or be one agent home directly. Direct-home rebuilds intentionally omit host and user fields from the sessions list and transcript. A later refresh must use the same layout and direct adapter; rebuild again to switch either.

Rebuild creates a uniquely named staging database beside the configured index and imports into that database. The existing index is not modified during discovery or ingestion. Before publication, Agent Lens runs SQLite quick-integrity and foreign-key checks, verifies that no ingest run remains unfinished, checkpoints the staging WAL, and closes its connections. It then checkpoints the existing index, verifies that its WAL and shared-memory sidecars are no longer in use, and renames the validated database over it.

Discovery, traversal, transaction, validation, checkpoint, or publication failures leave the previous index in place. Ordinary failures also remove the staging database and its sidecars. Terminating the process abruptly can leave files named like `.agent-lens.sqlite.rebuild-PID-SEQUENCE`; they are not used by the application and can be removed after confirming that no rebuild process is running. Raw source files are never changed.

## Web server procedure

Stop the Agent Lens web server before running `rebuild`, then start it again after the command succeeds. A staged import may be built while readers use the old database, but publication deliberately fails if another process still has the existing SQLite WAL database open. This prevents a long-lived server connection from continuing to serve the old database after its pathname has been replaced.

Use `refresh` when the server must remain available. Refresh updates the current database transactionally, so existing readers retain their snapshot and later requests see the committed update.

No schema migration is required for this behavior. Updating the executable changes subsequent rebuilds; it does not require an immediate rebuild.

## Verification and performance

Synthetic regressions cover successful replacement, failed-ingest preservation and cleanup, abandoned staging cleanup, SQLite integrity and foreign-key checks, and refusal to publish over an open index. The failed-ingest regression fails with the former delete-first implementation because the previous session is removed before discovery fails.

The 2026-09-12 performance review compared release binaries before and after this change using 40 synthetic sessions containing 60,000 messages and about 68 MiB of raw JSONL. Three sequential warm-cache rebuilds used the same source tree and separate SQLite paths. A `/proc` sampler recorded each process's peak resident memory; no private data was used.

| Implementation | Median elapsed | Observed range | Peak RSS range |
| --- | ---: | ---: | ---: |
| Delete-first rebuild | 5.510 s | 5.366–5.573 s | 15,904–15,968 KiB |
| Staged and validated rebuild | 6.275 s | 6.236–6.284 s | 20,084–20,120 KiB |

The staged rebuild was about 13.9% slower at the median and used about 4.1 MiB more peak resident memory in this workload. At that benchmark revision, ingestion used one batched transaction and retained the decoded records for one source file at a time. The added time and memory occurred during publication validation, which scans the completed database for structural and foreign-key errors; staging did not duplicate the index in process memory. Peak RSS sampling was polling-based and may miss brief maxima. Current ingestion uses a [bounded two-pass design](ingestion-memory.md).
