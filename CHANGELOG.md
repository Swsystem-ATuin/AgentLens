# Changelog

All notable changes to this project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project intends to follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Remove the compatibility-only `TODO.md`; `ROADMAP.md` is now the sole backlog for planned work.

### Fixed

- Hermes cron run documents containing only scheduler metadata are now hidden from the sessions browser by default, while cron artifacts and canonical cron sessions with assistant or tool output remain visible. An “All cron runs” filter restores the complete audit history without deleting indexed evidence or blocking direct transcript URLs; parser version 3 classifies existing artifacts on the next refresh.

- Gemini CLI session and hook context now render as collapsed internal evidence without opening turns or becoming fallback titles. Current `functionCall` and `functionResponse` content parts import as paired tool activity instead of disappearing or producing empty user messages, thought-marked text remains hidden, main sessions recover best-effort workspace attribution from structured session context when metadata omits directories, and parser version 2 repairs existing disposable indexes on refresh.

### Added

- Gated internal OIDC browser/session bridge with CSRF and exact-origin initiation checks, persisted opaque browser binding, one-use callback handling, and exact provisioned identity resolution using current local account permissions. Signed-provider regressions cover resumed/cross-browser sessions, replay and account/link changes; no authenticated session, provider credentials, or MFA proof is persisted, and production routes remain disabled pending MFA/session/logout completion.

- Opt-in OIDC declared-deployment preflight (`oidc-check --prepare --deployment`) requires an explicit matching HTTPS public origin, Secure-cookie declaration, current read-only state, and an enabled local administrator password record before credentials/network access. Does not certify proxy/MFA readiness, mutate state, or enable federated sign-in.

- Explicit OIDC `oidc-check --prepare` preflight loads bounded owner-only client-secret files and verified discovery/signing keys, constructs the internal protocol engine, and fails closed without writing state or enabling sign-in. Includes synthetic HTTPS and credential/privacy regressions; deployment/session/MFA/logout integration remains pending.

- Internal OIDC authorization-code engine using maintained `openidconnect`, with S256 PKCE, 256-bit state/nonce, browser-bound bounded one-use transactions, RS256 signature/issuer/audience/authorized-party/nonce/time/hash validation, RSA key-size limits, bounded HTTPS token transport, and signed synthetic-provider regressions. Not wired into browser authentication: runtime activation, local identity/MFA/session integration, and logout/revocation remain required before enabling sign-in.

- Persistent, exact issuer/subject OIDC identity links provisioned through local administrator CLI commands, with owner-only subject-file input, metadata-only bounded listing, account-scoped unlink/recovery, atomic session/token invalidation and fixed audit events. State schema v10 preserves existing local authentication and permissions; federated sign-in remains unavailable pending protocol/logout integration.

- Dedicated automation API reference covering every supported bearer endpoint, complete JSON response shapes, scopes and source authorization, rate limits, error envelopes, privacy, compatibility, and credential-safe curl examples; linked from the README and related feature documentation.

- Explicit OIDC discovery preflight (`oidc-check --discover`) with exact issuer pinning, required code/S256/client-authentication/logout capabilities, bounded verified HTTPS without redirects or ambient proxies, sanitized diagnostics, and real synthetic-TLS transport regressions. Offline validation remains the default and federated sign-in remains unavailable; containers now include CA certificates for outbound TLS.

- Offline OIDC configuration preflight with required issuer/client/public URL and explicit linking, local-permission, logout, and break-glass policies. Rejects incomplete/unsafe configuration without contacting providers or reading secrets; federated sign-in remains unavailable.

- Opt-in WebAuthn passkeys as password-first second factors, disabled until an exact public HTTPS URL and Secure cookies are configured. Adds named device enrollment/removal, required-MFA and shared recovery integration, bounded server-only challenges, single-use action proofs, current credential/session checks, committed counter protection, schema-v9 migration, signed software-authenticator regressions, and OpenSSL container dependencies.

- Opt-in TOTP MFA with administrator-required enrollment and recovery/reset controls, restricted setup sessions, one-time recovery-code reveal and digest-only storage, authenticated encrypted secret storage with durable key rotation, replay protection, persisted verification limits, fresh factors for sensitive actions, credential invalidation, and schema-v8 audit migration. MFA accounts use scoped tokens instead of password-only metrics authentication.

- Audited browser administration now provisions accounts, manages enabled state, roles, direct/team grants and membership, resets credentials, revokes sessions/tokens, and reviews bounded security history. Every mutation requires CSRF, a fresh password confirmation, exact-target confirmation, transactional actor/session checks, and atomic before/after facts in schema v7. Shared password-attempt throttling avoids automatic account lockouts and preserves local CLI recovery.

- Stable-ID teams, additive direct/team source grants, auditor and operator capabilities, local team-management commands, and a bounded own-access explanation page. State schema v6 preserves existing identities, sessions, preferences, tokens, and grants, audits team changes, and prevents disabling, deleting, or demoting the last enabled administrator.

- Personal access tokens now support one-time bearer reveal, digest-only persistent storage, bounded expiry, read scopes for metrics/redacted postmortems/live status, current account/source permission checks, password-confirmed and CSRF-protected creation, revocation, rate limits, bounded listing, retention, and audited schema-v5 migration.

- A shared typed capability policy now governs browser and metrics authentication, source-wide access, and self-service account/security routes. Unknown routes and unsupported methods fail closed for every role; tested credential ceilings prepare least-privilege token scopes without enabling future administrative features.

- Signed-in accounts now have a bounded session inventory and append-only security history, with individual or all-other-session revocation, current/active/expired/revoked states, allowlisted client summaries, optional display names, password age, schema-v4 migration, and explicit 30-day session/180-day event retention. Network addresses remain uncollected without a trusted-proxy design.

- Authenticated accounts now have typed persistent display and browsing preferences for UTC, browser-local, or IANA timezones; date formats; system/light/dark/high-contrast appearance; font scale; bounded page size; live-reader and transcript defaults; code wrapping; and the default sessions view. State schema version 3 migrates automatically, unknown future values fall back safely, historical IANA boundaries remain DST-correct, and raw indexed timestamps are unchanged.

- A dependency-ordered post-MVP account-management roadmap now defines completion gates for per-account preferences, self-service session and security history, scoped personal access tokens, capability-based roles and teams, audited browser administration, TOTP/passkey/OIDC decision points, redacted-content access, and conditional outbound notifications.

- Signed-in users can now change their own password from the Account page after confirming their current password. New passwords share a minimum 7-character complexity policy across browser and administrator workflows, successful changes rotate the current session, and all other sessions for the account are revoked immediately.

- Repository-wide Trunk code-quality configuration now pins checks for Rust, shell,
  Docker, TOML, Markdown, whitespace, and accidentally committed secrets, with managed
  pre-commit and pre-push hooks and documented local usage.

- Production service examples now support an unprivileged Agent Lens web process
  under systemd or SysV init, with a frequent non-overlapping incremental refresh
  through a systemd timer or cron. A shared environment file configures database,
  source, bind, local/authenticated, cookie, logging, and project settings, and
  packaging validation is part of the normal regression check.

- Hermes Agent now imports canonical interactive and gateway conversations from the default and named-profile `state.db` stores through consistent read-only SQLite snapshots, including lifecycle, titles, models, compacted history, tool activity, and aggregate usage. Completed scheduler artifacts under `cron/output/` are indexed as separate activities, while request-failure snapshots remain available as distinct attempts.

- A new Agent Lens logo now appears in the application header and sign-in screen, with embedded PNG and multi-size ICO favicons served to browsers without requiring authentication.

- An explicit `serve --local` option lets a developer review indexed activities and reports without creating an account. It permits only numeric loopback bind addresses and leaves the authenticated server mode as the default.

- Session pages now list and serve authenticated Codex-generated images directly from the copied `.codex/generated_images/<native-session-id>/` tree, with source-grant enforcement, strict path/type/size containment, compact lazy same-origin previews after the assistant output, and no SQLite or raw-source mutation. Previews omit a padded outer border and place their named full-size link below the image. Historical relative image links resolve directly when recorded `Copy-Item` or `cp` evidence maps the workspace alias to a retained file; unlinked desktop-generated media appears once in a transcript fallback section instead of remaining hidden.

- Codex `<apps_instructions>`, `<plugins_instructions>`, `<recommended_plugins>`, `<app-context>`, and `<image_resize_notice>` setup messages now render with descriptive labels and remain collapsed at session start and after resume. Incremental refresh reimports Codex rollouts once so these envelopes are also excluded from fallback session titles. A synthetic observed-envelope inventory must match the presentation contract, and drives complete initial/resumed rendering and title-derivation regression coverage.

- A multi-stage Docker build now produces a minimal non-root runtime image with persistent SQLite paths, an all-interface web bind, and documented read-only source mounts.

- Hermes Agent request snapshots are discovered from `.hermes/sessions/request_dump_*.json`, including OpenAI Responses and Anthropic message/tool shapes, direct-home operation, sanitised failure presentation, and strict exclusion of transport credentials and error payloads.

## [0.1.0] - 2026-09-14

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

- Agent Lens is now licensed under GPL-3.0-or-later so recipients can use and continue developing it while distributed derivatives preserve notices, identify modifications, and provide corresponding source under the same license.

- Source-scoped application authorization now separates all-access administrators from viewers with explicit stable source-ID grants. Lists, search, filter values, usage, diagnostics, transcripts, live status, turns, message/tool details, and postmortem exports are scoped before pagination or aggregation; ungranted session IDs return `404`, installation-wide health and metrics require an administrator, and future routes fail closed for viewers. Local role/grant commands, immediate policy updates, a separate owner-only schema-v2 state store, migration of existing accounts to administrators, rebuild-stable mappings, backup/recovery documentation, and two-account regressions complete M2 and M3.

- Individual application accounts now replace the interim shared plaintext credential file. Local CLI provisioning and recovery store salted Argon2id hashes in a separate owner-only persistent SQLite state database; browser login/logout use CSRF-protected opaque server-side sessions with identifier rotation, 30-minute inactivity and 12-hour absolute expiry, restart persistence, and immediate password-reset, disablement, and account-wide revocation enforcement. Every data route remains protected, and `/metrics` supports individual-account Basic authentication for Prometheus.

- An authenticated Prometheus text endpoint now exposes fixed-cardinality gauges for indexed ingest/source health, sessions, supported token totals and availability, tool results, and turn outcomes. Labels use bounded agent/status/freshness/outcome/reason/model-family vocabularies and exclude transcript content, identities, paths, raw model values, parser details, and tool data. Restart/rebuild semantics, Basic-auth scraping, response bounds, and a 100,000-session synthetic load measurement are documented and regression-tested.

- Gemini CLI is now a second source adapter, discovering current JSONL and legacy JSON chats in host/user or direct-home layouts. It applies upstream checkpoint/rewind semantics, normalizes conversation, model, embedded-tool, and per-turn token evidence, renders adapter-specific initial/resumed notices, and explicitly leaves unsupported lifecycle, aggregate usage, pressure, and turn completion unavailable. The pinned raw contract, privacy boundary, capability gaps, synthetic mixed-agent identity/project coverage, refresh/rebuild parity, and performance tradeoffs are documented.

- Authenticated source health and a sanitised local `health` command now expose latest-ingest duration/counts plus bounded per-source projection and freshness states. Failed refresh discovery/traversal attempts retain only fixed health codes while all projection changes roll back. Opaque IDs and fixed reason codes exclude raw paths, parser details, and transcript-derived titles; direct-home origin hiding, stale/future-clock-skew handling, and 100-row pagination have rendered regressions. The future opt-in push collector now has an explicit versioning, redaction, transport-authentication, replay/deduplication, retention, and summary-only projection design gate; collection remains disabled.

- Session pages now provide opt-in live reading through a lightweight authenticated revision probe and bounded server-rendered transcript replacement. The client preserves filters, sections, open blocks, and viewport position, retries disconnected probes, and displays import/source freshness separately from lifecycle status; synthetic concurrent-refresh, replacement, archive, and tool-pairing regressions cover the behavior.

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

- Initial Rust CLI and Axum web application.
- Disposable SQLite index with generic agent, host, user, and instance records.
- Tolerant Codex rollout JSONL ingestion from active and archived session trees.
- Normalized sessions, transcript messages, tool calls, token usage, source files, and ingest runs.
- Filterable sessions page and transcript drill-down page.
- Redacted ingestion fixture and automated tests.
- Fixture format documentation and repository contribution conventions.
- Agent Lens project identity and canonical repository metadata.

### Changed

- Release preparation now includes Cargo package metadata, source-package and locked release-build verification, an atomic `main`/annotated-tag publication procedure, and a 0.1.0 upgrade checklist.

- The conditional database-backend review retains SQLite for the measured single-host, single-index-writer deployment and now explicitly enables WAL for persistent account/session state. PostgreSQL migration triggers are multiple instances, remote storage, sustained independent writers, measured lock/checkpoint pressure, or managed HA/replication/PITR requirements; the decision records SQLite-specific migration work and evidence gates.

- The pre-account-management framework gate retains Rust/Axum and selects established server-side session authentication with locally provisioned Argon2id-backed application identities. The decision records the Django comparison, separate persistent-state boundary, provisioning/recovery and revocation expectations, OIDC deferral, implementation security contract, and evidence that would trigger reconsideration.

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
