# Changelog

All notable changes to this project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project intends to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial Rust CLI and Axum web application.
- Disposable SQLite index with generic agent, host, user, and instance records.
- Tolerant Codex rollout JSONL ingestion from active and archived session trees.
- Normalized sessions, transcript messages, tool calls, token usage, source files, and ingest runs.
- Filterable sessions page and transcript drill-down page.
- Redacted ingestion fixture and automated tests.
- Fixture format documentation and repository contribution conventions.
- Agent Lens project identity and canonical repository metadata.

### Changed

- Recognize resume-time repository instructions and tagged setup blocks throughout transcripts; label interruptions and user actions while keeping them expanded.
- Repository guidance now requires a rebuild-performance review whenever import or ingest code changes.
- Full index rebuilds batch SQLite writes in a transaction, avoiding per-record commit overhead as datasets grow.
- Session filters are populated from indexed host, user, model, and status values, and host and user are displayed in separate columns.
- Transcript messages are collapsible; initial configuration and environment sections start collapsed while conversation sections start expanded.
- Consecutive agent replies before the next user message are grouped into one transcript block as separate paragraphs.
- Initialization sections are recognized by their content rather than position and start collapsed with descriptive titles.
- The development web server now defaults to port 3001.
