# Project TODO

Tracked findings from the architecture review on 2026-09-09. These are open tasks, not implemented behavior. Keep this list current as work lands: check completed items and reference the implementing commit or documentation. Add newly discovered work here and revise priorities as requirements change.

The current direction is to retain Rust, SQLite, and server-rendered HTML. Technology changes below are conditional decisions, not scheduled rewrites. Performance findings are based on code review; expected improvements still need measurement.

## Priority 1 — Reliability and correctness

- [ ] **Preserve the usable index during rebuilds.** Build and validate a replacement before switching over. Explicitly manage active database connections and SQLite WAL/SHM files. Verify that failed or interrupted rebuilds preserve the previous index and that a successful switch serves the new data. Document the supported procedure when the web server is running.
- [ ] **Report incomplete source discovery.** Replace silently ignored directory and traversal errors with useful, sanitised diagnostics and an accurate ingest result. Verify missing and unreadable sources cannot look like a successful complete import.
- [ ] **Keep internal errors out of browser responses.** Return generic server-error messages while retaining sanitised diagnostic context on the server. Test that responses and logs do not disclose private transcript content or credentials.
- [ ] **Derive session status from event order.** Define completion, interruption, and resumed-session behavior, then cover those sequences with synthetic tests instead of relying on whether an event appears anywhere in a file.
- [ ] **Expand parser resilience tests.** Cover unknown fields, malformed lines, partial files, and observed schema variants with tiny synthetic or fully redacted fixtures. Verify useful records survive local parsing failures and diagnostics accurately describe partial imports.

## Priority 2 — Scale and maintainability

- [ ] **Bound ingestion memory.** Evaluate streaming with bounded buffering against a two-pass parser where metadata discovery requires it. Measure representative full rebuilds before and after, including elapsed time and peak memory; review transaction boundaries, query volume, file I/O, and allocations. Report only aggregate results, without private content.
- [ ] **Paginate the sessions list.** Add bounded queries with stable ordering and navigation that preserves filters. Verify that larger datasets do not require loading every matching session for one page.
- [ ] **Load long transcripts in sections.** Bound database reads and rendered response size while preserving message order and sensible grouping across section boundaries. Collapsed content should not require transferring the entire transcript upfront.
- [ ] **Separate source adapters from generic application logic.** Move Codex discovery, parsing, and setup-marker recognition into source-specific modules. Keep normalised events, indexing, queries, and presentation generic; preserve existing behavior with tests.
- [ ] **Separate querying and transcript presentation from route handlers.** Extract focused modules as part of the source-boundary refactor, keeping handlers responsible for request extraction and responses.

## Planned requirement — Multiple application users

Multiple application users are a confirmed future requirement. The current shared Basic authentication credential is an interim implementation. Timing and the visibility policy between users remain to be decided.

- [ ] **Choose the authentication approach before shared deployment.** Support individual identities using an established identity provider or established session authentication. Provide login, logout, session expiry, and account/session revocation. If passwords are managed locally, store password hashes instead of plaintext and define account provisioning and recovery.
- [ ] **Define and enforce access permissions.** Decide whether users share all indexed data or have access scoped to selected sources, hosts, or sessions, and define any administration roles. Enforce the chosen policy on the server for lists, filters, direct transcript URLs, and future endpoints. Test allowed and denied access with separate synthetic accounts.
- [ ] **Persist accounts and permissions independently of the index.** Keep application identities distinct from the indexed operating-system users in the existing schema. Ensure rebuilds preserve accounts and permissions; permission mappings must not rely on disposable SQLite row IDs that can change or be reused. Document migrations and backup/restore procedures for persistent application state.
- [ ] **Review the framework choice before implementing account management.** Compare continued Rust development with Django against the confirmed multi-user requirements and chosen identity integration. Record the decision; multiple users alone do not require a language or database change.

## Conditional decisions

- [ ] **Evaluate HTMX when adding partial page updates.** Consider it for filtering and transcript section loading while retaining server-rendered HTML. Introduce a separate frontend only if interactive timelines, graphs, or substantial browser state justify it.
- [ ] **Reassess SQLite if deployment changes.** Evaluate PostgreSQL if multiple application instances or substantial concurrent writing become requirements; retain SQLite for the current local importer and predominantly read-only browsing workload.

## Completion expectations

Follow `AGENTS.md` for implementation checks, documentation, and commit/push requirements. Behavioral changes require `cargo fmt` and `cargo test`; substantial work also requires Clippy. Ingest changes require a rebuild-performance review and representative before/after benchmarks when throughput or resource use may change. Tests must never read private development data. Document migration, rebuild, or restart requirements alongside each applicable change.
