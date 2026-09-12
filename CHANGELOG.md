# Changelog

All notable changes to this project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project intends to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Rebuild and refresh summaries now distinguish hard file errors, partial files, and malformed-line totals. Parser regressions verify valid-record survival, exact sanitised line diagnostics, unknown fields, schema variants, malformed JSON, and partial UTF-8.

- Session lifecycle status now follows source-record order: later completion or interruption events replace earlier outcomes, while later turn/user activity reopens a terminal session to `unknown` without claiming it is currently running. Existing indexes are automatically reimported on refresh through a parser-version bump.

- Internal web failures now return a generic browser response and emit only allowlisted diagnostic fields on the server. Request tracing excludes URIs and authorization data so private query values, transcript content, credentials, and underlying error text are not logged.

- Full rebuilds now import into and validate a staging database before replacing the usable index. Failed rebuilds preserve the previous index, and publication refuses to replace a database still held open by the web server.

- Refresh and rebuild accept directory symlinks to nightly backups throughout source discovery and traversal, preserving logical source paths across backup rotation. Broken links and directory loops roll back refresh without pruning indexed sessions.

- Hide `codex-auto-review` model sessions, including prefixed variants, from the sessions list, model dropdown, and direct transcript URLs without deleting indexed data.

- Refresh removes missing source files and their indexed sessions, messages, tools, usage, and checkpoints within the requested root. Discovery failures now abort and roll back the run instead of appearing successful; archive moves preserve sessions before old checkpoints are removed. CLI summaries include removal counts.

- Collapse `<multi_agent_role>` setup messages with a descriptive title at session start and after resuming.

### Added

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
