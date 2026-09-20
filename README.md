# Agent Lens

Agent Lens is a local, self-hosted Rust web application for indexing and browsing coding-agent activity from Codex, Gemini CLI, and Hermes Agent request snapshots. Its storage model uses generic agents, hosts, users, and instances so additional sources can be added without imposing source-specific concepts.

Raw agent files are always the source of truth. The SQLite database is a disposable local index: it can be deleted and rebuilt without changing anything under `data/`.

SQLite remains a deliberate fit for the current single-host, single-index-writer deployment. Both the disposable index and persistent application state use WAL; PostgreSQL is deferred until multiple instances, remote storage, sustained concurrent writers, measured lock pressure, or managed high-availability requirements exist. See the [database backend reassessment](docs/database-backend-decision.md).

Source-specific behavior is isolated behind [source adapters](docs/source-adapters.md). Discovery, raw-record interpretation, parser versioning, capabilities, and presentation markers live in dedicated Codex, Gemini, and Hermes modules; normalized database orchestration and web rendering remain source-neutral. The web application separately defines its [route, query, and presentation boundaries](docs/web-architecture.md).

## Current functionality

- Discovers Codex, Gemini CLI, and Hermes Agent homes at `data/{hostname}/{os_user}/{.codex,.gemini,.hermes}/`, or accepts one agent home directly.
- Reads Codex rollout JSONL from `sessions/` and `archived_sessions/`, plus Gemini current JSONL and legacy JSON chats from `tmp/*/chats/`, recursively. See the [Gemini adapter contract](docs/gemini-adapter.md).
- Reads Hermes `sessions/request_dump_*.json` provider-failure snapshots while excluding transport headers, URLs, error bodies, encrypted reasoning, and arbitrary unknown fields from SQLite. These snapshots are request attempts rather than canonical completed sessions; `state.db` is not yet read. See the [Hermes adapter contract](docs/hermes-adapter.md).
- Normalizes sessions, ordered turns and model evidence, messages, tool calls and ordered results, token usage, source files, and ingest runs into SQLite.
- Uses reproducible, source-scoped public session IDs so transcript links survive index rebuilds and identical native IDs from different copied homes or agents remain distinct. Session lists show readable titles from valid source advisory names, bounded conversation text, or the native ID in that order. See [session identity and title semantics](docs/session-identity.md).
- Groups working directories into optional host-aware projects and subprojects at request time. The sessions browser includes agent, project, archive-state, and inclusive activity-date filters alongside host, user, model, and status; it displays last observed activity and uses stable activity/public-ID ordering. See [project navigation and date semantics](docs/projects.md).
- Searches readable titles, native IDs, configured projects, and transcript text from the sessions page. SQLite FTS5 provides bounded, safely highlighted transcript previews linked to complete messages while preserving filters and pagination. See [search semantics, limits, and rebuild overhead](docs/search.md).
- Renders bounded transcript content as safe CommonMark with GFM tables, fenced code, strikethrough, and task lists. Raw HTML and unsafe link schemes are neutralized, remote images are reduced to alt text, and a restrictive Content Security Policy blocks automatic resource loads. Ordinary text retains its whitespace-preserving fallback. See [safe rich transcript rendering](docs/rich-transcripts.md).
- Interleaves messages, tool requests/results, and explicit questions/answers in source order. Session timelines provide event-kind filters, stable anchors, bounded previews, lazy detail parts, orphan visibility, and evidence-based observed/failed/unknown labels. See [tool and question timeline semantics](docs/tool-timeline.md).
- Offers disabled-by-default [live transcript updates](docs/live-reading.md) that probe the authenticated index every five seconds and fetch the current bounded section only after a committed refresh. Filters, section position, open blocks, and viewport anchors are preserved; import freshness and browser connection state remain separate from lifecycle evidence.
- Reconstructs ordered turn boundaries, completion, interruption, resumption, requested-model changes, observed served-model fallbacks, and associated tool outcomes. The paginated [turn history](docs/turn-history.md) preserves terminal-only and unclosed work, links to timeline evidence, and shows duration only for supported timestamp pairs.
- Surfaces timestamped [context-window and rate-limit observations](docs/diagnostics.md) with source scope and explicit recent/stale/expired/unavailable states. The diagnostics page provides provenance-backed counts for interruptions, incomplete turns, accounting anomalies, and observed tool failures without claiming live allowance, task failure, or quality scores.
- Reports sanitised [source health](docs/source-health.md) in an authenticated, 100-row-paged view and a local `health` CLI command, including ingest duration, partial/error counts, fixed failure reason codes, and recent/stale/clock-skew observations without exposing paths or parser details. The same document defines the security and retention gates for a future opt-in push collector; no remote collector is enabled today.
- Exposes authenticated, fixed-cardinality [operational metrics](docs/operational-metrics.md) at `/metrics` in Prometheus text format. Gauges cover indexed ingest/source health and supported session, usage, tool, and turn outcomes without labels containing prompts, identities, paths, raw model values, or tool data.
- Provides individually authenticated [application accounts](docs/application-authentication.md) with Argon2id password hashes, server-side sessions, login/logout, inactivity and absolute expiry, and immediate disable/reset/revocation enforcement. Accounts and sessions use a persistent SQLite state file separate from the disposable agent index; local administrator commands own provisioning and recovery.
- Lists and serves authenticated [Codex-generated images](docs/generated-images.md) retained under each copied `.codex/generated_images/<native-session-id>/` directory, with compact lazy thumbnail cards for direct transcript links, unlinked desktop-generated media, and the session gallery. Files remain read directly from the immutable copied home, are source-grant scoped, and are never copied into SQLite.
- Enforces [source-scoped access permissions](docs/access-permissions.md): administrators see all indexed data and operational endpoints, while viewers see only explicitly granted stable source IDs. Authorization scopes lists, search/filter choices, aggregates, transcripts, lazy details, live status, and exports before results are returned.
- Downloads deterministic, authenticated [redacted postmortems](docs/redacted-postmortems.md) from each visible session as Markdown or JSON. Exports contain aggregate outcomes, usage, model/tool observations, and bounded evidence-gap reason codes while excluding transcript bodies, paths, credentials, and other sensitive source fields.
- Reduces cumulative token snapshots into deterministic per-field session totals without double-counting. The authenticated [historical usage dashboard](docs/usage-dashboard.md) groups observation-time deltas by day, week, or month and by session, model, project, host, user, or agent, with cache ratios, recent transcript drill-down, anomaly counts, and explicit partial/unavailable evidence. See [token accounting semantics](docs/token-accounting.md).
- Continues past malformed lines and files, retaining valid records and reporting hard failures, partial files, and malformed-line totals separately.
- Refreshes new or changed sources using BLAKE3 checksums, skipping unchanged successful imports and preserving previous records if an update fails.
- Streams each Codex rollout in two record-at-a-time passes while batching index writes transactionally, bounding memory to individual record size instead of total transcript size. Gemini retains its final active message set to apply upstream checkpoint and rewind semantics. See [bounded rollout ingestion](docs/ingestion-memory.md) and the [Gemini capability boundaries](docs/gemini-adapter.md#capability-boundaries).
- Provides a sessions list with known filter values and 50-session pages with stable last-activity navigation that preserves every active filter. The [bounded transcript reader](docs/transcript-sections.md) loads 50-message sections, lazily chunks unusually large messages, minimizes recognized initialization sections, and groups consecutive agent replies into one block with separate paragraphs. Host and user fields are omitted when the index was built from one direct agent home.
- Derives each session's coarse lifecycle status from source-record order. Later completion or interruption events win; subsequent turn or user activity resets an earlier terminal outcome to `unknown`, which does not imply the copied session is currently running. See [session status semantics](docs/session-status.md).

Sessions whose model starts with `codex-auto-review` (case-insensitive) are hidden from the sessions list, model dropdown, and direct transcript URLs. Their indexed data is retained. Restart the server after updating; no refresh or rebuild is required for this visibility rule.

The observed fixture format is documented in [docs/fixture-format.md](docs/fixture-format.md).

Transcript setup blocks are labeled and collapsed even after a session resumes, including repository instructions, permissions, environment, skills, recommended plugins, app context, multi-agent roles, and mode/model changes. Interruption notices and user actions remain expanded with distinct labels. Unknown tags and markers quoted within ordinary prose remain conversation content. No full rebuild is needed; run `cargo run -- refresh --data data` to repair fallback titles previously derived from these new setup blocks, then restart the server with `cargo run -- serve`.

## Requirements

- A recent stable Rust toolchain
- Copied Codex, Gemini CLI, and/or Hermes Agent homes beneath `data/{hostname}/{os_user}/`, or one copied agent home

Fixture data and generated SQLite indexes are intentionally excluded from Git.

## Run locally

Create or refresh the index from raw data:

```bash
cargo run -- refresh --data data
# Or index one home without host/user labels:
cargo run -- refresh --data /path/to/.codex
cargo run -- refresh --data /path/to/.gemini
cargo run -- refresh --data /path/to/.hermes
```

Refresh checksums every source file, including files with unchanged sizes or timestamps. It imports new, changed, unverified, or outdated-parser files and skips successfully indexed matching versions. The CLI reports imported sessions, skipped files, hard file errors, partial files, malformed lines, files seen, and removed sessions/source files. After successful discovery, refresh removes missing source files and their indexed sessions, messages, tools, and usage; discovery errors abort and roll back projection changes while retaining only a fixed sanitised failure code and timing for health diagnostics. A direct-agent refresh is accepted for a new index or one whose previous successful rebuild/refresh used the same direct-home adapter. Changing adapters or switching between direct-home and host/user layouts requires `rebuild`, preventing unlabeled and labeled origins from being mixed. A browser reload only reads SQLite; run this command to index updated copied sources.

Refresh deterministically prefers active session files and then the lexically earliest logical path when duplicate source files claim one identity. It can replace an older noncanonical winner so repeated refresh and a fresh rebuild converge. For coherent periodic copies, publish a completed immutable versioned directory by atomically retargeting a stable directory symlink; the exact [snapshot, append-validation, retry, and duplicate rules](docs/refresh.md#snapshot-publication-contract) are documented with performance results.

Source directories may be symbolic links to nightly backups, including the data root, host/user folders, agent homes, and session/chat subdirectories. Keep the same link paths between refreshes and publish/retarget backups before starting refresh. Broken links or directory loops cancel refresh and preserve the existing index; links to individual files are unsupported. Update the binary and run refresh; no rebuild is needed.

Existing databases are upgraded automatically: files without hashes are reimported once, source/session identities are backfilled, transcript FTS is built once when absent, and legacy paired tool outputs become order-unknown timeline results. No full rebuild is required. Run one refresh after updating; parser version 7 reimports existing rollouts once to recover exact tool-result order, calculate trustworthy token totals, reconstruct ordered turn/model history, and retain context/rate evidence while also deriving conversation titles and applying optional `session_index.jsonl` names. Legacy usage and tool rows remain preserved, but deliberately have no calculated summaries, pressure values, or invented turn associations until that refresh. Checksums, normalized records, and subsequent search-index updates are committed together; failed or partial replacements retain the previous transcript and are retried on the next refresh. New partial files still expose their valid records. See [refresh behavior and verification](docs/refresh.md) for archive moves, duplicate identities, source publication, and limitations.

Explicitly discard and rebuild the index from raw data (this also stores BLAKE3 hashes for subsequent refreshes):

```bash
cargo run -- rebuild --data data
# Or rebuild from one home without host/user labels:
cargo run -- rebuild --data /path/to/.codex
cargo run -- rebuild --data /path/to/.gemini
cargo run -- rebuild --data /path/to/.hermes
```

Rebuild imports into a separate staging database, validates it, and only then replaces the existing index. A failed rebuild preserves the last usable index. Stop the web server before rebuilding and restart it afterward; publication refuses to replace an index that is still open. See [safe full rebuilds](docs/rebuild.md) for failure behavior, staging-file cleanup, validation, and performance results.

Create the first application account. Put the password in an owner-only temporary file so it does not appear in shell history or process arguments:

```bash
(umask 077; touch /tmp/agent-lens-password)
${EDITOR:-vi} /tmp/agent-lens-password
cargo run -- account create --username alice --password-file /tmp/agent-lens-password
```

Remove the temporary file through your operating system's secure local workflow after provisioning. Password files must be UTF-8, owner-only on Unix, at least 15 characters and at most 1,024 bytes after one trailing newline is removed, and contain no control characters. The password is stored only as a salted Argon2id PHC hash.

Start the web server:

```bash
cargo run -- serve
```

To enable project grouping, copy and edit the safe example, then pass it when serving:

```bash
cp projects.example.json projects.local.json
cargo run -- serve --projects-file projects.local.json
```

`AGENT_LENS_PROJECTS_FILE` provides the same override. Project configuration is loaded only by the web server, never written into copied agent homes, and does not require a refresh or rebuild. Restart the server after editing it.

The development server defaults to `127.0.0.1:3001`. Open <http://127.0.0.1:3001/sessions>, <http://127.0.0.1:3001/usage>, <http://127.0.0.1:3001/diagnostics>, <http://127.0.0.1:3001/health>, or the machine-readable <http://127.0.0.1:3001/metrics>. Use `--bind` to select another address. Browser requests are redirected to `/login`; the Account page provides explicit logout.

Authentication is required for every data route, including direct transcript URLs, lazy fragments, exports, health, and metrics. The server refuses to start without an enabled account. Browser sessions expire after 30 minutes of inactivity or 12 hours total. Responses use `Cache-Control: no-store`; login and logout are CSRF-protected and login rotates the opaque session identifier.

Accounts and sessions default to the owner-only `agent-lens-state.sqlite`, independently of the disposable `agent-lens.sqlite` index. Use the global `--state-database /path/to/state.sqlite` option or `AGENT_LENS_STATE_DB` for another location. Password reset, disable, enable, and account-wide session revocation use `account reset-password`, `account disable`, `account enable`, and `account revoke-sessions`; see the [complete operational contract](docs/application-authentication.md). Refresh and rebuild never need or modify application accounts.

The first account defaults to the `admin` role; later accounts default to `viewer`. Administrators copy stable `src_…` IDs from the Source health page and manage access locally with `account grant-source`, `account revoke-source`, `account set-role`, and `account permissions`. Viewers cannot access installation-wide health or metrics. See the [permission workflow and disclosure contract](docs/access-permissions.md).

For access beyond localhost, put the app behind an HTTPS reverse proxy, restrict direct access to the backend port, and pass `serve --secure-cookies` (or set `AGENT_LENS_SECURE_COOKIES=true`). The built-in server serves HTTP and defaults to loopback; credentials and cookies require HTTPS in transit.

Use a different database path when needed:

```bash
cargo run -- --database /tmp/agent-lens.sqlite rebuild --data data
```

The `AGENT_LENS_DB` environment variable provides the same override.

## Run with Docker

Build the production image from the repository root:

```bash
docker build --tag agent-lens .
docker volume create agent-lens-state
```

The image runs as an unprivileged user, listens on port 3001, and stores both SQLite databases in `/var/lib/agent-lens`. Keep that directory on the named volume. Mount copied agent homes read-only at `/data`; the raw files remain external to the image and are never modified.

Build the disposable index from the mounted sources:

```bash
docker run --rm \
  --mount type=bind,src="$(pwd)/data",dst=/data,readonly \
  --mount type=volume,src=agent-lens-state,dst=/var/lib/agent-lens \
  agent-lens rebuild --data /data
```

Create the first account using an owner-only password file from the host. The short-lived shell copies standard input to an owner-only file inside the container so the unprivileged runtime user can read it without weakening host file permissions:

```bash
docker run --rm --interactive \
  --mount type=volume,src=agent-lens-state,dst=/var/lib/agent-lens \
  --entrypoint /bin/sh agent-lens \
  -c 'umask 077; password_file=/tmp/account-password; cp /dev/stdin "$password_file"; exec /usr/local/bin/agent-lens account create --username alice --password-file "$password_file"' \
  < /tmp/agent-lens-password
```

Then run the authenticated web application:

```bash
docker run --detach --name agent-lens \
  --publish 3001:3001 \
  --mount type=volume,src=agent-lens-state,dst=/var/lib/agent-lens \
  agent-lens
```

Open <http://127.0.0.1:3001/sessions>. Run future `refresh --data /data` commands in a temporary container with the same read-only data mount and state volume. Stop the web container before a full `rebuild`, because rebuild publication refuses to replace an index held open by the server. For HTTPS deployments, terminate TLS at a reverse proxy and add `--env AGENT_LENS_SECURE_COOKIES=true` to the serving container.

## Development

Planned features and architecture follow-ups are tracked in [ROADMAP.md](ROADMAP.md), informed by the [comparison of codex_viewer, codex-dash, and codex-blackbox](docs/feature-comparison.md). Update the roadmap as work is completed or requirements change. The former TODO checklist has been consolidated into the roadmap.

Internal web failures return a generic response. Server diagnostics record only an allowlisted operation, error category, optional database/IO code, HTTP status, and request method; they do not record request URIs, authorization headers, database error text, or transcript content. See [web error handling](docs/web-errors.md).

Multiple application identities and source-scoped permissions are implemented following the [framework decision](docs/multi-user-framework-decision.md). Accounts, roles, grants, and sessions persist independently of the rebuildable agent index, completing the multi-user deployment gate while keeping future per-user features on the same stable identity boundary.

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Run the complete regression checks before each commit with `bash scripts/check.sh` (formatting, all-target/all-feature tests, Clippy, and whitespace checks). Every behavioral change must add or extend regression coverage in the normal `cargo test` suite. For bug fixes, first demonstrate that the new test fails without the fix, then passes with it. Documentation-only changes require verification of the affected instructions and links.

Tests use a small redacted rollout fixture under `tests/fixtures/`; they do not depend on private data under `data/`. Transcript display tests cover rendered HTML, initial and resumed setup, and ordinary conversation remaining visible.

The [historical regression audit](docs/regression-audit.md) maps earlier changes to their tests and records remaining coverage limits. The suite also exercises CLI rebuilds, parser resilience, transaction rollback, foreign keys, authentication, and database-backed filtering.

## Branching and releases

During initial MVP development, changes are committed directly to `develop`:

- `main` contains released work.
- `develop` contains all ongoing MVP work.
- Feature branches are intentionally deferred until the project needs them.

Record user-visible changes in [CHANGELOG.md](CHANGELOG.md) under `Unreleased`. Releases follow semantic versioning where practical.

The [release procedure](docs/releasing.md) defines package verification, the `develop` to `main` fast-forward, annotated tags, and the 0.1.0 upgrade checklist.

The canonical repository is <https://git.s4w.uk/infrastructure/agent-lens.git>.

## License

Copyright (C) 2026 Agent Lens contributors.

Agent Lens is free software licensed under the [GNU General Public License, version 3 or later](LICENSE). You may use, modify, and redistribute it under those terms. Distributed modified versions must preserve the required notices, identify their changes, and make the corresponding source available under the same license.

## Privacy and data handling

Codex transcripts may include prompts, source code, tool output, paths, and other sensitive material. Do not commit files from `data/`, generated databases, or unredacted excerpts. Ingest treats source homes as read-only and only writes to the selected SQLite index.
