# Incremental refresh

```bash
cargo run -- refresh --data data
cargo run -- --database /tmp/agent-lens.sqlite refresh --data data
```

`AGENT_LENS_DB` also selects the index. Refresh creates an index when missing and requires no authentication file. Use the same working directory and source-path spelling across runs: source homes and file paths are currently stored as supplied, not canonicalized identities.

Every discovered rollout is checked against a source-file checkpoint in SQLite. Checkpoints contain a `blake3:`-prefixed digest of the exact file bytes, a parser version, and parse status. A matching checksum skips parsing only when the parser version is current, status is `ok`, and an indexed session references that source. Session lifecycle status is not used: a completed task may resume later.

Refresh hashes every successfully indexed candidate, even if size and modification time match. There is no metadata-only shortcut or separate `--verify` mode. This detects historical edits that preserve size and timestamps. New/unverified files are hashed during parsing; changed verified files require a checksum read followed by a parsing read. Hashing uses bounded buffers. The existing parser still holds the decoded records for one file in memory.

## Existing indexes and failure handling

Opening an old index automatically adds nullable checkpoint columns and an index for source/session lookups. Rows without a checksum or current parser version receive a one-time reimport. Merely attaching a new checksum to old indexed records would incorrectly certify their completeness.

A single outer transaction batches the run, with one savepoint per file. A successful replacement updates session metadata, replaces messages/tools/usage, and stores its checkpoint together. Session IDs are preserved for the same instance and native session ID. File failures roll back that file's changes, record an error, and allow other files to proceed. Failure to finalize or commit the run rolls back the entire run.

Partial new files retain valid records and are marked `partial`; they cannot be skipped. Partial replacements retain the previously indexed transcript and record an error. Repairing the source allows the next refresh to replace it. Malformed-line diagnostics contain line numbers and a generic reason, without quoting source content. CLI error totals count failed files; initially imported partial files are included in imported sessions, with details in `source_files.parse_status` and `parse_error`.

The recorded hash certifies a successfully committed file projection only when its status is `ok`. On a failed replacement, any existing hash still describes the previous projection, and the `error` status prevents skipping it. Parser changes affecting normalized data must bump `PARSER_VERSION` in `src/ingest.rs`.

## Source changes and supported operation

- Appends, truncations, and rewritten content replace the affected file's projection, including tool outputs and usage events. A replacement with a different native session ID removes that path's previous session.
- A move to `archived_sessions` preserves the session when its former path no longer exists. The previous source row remains as provenance; the session points to its new source.
- Two existing files claiming the same session identity are conflicts. The already indexed source wins; on a fresh index the first successfully imported source wins. Conflicting files are reported as errors and retried. Divergent duplicate sources require resolution in the upstream copying process.
- Missing source files are not pruned. Use a full rebuild when the index must exactly reflect deletions. Discovery still has the existing limitation of silently skipping some missing/unreadable directories (roadmap R0.2).
- Publish a coherent copied source tree before refreshing. Do not modify files in place during refresh. File size and modification time are checked around reading and again after importing; Unix also checks device, inode, and change time. Detected concurrent edits roll back the file and require retry. These checks do not provide a filesystem snapshot or prevent changes after the final check.

Refresh can run while the server reads the same SQLite database: readers see committed data, and subsequent requests see the completed refresh. Run one importer at a time. Existing long-lived read transactions can continue seeing their earlier snapshot. Restart the server to load updated application code. Browser reloads do not run ingestion.

Full rebuild remains available with `cargo run -- rebuild --data data`. Stop the server before rebuilding: rebuild still removes the index and its sidecars, and does not preserve the old index on failure (roadmap R0.1). Raw sources are never changed by either command.

## Regression verification

Run `bash scripts/check.sh`. Tests use temporary synthetic sources and the small redacted fixture, never private `data/`. Coverage includes missing hashes, legacy schema migration, parser-version invalidation, repeated skips, checksum correctness, same-size/same-timestamp edits, resumed completed sessions, truncation, changed identities, archive moves, duplicate conflicts, partial JSON/UTF-8, rollback of failed replacements and failed finalization, retries, source immutability, and normalized refresh/rebuild parity for messages, tools, and usage.

Before implementation, the three initial refresh regressions failed on the existing duplicate-path/foreign-key behavior. They pass with transactional replacement and safe source-row updates.

## Performance review (2026-09-10)

Release builds before and after this change were measured on the same synthetic corpus: 40 sessions, 60,000 messages, 66,362,390 source bytes (63.3 MiB), with roughly 960 characters of synthetic content per message. Three sequential runs per case used warm filesystem caches, the same temporary SQLite location, and no concurrent build. Elapsed time includes CLI startup and commit; peak resident memory comes from child-process `wait4` usage on Linux. These measurements are workload-specific, not cold-storage predictions.

| Operation | Median elapsed | Peak RSS range |
| --- | ---: | ---: |
| Previous full rebuild | 5.237 s | 13,356 KiB |
| Full rebuild with checkpoints | 5.513 s | 13,512–13,740 KiB |
| Refresh, all 40 unchanged | 0.115 s | 13,512 KiB |
| Refresh, one appended session | 0.316 s | 13,328–13,584 KiB |

The new full rebuild was about 5.3% slower; an unchanged refresh was about 48 times faster than the new rebuild. The changed-file case appended one synthetic message per run and verified that exactly one session was imported and 39 were skipped.

Rebuild retains one batched transaction and reads each source once, hashing during the parse. Additional database work is constant per source: checkpoint/identity lookups, a savepoint, and checkpoint updates. The source-file session index avoids scanning all sessions for each lookup. Record insertion volume is unchanged. Unchanged refresh reads all source bytes once but performs no transcript parsing or message/tool/usage writes. A changed verified source is read twice; unchanged peers are read once. Source read volume follows these paths; physical disk I/O was not separately instrumented because the benchmark used warm caches.

Checksum buffers are bounded (1 MiB for verification); parsing reuses its line buffer and retains the existing per-file decoded-record collection. No corpus-wide transcript collection was introduced. Peak memory in this corpus remained similar; exceptionally large individual rollouts still require the separate bounded-parser work in roadmap R1.1.
