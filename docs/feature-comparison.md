# Feature review: Codex viewers and observability tools

Reviewed on 2026-09-10 against AgentLens commit `23641a1cd0b489b74d3540212ca69492faa449a1`. This is a public documentation and selected source review, not a runtime evaluation or performance benchmark. The names in the request are interpreted as the repositories below. Links pin the reviewed revisions because upstream features can change.

## Reviewed projects

| Project | Reviewed revision | Main role |
| --- | --- | --- |
| [someonegg/codex_viewer](https://github.com/someonegg/codex_viewer/blob/d14662c41b20654069d3b5f0f3746270e8392bb5/README.md) | `d14662c` | Browser-based history reader |
| [ArnabCodes/codex-dash](https://github.com/ArnabCodes/codex-dash/blob/4bbe168ba07e445be10dd4834b6bc3f89c038f12/README.md) | `4bbe168` | Terminal session board across machines |
| [softcane/codex-blackbox](https://github.com/softcane/codex-blackbox/blob/1e1a9311fc031f0003e3d88d503f5f63fed51e4f/README.md) | `1e1a931` | Proxy-based observation of model requests and responses |

**codex_viewer:** reads active and archived rollouts with project/date/archive filters, stable session links, Markdown/GFM/math, opt-in live reading, and optional interaction with an existing bound tmux pane. Its documented protections include TLS/mTLS options, safe Markdown, blocked remote images, and path containment. It also reads advisory thread names; it does not require Codex databases. See the pinned README above.

Its [filtering contract](https://github.com/someonegg/codex_viewer/blob/d14662c41b20654069d3b5f0f3746270e8392bb5/docs/session-jsonl-filtering.md) describes paged timelines, lazy technical details, separate tool calls/results, question/answer cards, event visibility controls, and bounded diagnostics. [Incremental loading](https://github.com/someonegg/codex_viewer/blob/d14662c41b20654069d3b5f0f3746270e8392bb5/docs/adr/0012-use-checkpointed-incremental-rollout-loading.md) resumes from committed newline boundaries and falls back to full decoding after invalidation; its limited file probes can miss some historical edits. [Thread names](https://github.com/someonegg/codex_viewer/blob/d14662c41b20654069d3b5f0f3746270e8392bb5/docs/adr/0015-expose-session-index-names-as-presentation-metadata.md) remain presentation metadata rather than authoritative session discovery.

**codex-dash:** combines session search/sorting, project/subproject grouping and context, activity indicators, usage/cache/context/rate-limit views, account labels, and machine/origin information. A watcher refreshes local exports; explicit sync pools machine/session JSON without copying full rollouts or authentication files. Terminal controls can launch, resume, attach, assign, or hide sessions. Usage periods group sessions by update time, so those figures are not necessarily tokens consumed during each period. See its pinned README above.

The [implementation](https://github.com/ArnabCodes/codex-dash/blob/4bbe168ba07e445be10dd4834b6bc3f89c038f12/codex_board.py) uses process presence and machine freshness as well as transcript events for activity inference. Its hiding/bin functions update board metadata; the reviewed functions do not delete raw rollouts. Board snapshots can still contain private prompts, paths, and account labels.

**codex-blackbox:** observes Codex Responses traffic through Envoy and a Rust core, with Grafana, Prometheus, CLI watch/status/guard, prerequisite checks, and redacted postmortems. It reports response outcomes, latency, token mix, context pressure, model fallback and tool-call intent. Dashboard headline counters cover the current core process lifetime; durable SQLite evidence supports historical reports. Guard advice cannot interrupt a response already streaming. Desktop/UI routing is documented as experimental. See its pinned README above.

Its [architecture](https://github.com/softcane/codex-blackbox/blob/1e1a9311fc031f0003e3d88d503f5f63fed51e4f/ARCHITECTURE.md) separates evidence sources and confidence, treats cached input as part of input, and avoids private/high-cardinality metric labels. Tool intent does not establish local execution success. Dormant coach/baseline/tmux code is outside the default product surface; it is not counted as delivered functionality here.

## Comparison with AgentLens

The external columns below summarise the sources above. AgentLens findings come from [ingestion](../src/ingest.rs), [schema](../schema.sql), [routes](../src/web.rs), [templates](../templates/session.html), and [CLI](../src/main.rs). “Absent” means absent from the reviewed user-facing functionality, even where raw data or schema fields could support it.

| Capability | codex_viewer | codex-dash | codex-blackbox | AgentLens today / roadmap |
| --- | --- | --- | --- | --- |
| Primary evidence | Rollouts and advisory names | Local state, rollouts, pooled board exports | Observed model traffic | Copied rollouts across hosts/users; retain this baseline |
| Find sessions | Project/date/archive filters and titles | Search, sorting, project hierarchy | Session selection for supervision | Host/user/model/status filters; add titles, projects, date/archive filters and search in R2 |
| Read conversations | Rich, paged timeline with technical-event controls | Session board and terminal handoff | Turn evidence and postmortems | Escaped plain text, collapsed setup, grouped replies; add safe rich text and tool/question views in R2 |
| Usage | Timeline token events | Aggregate usage and pressure dashboard | Per-turn accounting and operational metrics | Cumulative snapshots indexed, no usage UI; R3 |
| Activity and failures | Live session reading | Inferred activity plus machine freshness | Response outcomes and advisory decisions | Coarse completion/abort status; event-order fix R0, evidence-based diagnostics R3 |
| Refresh and scale | Bounded paging and incremental file reads | Watchers, JSON refresh and sync | Live proxy stream | Manual full rebuild; bounded loading R1, incremental updates R4 |
| Multiple machines | Configured local home | Explicit metadata pooling | Wrapped traffic observation | Multiple copied homes already indexed; collection freshness and optional summaries R4 |
| Access | Network boundary; optional TLS/mTLS | Local terminal boundary | Local observability stack | Individual accounts, sessions, roles, and persistent stable source grants |
| Agent control | Opt-in tmux input/preview | Launch/resume/attach | Wrapper and advisory guard | No control surface; keep execution outside this roadmap |
| Additional agents | Codex adapter seam | Codex-specific board | Codex Responses focus | Generic database identities but Codex-coupled ingestion/presentation; R1 then R5 |

## Recommended adaptation

Build on AgentLens's existing host/user model and transcript classification. The largest immediate usability gain is finding a named session and reading its relevant messages and tools without loading everything. The existing usage rows then provide a starting point for historical reporting, after counter semantics are corrected. Turn diagnostics should follow evidence retention and ordered lifecycle handling.

Live observation needs a separate freshness contract: a copied file with a recent timestamp is not proof of a running process. Likewise, rollout timestamps cannot establish Blackbox-style request latency or served-model fallback. Preserve unknown values and identify whether a result is observed, derived, or unavailable.

Adopt the products' useful browsing and reporting ideas within Rust, SQLite, and server-rendered HTML. Project annotations and user accounts need durable application storage outside both the disposable index and copied homes. Optional summary collection and metrics are later extensions, with full transcripts remaining an explicit collection choice.

Do not import terminal execution, proxy routing, credential inspection for account labels, or writes into agent homes as incidental dashboard features. Math rendering is optional after basic Markdown. Framework and database changes remain conditional decisions. The actionable backlog, dependencies, and completion criteria are in [ROADMAP.md](../ROADMAP.md).

## Review verification

All 16 open architecture-review TODO entries were transferred verbatim apart from adding roadmap IDs. The compatibility-only `TODO.md` was subsequently removed so the roadmap is the sole backlog. Validation passed: all 16 original checklist texts remain in the roadmap, all 16 local Markdown links resolve, and all eight cited upstream file links match the public Git trees at the pinned revisions. `bash scripts/check.sh` passed formatting, 23 tests, Clippy with warnings denied, and whitespace checks; the documented command was executed from the repository root. No application behavior, raw sources, database schema, or ingest performance changes are introduced; no migration, rebuild, or restart is required.
