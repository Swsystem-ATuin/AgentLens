# Incremental refresh

```bash
cargo run -- refresh --data data
cargo run -- --database /tmp/agent-lens.sqlite refresh --data data
cargo run -- refresh --data /path/to/.codex
cargo run -- refresh --data /path/to/.gemini
cargo run -- refresh --data /path/to/.hermes
```

`AGENT_LENS_DB` also selects the index. Refresh creates an index when missing and requires no authentication file. The data path may use a multi-origin `root/host/user/` layout containing `.codex`, `.gemini`, and/or `.hermes` homes, or name one agent home directly. Direct-home indexes omit host/user filters, columns, and transcript metadata. Refresh preserves the layout and direct adapter recorded by the previous successful rebuild or refresh; use `rebuild` to change them. Use the same working directory and source-path spelling across runs: source homes and file paths are currently stored as supplied, not canonicalized identities.

Every discovered rollout, chat, Hermes canonical database, cron output, or request snapshot is checked against a source checkpoint in SQLite. Ordinary-file checkpoints contain a `blake3:`-prefixed digest of the exact bytes. Hermes `state.db` checkpoints instead digest the selected logical rows from one consistent SQLite read transaction, so WAL checkpoints and page-layout changes do not create false transcript changes. Checkpoints also record an adapter-specific parser version and parse status. A matching current, successful checksum skips parsing. Session lifecycle status is not used: a completed task may resume later.

Refresh hashes every successfully indexed candidate, even if size and modification time match. There is no metadata-only shortcut or separate `--verify` mode. This detects historical edits that preserve size and timestamps. Unchanged verified ordinary files require one checksum read. New/unverified ordinary imports read a source twice; verified changed ordinary files require a checksum read followed by both import passes. Codex streams the passes, Gemini retains its active message set to apply recording mutations, and Hermes retains one decoded request snapshot or cron artifact at a time. Canonical Hermes databases stream their logical digest and then load one session at a time from the same source transaction. See [ingestion memory](ingestion-memory.md) and the adapter-specific documents for memory, file-I/O, query, and benchmark details.

## Existing indexes and failure handling

Opening an old index automatically adds nullable checkpoint columns, ordered usage/accounting, pressure and turn/model storage, nullable tool-to-turn associations, and supporting source/session/tool/usage/turn indexes. Legacy usage and tool rows remain order-unknown evidence and deliberately receive no calculated usage summary, pressure values, or invented turn association. Rows without a checksum or current parser version receive a one-time reimport. Codex parser version 7 recovers exact tool-result order, computes token deltas/summaries, reconstructs source-ordered turns, and retains context/rate snapshots. Hermes parser version 2 adds canonical databases and cron outputs. Merely attaching a new checksum to old indexed records would incorrectly certify their completeness.

A single outer transaction batches the run, with one savepoint per file. A successful replacement updates session metadata, replaces messages/tools/ordered usage and its derived session summary, and stores its checkpoint together. Session IDs are preserved for the same instance and native session ID. File failures roll back that file's changes, record an error, and allow other files to proceed. Failure to finalize or commit the run rolls back the entire run.

Partial new files retain valid records and are marked `partial`; they cannot be skipped. Partial replacements of files still present retain the previously indexed transcript and record a hard file error. Missing-source cleanup still applies if a moved replacement cannot be imported: a transcript whose indexed source is gone is removed. Repairing the source allows the next refresh to replace it. Malformed-line diagnostics contain exact line numbers and a generic reason, without quoting source content. Totals remain exact while persisted examples are capped at 20. CLI summaries report hard file errors, partial files, and malformed lines separately; initially imported partial files are also included in imported sessions, with details in `source_files.parse_status` and `parse_error`.

The recorded hash certifies a successfully committed source projection only when its status is `ok`. On a failed replacement, any existing hash still describes the previous projection, and the `error` status prevents skipping it. Parser changes affecting normalized data must bump the source adapter's `PARSER_VERSION`. Context/rate retention raises the Codex version to 7, while canonical Hermes support raises that adapter to version 2; the first refresh reimports older checkpoints and commits each adapter's new projection atomically.

## Source changes and supported operation

- Appends, truncations, and rewritten content replace the affected file's projection, including ordered turns/model evidence, tool results, usage, and context/rate observations. A replacement with a different native session ID removes that path's previous session.
- A move to `archived_sessions` preserves the session when its former path no longer exists. The session points to its new source before cleanup removes the missing old source row.
- Two existing files claiming the same session identity are conflicts. Selection is deterministic: an active `sessions` source precedes an `archived_sessions` source, then the lexically earliest logical path wins within an area. Directory traversal sorts every level, and refresh can replace an older noncanonical winner so repeated refresh and a fresh rebuild converge. Losing files are retained as source errors and retried; divergent duplicates still require resolution in the upstream copying process.
- Missing source files are removed from the index after discovery/import, including their sessions, turns/model evidence, messages, tool calls/results, usage records, and source checkpoints/errors. This also handles removal of an entire host, user, home, or session/chat directory. Cleanup covers only the requested direct agent home or host/user root; unrelated roots remain untouched. The CLI reports the numbers of sessions and source rows removed.
- A missing/unreadable root, directory inspection failure, traversal error, broken symbolic link, directory loop, or symbolic link to an individual file aborts the run and rolls back imports and deletions. A present, readable, empty root is an authoritative empty snapshot and removes its indexed sessions. Individual malformed files remain localized parse errors and do not block removal of unrelated missing files. Use a coherent, fully published copy; an empty mount point cannot be distinguished from an intentionally empty source tree.
- Directory symbolic links are followed at the root, host, user, agent-home, source-area, and nested session-directory levels. Relative targets work too. Logical paths through the links remain the indexed identities, so retargeting a stable link to the next nightly backup retains unchanged-file checkpoints. Targets may be outside the source root and are read only. Keep link targets available and unchanged throughout refresh; broken links cancel the whole run instead of being treated as removed directories. No schema migration or rebuild is needed.
- Publish a coherent copied source tree before refreshing. Do not modify ordinary files in place during refresh. File size and modification time are checked around reading and again after importing; Unix also checks device, inode, and change time. Detected concurrent edits roll back the file and require retry. Hermes `state.db` is the deliberate exception: its WAL-safe read transaction provides the source snapshot while a live Hermes process writes concurrently. These checks do not provide a multi-file filesystem snapshot or prevent changes after the final check.

### Snapshot publication contract

For periodic copies, write each complete snapshot to a new versioned directory that AgentLens is not currently traversing. Finish and verify the copy before atomically retargeting a stable directory symbolic link to that immutable directory. Retarget before starting refresh, then keep both the link and selected target present and unchanged until refresh completes. Publishing by renaming a fully prepared directory into a previously absent path is also safe; replacing individual files one by one in the live tree is not an atomic snapshot.

For a directly copied live `.codex` home, append-only writers are tolerated only when a complete read remains stable. AgentLens hashes the entire file, scans it again for normalized writes, and verifies metadata before, between, and after those passes. A partial JSON or UTF-8 tail is recorded on a new source and retried, while a partial replacement rolls back to the prior projection. A later append, truncation, or historical rewrite is detected by a full checksum read; no prefix checkpoint or size/mtime probe is trusted. Because per-file validation cannot make a multi-file tree atomic, use the immutable-directory protocol whenever additions, removals, and archive moves must appear as one coherent snapshot.

Refresh can run while the server reads the same SQLite database: readers see committed data, and subsequent requests see the completed refresh. Run one importer at a time. Existing long-lived read transactions can continue seeing their earlier snapshot. Restart the server to load updated application code. Browser reloads do not run ingestion.

Each committed candidate check records `source_files.observed_at`, including an unchanged checksum and a localized source error. The [opt-in live reader](live-reading.md) uses that observation separately from the newest completed ingest-run timestamp and the session lifecycle status. Existing indexes add the nullable column automatically; one refresh populates it.

Both rebuild and refresh store BLAKE3 hashes during successful imports. Full rebuild remains available with `cargo run -- rebuild --data data`. It builds and validates a staging database before replacing the old index, so failures preserve the last usable index. Stop the server before rebuilding because publication refuses to replace an open SQLite database; see [safe full rebuilds](rebuild.md). Raw sources are never changed by either command. No manual migration or full rebuild is required for missing-source cleanup: update the binary and run refresh. Supporting indexes for tool/usage cascades are created automatically when the database opens.

After refresh commits, new list and transcript requests cannot retrieve deleted sessions (direct URLs return 404). Already displayed browser pages are not remotely cleared. This is removal of indexed rows, not secure erasure of SQLite free pages, WAL files, or external backups.

## Regression verification

Run `bash scripts/check.sh`. Tests use temporary synthetic sources and the small redacted fixture, never private `data/`. Coverage includes direct-home rebuild/refresh and rendered hostless pages, layout mismatch rollback, missing hashes, legacy schema migration and identity backfill, parser-version invalidation, stable public IDs across rebuilds, source-scoped native-ID collisions, repeated skips, advisory-title refresh, checksum correctness, same-size/same-timestamp edits, resumed completed sessions and turn history, truncation, changed identities, archive moves, duplicate conflicts, partial JSON/UTF-8, rollback of failed replacements and failed finalization, retries, source immutability, and normalized refresh/rebuild parity for messages, tools, turns, ordered usage evidence, and derived usage summaries.

Duplicate coverage additionally seeds a noncanonical winner, publishes the lexically preferred source, and verifies that refresh switches to it. A repeated refresh and a clean rebuild then select the same path and produce the same normalized projection.

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

## R4.1 deterministic-order performance review (2026-09-13)

Release builds at baseline commit `c42896b` and with deterministic traversal/duplicate convergence were measured on the same synthetic direct-home corpus: 40 sessions, 60,000 messages, and approximately 59.8 MB of source JSONL. Three sequential warm-cache runs per implementation covered a full rebuild, a refresh with all files unchanged, and a refresh after appending one synthetic record to one file. Elapsed time includes CLI startup and commit. A 5 ms `/proc` sampler measured resident memory and process I/O; no private fixture was used.

| Operation | Before median | After median | Before peak RSS median | After peak RSS median |
| --- | ---: | ---: | ---: | ---: |
| Full rebuild | 6.135 s | 6.127 s | 13,856 KiB | 13,888 KiB |
| Refresh, all unchanged | 0.138 s | 0.141 s | 9,836 KiB | 9,828 KiB |
| Refresh, one file changed | 0.484 s | 0.492 s | 12,312 KiB | 12,256 KiB |

The elapsed and memory ranges overlap, so the measurements do not establish a meaningful performance change. Linux reported zero physical `read_bytes` for every warm-cache run. Sampled process `rchar`, which also includes SQLite and runtime reads, was approximately 870 MB for rebuild, 57–60 MB for unchanged refresh, and 69–74 MB for changed refresh in both builds; short refreshes can finish between samples, so it is not an exact source-byte counter.

Logical source I/O is unchanged and is the reliable contract: rebuild and new/unverified sources read each rollout twice, unchanged verified sources once, and changed verified sources three times. Sorting directory entries does not add source-file reads, normalized writes, or file-sized allocations. It buffers and sorts only the entries of the current directory; rollout records remain streamed. The existing large-rollout benchmark in [bounded rollout ingestion](ingestion-memory.md) covers the dominant memory behavior.

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
