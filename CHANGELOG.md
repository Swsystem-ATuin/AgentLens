# Changelog

All notable changes to this project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project intends to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Incremental refresh now traverses source directories deterministically and converges legacy duplicate winners on the active-area, lexically earliest logical source selected by a fresh rebuild. The snapshot publication, append validation, retry, I/O, memory, and performance contracts are documented and regression-tested.

- Rebuild and refresh summaries now distinguish hard file errors, partial files, and malformed-line totals. Parser regressions verify valid-record survival, exact sanitised line diagnostics, unknown fields, schema variants, malformed JSON, and partial UTF-8.

- Session lifecycle status now follows source-record order: later completion or interruption events replace earlier outcomes, while later turn/user activity reopens a terminal session to `unknown` without claiming it is currently running. Existing indexes are automatically reimported on refresh through a parser-version bump.

- Internal web failures now return a generic browser response and emit only allowlisted diagnostic fields on the server. Request tracing excludes URIs and authorization data so private query values, transcript content, credentials, and underlying error text are not logged.

- Full rebuilds now import into and validate a staging database before replacing the usable index. Failed rebuilds preserve the previous index, and publication refuses to replace a database still held open by the web server.

- Refresh and rebuild accept directory symlinks to nightly backups throughout source discovery and traversal, preserving logical source paths across backup rotation. Broken links and directory loops roll back refresh without pruning indexed sessions.

- Hide `codex-auto-review` model sessions, including prefixed variants, from the sessions list, model dropdown, and direct transcript URLs without deleting indexed data.

- Refresh removes missing source files and their indexed sessions, messages, tools, usage, and checkpoints within the requested root. Discovery failures now abort and roll back the run instead of appearing successful; archive moves preserve sessions before old checkpoints are removed. CLI summaries include removal counts.

- Collapse `<multi_agent_role>` setup messages with a descriptive title at session start and after resuming.

### Added

- Visible session pages now offer authenticated, deterministic Markdown and JSON postmortems containing aggregate turn, usage, model, tool, pressure, and evidence-gap facts. A fixed redaction profile excludes transcript and tool bodies, sensitive paths, credentials, native identifiers, and user annotations; adversarial synthetic regressions enforce the boundary.

- An authenticated diagnostics page now retains and displays timestamped context-window and rate-limit snapshots, including rate-only events with missing token information, source scope, recent/stale/expired/unavailable states, and exact evidence links. Deterministic summaries explain interruptions, incomplete turns, accounting anomalies, and explicitly failed tools without inferring live allowances, task failure, or quality.

- Session pages now link to a paginated ordered turn history with completion, interruption, resumption, unclosed/terminal-only preservation, evidence-based duration, requested-model changes, separately observed served-model fallbacks, associated tool outcomes, and links to supporting timeline events. Existing indexes upgrade safely; parser version 6 populates history on refresh.

- An authenticated historical usage dashboard now provides fixed-timezone daily, weekly, and monthly observation-time totals grouped by session, model, project, host, user, or agent. It includes cache ratios, partial/unavailable labels, anomaly and unattributed counts, bounded recent transcript drill-down, and paginated detail that reconciles with complete-range totals.

- Cumulative token snapshots now reduce into deterministic, per-field session totals without double-counting. Ordered raw observations retain cumulative and non-additive per-turn scopes, deltas, duplicate/reset/missing/inconsistent evidence, cached-input and reasoning-output subset semantics, and explicit `NULL` availability. Existing indexes upgrade safely, and one parser-version refresh computes summaries with rebuild parity.

- Session pages now provide a source-ordered message/tool/question timeline with event-kind filters, per-event anchors, bounded previews, lazy argument/result details, retained orphan outputs, repeated-ID-safe pairing, and explicit observed/failed/unknown evidence labels. Existing indexes migrate automatically; one parser-version refresh recovers exact result order.

- Transcript sections and lazy message pages now safely render bounded CommonMark with GFM tables, fenced code, strikethrough, and task lists. Raw HTML is escaped, unsafe links lose their anchors, images become alt text, ordinary text keeps its whitespace-preserving presentation, and a restrictive Content Security Policy prevents automatic remote resource loads while existing setup labels, grouping, notices, and pagination remain intact.

- Session and transcript search now combines literal title/native-ID/project matching with a disposable SQLite FTS5 transcript index. Results preserve all filters and pagination, show bounded escaped previews linked to matching messages, prefer ordinary conversation over setup matches, and automatically backfill existing indexes. Full rebuilds bulk-build FTS before publication; documented synthetic measurements show the resulting rebuild and storage overhead.

- The sessions browser now supports optional host-aware project and subproject grouping, agent/project/archive/date filters, last-observed activity, and stable activity ordering. Project configuration is read-only and runtime-loaded, with POSIX and drive-letter Windows path support, an explicit fixed-offset display timezone, preserved filter pagination, and clear empty-result states.

- Transcript URLs now use reproducible source-scoped session IDs instead of disposable SQLite row IDs, remaining stable across rebuilds while preventing collisions between copied homes with the same native session ID. Session pages and lists show readable titles from streamed optional Codex advisory metadata, bounded conversation text, or native IDs, with automatic legacy-index migration.

- Long transcripts now load in 50-message sections. Large message bodies use bounded previews and lazy 16,384-character parts, keeping transcript database reads and rendered responses bounded while retaining access to all indexed content.

- The sessions list now uses 50-row pages with deterministic newest-first ordering and previous/next links that preserve active filters. Queries fetch at most 51 session rows to determine whether another page exists.

- Rebuild and refresh can index one `.codex` home directly. Direct-home indexes omit host/user UI, repeated direct refreshes remain supported, and refresh rejects switching between hostless and host/user layouts without a rebuild.

- Incremental `refresh --data data` command with BLAKE3 file checksums, automatic legacy-index upgrades, parser-version invalidation, and unchanged-file skipping. File replacements are transactional, preserve session IDs, and retain previous transcripts on failure; synthetic regressions cover refresh/rebuild parity, archive moves, conflicts, partial files, and retries.

- Feature comparison of codex_viewer, codex-dash, and codex-blackbox, with pinned sources and a phased AgentLens roadmap covering browsing, usage, diagnostics, incremental collection, and additional agents.

- Historical regression audit and expanded synthetic coverage for ingestion variants and failures, transaction rollback, database integrity, CLI rebuilds, authentication, filtering, and rendered transcripts.
- A shared `scripts/check.sh` regression check command and mandatory per-change regression verification, including rendered transcript coverage.
- Prioritised project TODO list tracking architecture-review findings and conditional technology decisions.

- Mandatory username/password authentication on all web routes, with a Git-ignored local credentials file, an empty example, fail-closed startup, and no-store responses.

- Initial Rust CLI and Axum web application.
- Disposable SQLite index with generic agent, host, user, and instance records.
- Tolerant Codex rollout JSONL ingestion from active and archived session trees.
- Normalized sessions, transcript messages, tool calls, token usage, source files, and ingest runs.
- Filterable sessions page and transcript drill-down page.
- Redacted ingestion fixture and automated tests.
- Fixture format documentation and repository contribution conventions.
- Agent Lens project identity and canonical repository metadata.

### Changed

- Web database reads and transcript/view-model construction now live in focused query and presentation modules. Route handlers are limited to request validation, orchestration, error mapping, and template responses without changing existing routes or rendering behavior.

- Codex home discovery, rollout parsing, lifecycle interpretation, hidden-model policy, and transcript marker recognition now live behind a source adapter. Ingestion consumes source-neutral normalized records, and presentation policies dispatch by indexed agent kind without changing existing parser semantics.

- Rollout ingestion now uses two record-streaming passes instead of retaining every decoded record in a file. Malformed diagnostic examples are capped with exact totals, and tool outputs use indexed SQLite pairing instead of an unbounded in-memory map. A 65.2 MB synthetic single-rollout benchmark reduced sampled peak RSS by roughly 93% and improved median rebuild time by about 18%.

- Consolidate all 16 open architecture TODO items into the roadmap, retaining acceptance criteria and multi-user deployment requirements; TODO.md now links to the single backlog.

- Record multiple application users as a confirmed roadmap requirement, including individual identities, access permissions, and account storage independent of index rebuilds.
- Recognize resume-time repository instructions and tagged setup blocks throughout transcripts; label interruptions and user actions while keeping them expanded.
- Repository guidance now requires a rebuild-performance review whenever import or ingest code changes.
- Full index rebuilds batch SQLite writes in a transaction, avoiding per-record commit overhead as datasets grow.
- Session filters are populated from indexed host, user, model, and status values, and host and user are displayed in separate columns.
- Transcript messages are collapsible; initial configuration and environment sections start collapsed while conversation sections start expanded.
- Consecutive agent replies before the next user message are grouped into one transcript block as separate paragraphs.
- Initialization sections are recognized by their content rather than position and start collapsed with descriptive titles.
- The development web server now defaults to port 3001.
