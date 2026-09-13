# Incremental refresh

```bash
cargo run -- refresh --data data
cargo run -- --database /tmp/agent-lens.sqlite refresh --data data
cargo run -- refresh --data /path/to/.codex
```

`AGENT_LENS_DB` also selects the index. Refresh creates an index when missing and requires no authentication file. The data path may use the multi-origin `root/host/user/.codex` layout or name one `.codex` home directly. Direct-home indexes omit host/user filters, columns, and transcript metadata. Refresh preserves the layout recorded by the previous successful rebuild or refresh; use `rebuild` to change layouts. Use the same working directory and source-path spelling across runs: source homes and file paths are currently stored as supplied, not canonicalized identities.

Every discovered rollout is checked against a source-file checkpoint in SQLite. Checkpoints contain a `blake3:`-prefixed digest of the exact file bytes, a parser version, and parse status. A matching checksum skips parsing only when the parser version is current, status is `ok`, and an indexed session references that source. Session lifecycle status is not used: a completed task may resume later.

Refresh hashes every successfully indexed candidate, even if size and modification time match. There is no metadata-only shortcut or separate `--verify` mode. This detects historical edits that preserve size and timestamps. Unchanged verified files require one checksum read. New/unverified imports use two record-streaming passes; verified changed files require a checksum read followed by both import passes. See [bounded rollout ingestion](ingestion-memory.md) for memory, file-I/O, query, and benchmark details.

## Existing indexes and failure handling

Opening an old index automatically adds nullable checkpoint columns and supporting source/session and tool-pairing indexes. Rows without a checksum or current parser version receive a one-time reimport. Merely attaching a new checksum to old indexed records would incorrectly certify their completeness.

A single outer transaction batches the run, with one savepoint per file. A successful replacement updates session metadata, replaces messages/tools/usage, and stores its checkpoint together. Session IDs are preserved for the same instance and native session ID. File failures roll back that file's changes, record an error, and allow other files to proceed. Failure to finalize or commit the run rolls back the entire run.

Partial new files retain valid records and are marked `partial`; they cannot be skipped. Partial replacements of files still present retain the previously indexed transcript and record a hard file error. Missing-source cleanup still applies if a moved replacement cannot be imported: a transcript whose indexed source is gone is removed. Repairing the source allows the next refresh to replace it. Malformed-line diagnostics contain exact line numbers and a generic reason, without quoting source content. Totals remain exact while persisted examples are capped at 20. CLI summaries report hard file errors, partial files, and malformed lines separately; initially imported partial files are also included in imported sessions, with details in `source_files.parse_status` and `parse_error`.

The recorded hash certifies a successfully committed file projection only when its status is `ok`. On a failed replacement, any existing hash still describes the previous projection, and the `error` status prevents skipping it. Parser changes affecting normalized data must bump the source adapter's `PARSER_VERSION`. Readable conversation-title derivation raised the Codex version to 3, so the first refresh with this release reimports every rollout whose checkpoint has an older version and updates both titles and earlier lifecycle projections.

## Source changes and supported operation

- Appends, truncations, and rewritten content replace the affected file's projection, including ordered tool results and usage events. A replacement with a different native session ID removes that path's previous session.
- A move to `archived_sessions` preserves the session when its former path no longer exists. The session points to its new source before cleanup removes the missing old source row.
- Two existing files claiming the same session identity are conflicts. The already indexed source wins; on a fresh index the first successfully imported source wins. Conflicting files are reported as errors and retried. Divergent duplicate sources require resolution in the upstream copying process.
- Missing source files are removed from the index after discovery/import, including their sessions, messages, tool calls/results, usage records, and source checkpoints/errors. This also handles removal of an entire host, user, home, or sessions directory. Cleanup covers only the requested direct `.codex` home or `root/host/user/.codex` scope; other roots and agent kinds are untouched. The CLI reports the numbers of sessions and source rows removed.
- A missing/unreadable root, directory inspection failure, traversal error, broken symbolic link, directory loop, or symbolic link to an individual file aborts the run and rolls back imports and deletions. A present, readable, empty root is an authoritative empty snapshot and removes its indexed sessions. Individual malformed files remain localized parse errors and do not block removal of unrelated missing files. Use a coherent, fully published copy; an empty mount point cannot be distinguished from an intentionally empty source tree.
- Directory symbolic links are followed at the root, host, user, `.codex`, active/archive area, and nested session-directory levels. Relative targets work too. Logical paths through the links remain the indexed identities, so retargeting a stable link to the next nightly backup retains unchanged-file checkpoints. Targets may be outside the source root and are read only. Keep link targets available and unchanged throughout refresh; broken links cancel the whole run instead of being treated as removed directories. No schema migration or rebuild is needed.
- Publish a coherent copied source tree before refreshing. Do not modify files in place during refresh. File size and modification time are checked around reading and again after importing; Unix also checks device, inode, and change time. Detected concurrent edits roll back the file and require retry. These checks do not provide a filesystem snapshot or prevent changes after the final check.

Refresh can run while the server reads the same SQLite database: readers see committed data, and subsequent requests see the completed refresh. Run one importer at a time. Existing long-lived read transactions can continue seeing their earlier snapshot. Restart the server to load updated application code. Browser reloads do not run ingestion.

Both rebuild and refresh store BLAKE3 hashes during successful imports. Full rebuild remains available with `cargo run -- rebuild --data data`. It builds and validates a staging database before replacing the old index, so failures preserve the last usable index. Stop the server before rebuilding because publication refuses to replace an open SQLite database; see [safe full rebuilds](rebuild.md). Raw sources are never changed by either command. No manual migration or full rebuild is required for missing-source cleanup: update the binary and run refresh. Supporting indexes for tool/usage cascades are created automatically when the database opens.

After refresh commits, new list and transcript requests cannot retrieve deleted sessions (direct URLs return 404). Already displayed browser pages are not remotely cleared. This is removal of indexed rows, not secure erasure of SQLite free pages, WAL files, or external backups.

## Regression verification

Run `bash scripts/check.sh`. Tests use temporary synthetic sources and the small redacted fixture, never private `data/`. Coverage includes direct-home rebuild/refresh and rendered hostless pages, layout mismatch rollback, missing hashes, legacy schema migration and identity backfill, parser-version invalidation, stable public IDs across rebuilds, source-scoped native-ID collisions, repeated skips, advisory-title refresh, checksum correctness, same-size/same-timestamp edits, resumed completed sessions, truncation, changed identities, archive moves, duplicate conflicts, partial JSON/UTF-8, rollback of failed replacements and failed finalization, retries, source immutability, and normalized refresh/rebuild parity for messages, tools, and usage.

Before implementation, the three initial refresh regressions failed on the existing duplicate-path/foreign-key behavior. They pass with transactional replacement and safe source-row updates.

Removal regressions additionally cover cascading content deletion, removed homes, root isolation, missing/invalid roots, traversal failures, rollback/retry, coexistence with parse errors, CLI removal counts, rebuild-generated checksums, and authenticated list/direct-URL visibility. The four initial removal/discovery regressions failed before the fix and pass afterward.

## Initial refresh performance review (2026-09-10)

Release builds before and after this change were measured on the same synthetic corpus: 40 sessions, 60,000 messages, 66,362,390 source bytes (63.3 MiB), with roughly 960 characters of synthetic content per message. Three sequential runs per case used warm filesystem caches, the same temporary SQLite location, and no concurrent build. Elapsed time includes CLI startup and commit; peak resident memory comes from child-process `wait4` usage on Linux. These measurements are workload-specific, not cold-storage predictions.

| Operation | Median elapsed | Peak RSS range |
| --- | ---: | ---: |
| Previous full rebuild | 5.237 s | 13,356 KiB |
| Full rebuild with checkpoints | 5.513 s | 13,512–13,740 KiB |
| Refresh, all 40 unchanged | 0.115 s | 13,512 KiB |
| Refresh, one appended session | 0.316 s | 13,328–13,584 KiB |

The new full rebuild was about 5.3% slower; an unchanged refresh was about 48 times faster than the new rebuild. The changed-file case appended one synthetic message per run and verified that exactly one session was imported and 39 were skipped.

Rebuild retains one batched transaction and reads each source once, hashing during the parse. Additional database work is constant per source: checkpoint/identity lookups, a savepoint, and checkpoint updates. The source-file session index avoids scanning all sessions for each lookup. Record insertion volume is unchanged. Unchanged refresh reads all source bytes once but performs no transcript parsing or message/tool/usage writes. A changed verified source is read twice; unchanged peers are read once. Source read volume follows these paths; physical disk I/O was not separately instrumented because the benchmark used warm caches.

This section describes the initial checkpoint implementation. Its per-file decoded-record collection was subsequently replaced by the [bounded two-pass importer](ingestion-memory.md); the newer benchmark measures one large rollout specifically to expose the former memory growth.

## Missing-source cleanup performance review (2026-09-10)

Release builds immediately before and after cleanup were compared on 40 synthetic sources containing 60,000 messages, 6,000 tool calls, and 6,000 usage events (67,817,590 source bytes). Three sequential runs per operation used warm filesystem caches and the same temporary SQLite location, without concurrent compilation. Medians include process startup; Linux `wait4` peak RSS includes process-launch overhead.

| Operation | Before cleanup | With cleanup |
| --- | ---: | ---: |
| Full rebuild | 6.130 s | 5.914 s |
| Refresh, all unchanged | 0.131 s | 0.132 s |
| Refresh, one source removed | Not supported | 0.149 s |

Rebuild ranges overlapped (5.891–6.292 s before; 5.869–6.332 s after), so this run does not establish a rebuild speed improvement. Recorded peak RSS for the rebuild/unchanged cases was 18,668 KiB before and 18,752 KiB after. Each measured deletion removed one session, 1,500 messages, 150 tool calls, 150 usage events, and its checkpoint; counts were verified directly in the synthetic index.

Cleanup retains the outer transaction and per-file import savepoints. It reads candidate paths in batches of at most 256, filters by the requested source-root contract, and inspects filesystem metadata for scoped sources. It reads no additional transcript contents. Candidate queries currently traverse the indexed Codex sources across roots, while metadata inspection and deletion remain scoped. Each removed source adds two parameterized deletes; foreign-key cascades remove session children. Session-key indexes for tools and usage are created automatically to avoid scanning those tables for every deleted session. Full rebuild source reading, parser allocations, and per-file decoded-record memory remain unchanged. Physical disk I/O was not separately measured under the warm-cache setup.

## Directory symlink verification and performance (2026-09-11)

The new directory-link regression failed with the previous unsupported-source-directory error and passes with this change. Synthetic tests cover links at every discovery level, active and archived areas, nested directories, relative targets, backup retargeting with stable checkpoints, source immutability, missing-file cleanup, and rollback for dangling links and loops. Individual file links remain rejected. `bash scripts/check.sh` passes, and local README/refresh documentation links resolve.

Full rebuilds were compared using release binaries before and after the change, with 40 synthetic sessions, 60,000 messages, and 64,982,390 source bytes. Three sequential runs per binary used warm caches, ordinary source directories, the same temporary SQLite path, and no concurrent build. Median elapsed time was 5.566 s before (5.556–5.808 s) and 5.811 s after (5.465–5.932 s), a 4.4% increase with overlapping ranges. The benchmark driver's cumulative child-process peak RSS stayed at 17,920 KiB across both groups; this is not a per-run memory comparison.

Transaction boundaries, SQL query volume, transcript reads, and parser allocations are unchanged. Directory links add target metadata resolution and WalkDir's ancestor-loop checks; ordinary discovery retains directory-entry type checks. Traversal remains streaming and cleanup still buffers at most 256 paths. Actual backup storage latency and directory layout may dominate costs; these warm-cache measurements do not predict remote-backup performance.

## Direct-home performance review (2026-09-12)

Release builds immediately before and after direct `.codex` support were compared on 40 synthetic sessions containing 60,000 messages and 65,223,640 source bytes. Three alternating warm-cache rebuilds per binary used the same host/user source tree and separate SQLite paths. Elapsed time includes process startup and publication; a `/proc` sampler recorded peak resident memory. No private fixture was read.

| Implementation | Median elapsed | Observed range | Peak RSS range |
| --- | ---: | ---: | ---: |
| Before direct-home support | 6.395 s | 6.367–6.416 s | 19,676–19,844 KiB |
| After direct-home support | 6.419 s | 6.352–6.673 s | 19,704–19,864 KiB |

The 0.4% median difference and memory ranges are within the observed run-to-run spread. Normal host/user discovery still visits the same directories and source files. The change adds one bounded lookup of the previous layout and one layout value per ingest run; parsing, hashing, per-source queries, transaction boundaries, decoded-record allocation, and source-file I/O are unchanged. Direct-home mode skips the two directory levels used to discover hosts and users. Peak RSS sampling was polling-based and may miss brief maxima.

## Parser-resilience verification (2026-09-12)

Synthetic regressions cover unknown top-level and payload fields, unknown record/content types, absent optional values, active and archived rollouts, malformed JSON at interior lines and file tails, partial UTF-8, missing metadata/IDs, valid-record survival, raw JSON retention, exact sanitised line diagnostics, refresh retry behavior, and separate CLI totals for hard failures, partial files, and malformed lines. The aggregate-diagnostic regression failed before this change because no partial or malformed-line counts existed.

The reporting change reads the length of the malformed-line diagnostics already collected for each source and carries two integer counters in the ingest report. Transaction/savepoint boundaries, SQL query volume, source reads, decoded-record and diagnostic allocations, and normalized writes were unchanged, so no separate performance benchmark was run for that reporting-only change. The unbounded collections noted at the time were subsequently replaced by the [bounded two-pass importer](ingestion-memory.md).
