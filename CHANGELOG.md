# Changelog

All notable changes to this project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project intends to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- Collapse `<multi_agent_role>` setup messages with a descriptive title at session start and after resuming.

### Added

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
