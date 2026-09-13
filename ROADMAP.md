# AgentLens roadmap

Updated 2026-09-13. This is the single backlog for planned features, architecture work, and conditional decisions. It incorporates every open item from the 2026-09-09 architecture TODO and the [feature comparison](docs/feature-comparison.md) of codex_viewer, codex-dash, and codex-blackbox. All unchecked items are unimplemented proposals; the multiple-user requirement is confirmed, while its visibility policy remains undecided.

Keep IDs stable, check items when delivered, and link the implementing commit or documentation. Milestones express dependency order, not delivery dates. Retain Rust, SQLite, server-rendered HTML, immutable raw sources, and a completely rebuildable index. Performance benefits are hypotheses until measured.

## Delivery sequence

| Milestone | Outcome | Dependencies / completion gate |
| --- | --- | --- |
| R0 — Reliability | Trust rebuilds, errors and status | First: protects existing data access and later reports |
| R1 — Bounded browsing and source boundaries | Read large datasets with bounded resources | R0; measure rebuilds and query/response bounds |
| R2 — Find and understand sessions | Titles, projects, search and richer transcripts | R1; durable identity before saved links or annotations |
| R3 — Usage and diagnostics | Explain consumption and problematic turns | R0 lifecycle semantics and R1 adapters; R2 tool views for drill-down |
| R4 — Incremental observation | Freshness-aware updates and optional collection | R0/R1; use M1–M4 before shared deployment |
| R5 — Wider observability | Additional adapters and optional metrics | R1 contracts; R3 accounting; M1–M4 for shared access |
| M — Multiple application users | Individual identities and enforced permissions | Design may begin now; must complete before shared deployment, regardless of feature milestone |
| C — Conditional decisions | Evidence-based technology choices | Evaluate at the relevant feature/deployment gate |

Start with R0, then deliver R1 before expanding the browser workload. R2 and R3 can be scheduled independently once their stated prerequisites hold. M is a deployment gate, not a feature deferred until after R5.

## R0 — Reliability and correctness

The following five items retain the original TODO acceptance criteria.

- [x] **R0.1 — Preserve the usable index during rebuilds.** Build and validate a replacement before switching over. Explicitly manage active database connections and SQLite WAL/SHM files. Verify that failed or interrupted rebuilds preserve the previous index and that a successful switch serves the new data. Document the supported procedure when the web server is running. Delivered with staged, validated publication; [behavior and verification](docs/rebuild.md).

- [x] **R0.2 — Report incomplete source discovery.** Replace silently ignored directory and traversal errors with useful, sanitised diagnostics and an accurate ingest result. Verify missing and unreadable sources cannot look like a successful complete import. Delivered with transactional missing-source cleanup; [behavior and verification](docs/refresh.md).

- [x] **R0.3 — Keep internal errors out of browser responses.** Return generic server-error messages while retaining sanitised diagnostic context on the server. Test that responses and logs do not disclose private transcript content or credentials. Delivered with source-free browser responses and allowlisted server diagnostics; [behavior and verification](docs/web-errors.md).

- [x] **R0.4 — Derive session status from event order.** Define completion, interruption, and resumed-session behavior, then cover those sequences with synthetic tests instead of relying on whether an event appears anywhere in a file. Delivered with an ordered, conservative lifecycle reducer; [behavior and verification](docs/session-status.md).

- [x] **R0.5 — Expand parser resilience tests.** Cover unknown fields, malformed lines, partial files, and observed schema variants with tiny synthetic or fully redacted fixtures. Verify useful records survive local parsing failures and diagnostics accurately describe partial imports. Delivered with retained raw-field/schema-variant coverage, valid-record survival across malformed JSON/UTF-8, exact sanitised line diagnostics, and separate CLI partial-file/malformed-line totals; [format contract](docs/fixture-format.md) and [refresh behavior](docs/refresh.md).

## R1 — Bounded browsing and source boundaries

These five items retain the original TODO acceptance criteria. The regression suite already covers several parser variants; the remaining work extends its guarantees rather than replacing it.

- [x] **R1.1 — Bound ingestion memory.** Evaluate streaming with bounded buffering against a two-pass parser where metadata discovery requires it. Measure representative full rebuilds before and after, including elapsed time and peak memory; review transaction boundaries, query volume, file I/O, and allocations. Report only aggregate results, without private content. Delivered with two-pass record streaming, bounded malformed-line examples, indexed tool-output pairing, preserved transactional semantics, and a synthetic before/after benchmark; [design and verification](docs/ingestion-memory.md).

- [x] **R1.2 — Paginate the sessions list.** Add bounded queries with stable ordering and navigation that preserves filters. Verify that larger datasets do not require loading every matching session for one page. Implemented with 50-row pages, a 51-row query ceiling for next-page detection, deterministic timestamp/ID ordering, safely encoded filter continuity, and rendered multi-page regressions.

- [x] **R1.3 — Load long transcripts in sections.** Bound database reads and rendered response size while preserving message order and sensible grouping across section boundaries. Collapsed content should not require transferring the entire transcript upfront. Delivered with 50-message/51-row sections, bounded message previews, lazy message-part navigation, section-aware grouping, and [documented query and response limits](docs/transcript-sections.md).

- [x] **R1.4 — Separate source adapters from generic application logic.** Move Codex discovery, parsing, and setup-marker recognition into source-specific modules. Keep normalised events, indexing, queries, and presentation generic; preserve existing behavior with tests. Delivered with a source-neutral record/presentation contract, a dedicated Codex discovery and streaming parser module, agent-kind presentation dispatch, cross-source policy regressions, and [documented boundaries and performance verification](docs/source-adapters.md).

- [x] **R1.5 — Separate querying and transcript presentation from route handlers.** Extract focused modules as part of the source-boundary refactor, keeping handlers responsible for request extraction and responses. Delivered with dedicated SQLite query and presentation/view-model modules, thin coordinating handlers, preserved route-level regressions, and [documented web boundaries](docs/web-architecture.md).

## R2 — Find and understand sessions

Inspiration: codex_viewer's reader and advisory names, and codex-dash's project navigation. See the [review and pinned sources](docs/feature-comparison.md). These are adaptations to AgentLens's web application.

- [x] **R2.1 — Stable session identity and readable titles.** Use an application-defined source identity plus agent-native session ID for durable links; never depend on rebuildable SQLite row IDs. Prefer valid advisory names where available, with a bounded conversation-derived fallback and native ID fallback. Keep discovery authoritative to rollouts. For Codex, read optional `session_index.jsonl` through the adapter, tolerate malformed/partial entries, and document the precedence rule. Delivered with reproducible source-scoped public IDs, streamed advisory-name enrichment, bounded conversation/native fallbacks, automatic disposable-index migration, and rebuild/collision/browser-route regressions; [identity and title semantics](docs/session-identity.md).

- [x] **R2.2 — Project navigation and useful filters.** Group working directories using configurable host-aware project roots, with optional subprojects. Add project, date range, archive state, and agent filters alongside existing filters; define timezone and date-boundary semantics. Include readable titles, last observed activity, stable sorting, and clear filtered-empty states. Test overlapping roots, Windows/POSIX paths, missing metadata, filter combinations, and pagination continuity. Delivered as read-only runtime grouping with bounded host/path rules, most-specific subproject matching, fixed-offset inclusive activity dates, stable activity/public-ID ordering, composable filters, and rendered edge-case regressions; [project navigation semantics](docs/projects.md). Manual assignments and context notes remain deferred to M2/M3.

- [x] **R2.3 — Search sessions and transcripts.** Start with title/native ID/project search, then indexed transcript search with bounded snippets and links to matching messages. Evaluate SQLite FTS in the disposable index. Preserve filters and pagination, escape search highlights, and enforce the same access scope on results, counts and snippets. Verify special characters, rebuild parity, large result sets, and setup blocks not obscuring ordinary conversation matches. Benchmark search-index rebuild overhead. Delivered with literal metadata search, disposable FTS5 transcript indexing, message-linked escaped previews, composable pagination/filtering, bounded candidate and response sizes, automatic legacy backfill, bulk rebuild construction, access-scope regressions, and a synthetic before/after benchmark; [search semantics and verification](docs/search.md).

- [x] **R2.4 — Safe rich transcript rendering.** Add Markdown/GFM, fenced code and tables while preserving current setup labels, interruption notices, grouped replies, and plain-text fallback. Disable raw HTML, unsafe URL schemes and automatic remote resource loads. Verify rendered HTML for initial/resumed sessions, malicious content, ordinary prose and section boundaries. Add optional math only after core rendering and resource limits are established; it is not a prerequisite for the reader milestone. Delivered with bounded CommonMark/GFM rendering, escaped raw HTML, image suppression, scheme-filtered links, a restrictive no-resource Content Security Policy, whitespace-preserving plain-text fallback, and rendered security/grouping/resume/section regressions; [rendering contract and limits](docs/rich-transcripts.md). Math remains optional and disabled.

- [x] **R2.5 — Tool and question/answer timeline.** Interleave messages with tool calls, results, and explicit user-input requests/responses using source order. Provide bounded previews, lazy details, event-kind filters and message/turn anchors. Preserve call/result identity across pages and refreshes. Distinguish requested, output observed, failed and unknown outcomes: the existing `complete` tool status only means an output was paired, not successful execution. Cover missing/orphan outputs, repeated IDs, malformed arguments, structured outputs, aborted questions and ordinary conversation in both domain and HTML tests. Retain provenance needed to reconstruct order; existing normalized tool rows do not contain a separate output sequence. Delivered with first-class ordered result rows, latest-preceding-ID pairing, explicit question/answer kinds, 50-event sections, bounded previews/lazy details, event filters, stable anchors, legacy-order disclosure, refresh/rebuild parity, rendered edge-case regressions, and a measured tool-heavy rebuild tradeoff; [timeline semantics and verification](docs/tool-timeline.md).

## R3 — Usage and turn diagnostics

Inspiration: codex-dash's usage views and codex-blackbox's evidence-based reporting. Implement historical analysis from available raw records first; proxy-only observations remain unavailable without an additional source.

- [ ] **R3.1 — Correct token accounting.** Define generic cumulative-snapshot and per-turn usage semantics in source adapters. Current Codex ingestion stores every `total_token_usage` snapshot; summing those rows would double-count. Handle duplicate snapshots, counter resets, missing values, resumed sessions and inconsistent totals explicitly. Cached input is a subset of input; reasoning output, where reported as a subset, is not added to output again. Preserve raw evidence and label unavailable values rather than zero-filling. Test all cases with synthetic sequences and verify repeatable rebuild totals before any aggregate dashboard ships; require incremental parity when R4.1 is implemented.

- [ ] **R3.2 — Historical usage dashboard.** Show input, cached input, output, available reasoning detail and totals by session, model, project, host, user and agent. Add daily/weekly/monthly ranges, cache ratios, recent activity and drill-down. Define periods by event time and an explicit timezone; do not label a session's lifetime total as usage today merely because it was updated today. If only snapshots are usable, label the attribution limitation. Test period boundaries, unknown models, zero denominators and reconciliation from detail to totals. Monetary estimates are deferred pending versioned pricing and evidence about billing semantics.

- [ ] **R3.3 — Ordered turn and model history.** Normalize turn boundaries, requested models, completion, interruption, resumption and relevant tool outcomes. Show duration only where start/end evidence supports it, including the definition of that duration. Preserve unknown/unclosed turns; a recent file or last timestamp is not proof of activity or completion. Label a requested-model change separately from observed served-model fallback. Add sequence tests for resumed work, out-of-order timestamps and missing events, plus links back to the supporting timeline items.

- [ ] **R3.4 — Context, rate pressure and diagnostic summaries.** Surface observed context-window and rate-limit snapshots with timestamps and source scope where present. Retain the necessary events even when token `info` is missing. Show expired/stale readings as such; do not infer current subscription allowance from copied data. Build deterministic summaries of interruptions, incomplete turns, accounting anomalies and observed failures, each with provenance and an explanation. No automatic blocking, generated quality scores or claims that an interrupted session failed its task. Verify sparse/unknown variants, freshness rules and links to evidence.

- [ ] **R3.5 — Redacted postmortems.** Provide a local report of turn outcomes, usage, model changes and known evidence gaps, with explicit redacted Markdown/JSON export. Require R3.1/R3.3/R3.4 and stable session identity. Default exports to aggregate facts and bounded reason codes; exclude prompts, source code, tool bodies, sensitive paths and credentials. Apply access permissions to export endpoints, document any included identifying fields, and test adversarial synthetic redaction cases. Reports must remain reconstructible from raw sources; saved user annotations belong in durable application state.

## R4 — Incremental observation and collection

Inspiration: codex_viewer's incremental reader and codex-dash's freshness-aware pooled board. Copied homes remain immutable inputs; collector outputs and checkpoints must live elsewhere.

Initial CLI refresh delivered; see [refresh behavior and verification](docs/refresh.md). Checksums verify every file; changed projections replace atomically, and legacy checkpoints upgrade automatically. Missing-source deletion and fail-closed discovery are also delivered. R4.1 remains open for broader snapshot-publication and duplicate-resolution semantics.

- [ ] **R4.1 — Incremental, idempotent indexing.** Add an explicit refresh path that detects new/changed sources and updates only affected records. Handle appended partial JSON/UTF-8, truncation, replacement, archive moves, duplicates and interrupted commits. Define how immutable snapshots are published and how appended sources are validated; do not silently inherit an upstream probe heuristic that can miss historical edits. Keep full rebuild as the repair path and all checkpoints disposable. Verify repeated refreshes and a fresh rebuild yield equivalent normalized data; benchmark full rebuild and incremental elapsed time, peak memory and file I/O before/after.

- [ ] **R4.2 — Live reading with bounded updates.** Once R4.1 and R1 paging exist, add opt-in updates that preserve reading position, filters, open blocks and page continuity. Select polling/long-polling or SSE based on measured load; evaluate C1 here. Show last successful import, source observation time and disconnected/stale states separately from lifecycle status. No live-process claim from file modification time alone. Test reconnects, replacements, concurrent readers, archived sessions and call/result pairs across updates. Document actual update latency and required collector or copy workflow.

- [ ] **R4.3 — Source health and optional collection.** First expose sanitised discovery/parse failures, partial counts, ingest duration and per-source freshness in an authenticated health view and CLI diagnostic command. Later design opt-in collectors that export a versioned, minimal machine/session summary to an application-owned location; opening a page must not initiate SSH. Preserve source IDs, distinguish summary-only sessions from transcript-backed sessions, and label missing transcript drill-down. Require an explicit collection/redaction policy, transport authentication, replay/deduplication rules and retention limits. Test stale/offline machines and clock skew. Never collect authentication files or execute attach commands supplied by metadata.

## R5 — Wider observability

- [ ] **R5.1 — Validate a second agent adapter.** After R1 separates source concerns, choose a second agent based on available formats and user demand. Publish its raw format contract using tiny synthetic/redacted examples. Support missing capabilities and unknown events without forcing Codex-specific concepts into the generic domain. Test two agents in the same host/project views, identity isolation, rebuild parity and adapter-specific setup rendering. Selection and implementation are deferred until the source and acceptance corpus are agreed.

- [ ] **R5.2 — Optional operational metrics.** After R3 accounting and R4 health definitions, expose authenticated or separately restricted aggregate metrics for ingest health and supported usage/outcome counts. Use bounded agent/model/status labels, never prompts, session IDs, user identities, paths or tool inputs. Document whether values reflect process lifetime or indexed history and test restart/rebuild behavior. Offer a Grafana example only if deployment demand justifies it; the core UI must remain useful without a monitoring stack.

## M — Multiple application users

These four requirements are carried forward from TODO. Timing and the visibility policy remain to be decided, but the complete access model is a gate before shared deployment. Make M4's framework decision before implementing account management; settle M2/M3 before adding durable per-user annotations or saved views.

- [ ] **M1 — Choose the authentication approach before shared deployment.** Support individual identities using an established identity provider or established session authentication. Provide login, logout, session expiry, and account/session revocation. If passwords are managed locally, store password hashes instead of plaintext and define account provisioning and recovery.

- [ ] **M2 — Define and enforce access permissions.** Decide whether users share all indexed data or have access scoped to selected sources, hosts, or sessions, and define any administration roles. Enforce the chosen policy on the server for lists, filters, direct transcript URLs, and future endpoints. Test allowed and denied access with separate synthetic accounts.

- [ ] **M3 — Persist accounts and permissions independently of the index.** Keep application identities distinct from the indexed operating-system users in the existing schema. Ensure rebuilds preserve accounts and permissions; permission mappings must not rely on disposable SQLite row IDs that can change or be reused. Document migrations and backup/restore procedures for persistent application state.

- [ ] **M4 — Review the framework choice before implementing account management.** Compare continued Rust development with Django against the confirmed multi-user requirements and chosen identity integration. Record the decision; multiple users alone do not require a language or database change.

## C — Conditional technology decisions

These remain conditional decisions from TODO, not scheduled rewrites.

- [ ] **C1 — Evaluate HTMX when adding partial page updates.** Consider it for filtering and transcript section loading while retaining server-rendered HTML. Introduce a separate frontend only if interactive timelines, graphs, or substantial browser state justify it.

- [ ] **C2 — Reassess SQLite if deployment changes.** Evaluate PostgreSQL if multiple application instances or substantial concurrent writing become requirements; retain SQLite for the current local importer and predominantly read-only browsing workload.

## Deferred or excluded scope

| Idea | Decision and reason |
| --- | --- |
| Browser-triggered launch/resume, SSH/tmux attach, terminal input or preview | Excluded from this observability roadmap; would require a separately designed execution boundary and authorization model |
| Envoy interception, provider rerouting, request guards and automatic intervention | Deferred as a separate product decision; these add live request-path dependencies and evidence unavailable in copied rollouts |
| Provider account labels from authentication files | Excluded; use configured source labels and independent application identities |
| Edit/delete/archive source sessions or store project state inside copied homes | Excluded by the immutable-source contract; future reversible hiding would use application metadata |
| Dollar budgets, automatic coaching and performance/quality scoring | Deferred; token volume alone does not establish spend or task quality, and supported evidence is incomplete |
| Mandatory SPA, TUI, Grafana stack, Django or PostgreSQL rewrite | No current requirement; apply C1/C2 and M4 when their decision gates arise |

## Completion and verification

Follow [AGENTS.md](AGENTS.md) for every implementation: synthetic regression tests in the normal suite, failing-before/passing-after proof for fixes, rendered HTML coverage for transcript changes, and no tests reading private `data/`. Preserve useful unknown records and report local parsing failures. Update README and CHANGELOG with delivered behavior and cite the implementing commit here.

Run from the repository root before each commit:

```bash
bash scripts/check.sh
```

This runs formatting checks, all-target/all-feature tests, Clippy with warnings denied, and whitespace checks. Run `cargo fmt` when formatting code changes. Review full-rebuild transaction boundaries, query volume, file I/O, allocations and memory whenever ingestion changes; benchmark representative before/after datasets when throughput or resource use can change, reporting only aggregate results. Bounded browsing features also require measured query and response limits.

Document migration/rebuild/restart requirements for each delivery. Derived schema changes must support a complete rebuild from raw sources; accounts, permissions and manual annotations require separate persistent-state migrations and backup/restore. This roadmap and TODO consolidation are documentation-only: no migration, rebuild or restart is needed.
