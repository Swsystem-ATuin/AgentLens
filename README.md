# Agent Lens

Agent Lens is a local, self-hosted Rust web application for indexing and browsing coding-agent activity from Codex, Gemini CLI, and Hermes Agent. Its storage model uses generic agents, hosts, users, and instances so additional sources can be added without imposing source-specific concepts.

Raw agent files are always the source of truth. The SQLite database is a disposable local index: it can be deleted and rebuilt without changing anything under `data/`.

SQLite remains a deliberate fit for the current single-host, single-index-writer deployment. Both the disposable index and persistent application state use WAL; PostgreSQL is deferred until multiple instances, remote storage, sustained concurrent writers, measured lock pressure, or managed high-availability requirements exist. See the [database backend reassessment](docs/database-backend-decision.md).

Source-specific behavior is isolated behind [source adapters](docs/source-adapters.md). Discovery, raw-record interpretation, parser versioning, capabilities, and presentation markers live in dedicated Codex, Gemini, and Hermes modules; normalized database orchestration and web rendering remain source-neutral. The web application separately defines its [route, query, and presentation boundaries](docs/web-architecture.md).

## Current functionality

For automation, see the dedicated [API reference](docs/api.md) for endpoints, response
fields, authentication, scopes, errors, rate limits, and request examples.

- Discovers Codex, Gemini CLI, and Hermes Agent homes at `data/{hostname}/{os_user}/{.codex,.gemini,.hermes}/`, or accepts one agent home directly.
- Reads Codex rollout JSONL from `sessions/` and `archived_sessions/`, plus Gemini current JSONL and legacy JSON chats from `tmp/*/chats/`, recursively. Gemini setup context is collapsed without becoming a turn or title, and current content-part tools remain paired with their results. See the [Gemini adapter contract](docs/gemini-adapter.md).
- Reads canonical Hermes sessions from `state.db` and named-profile databases through consistent read-only SQLite snapshots, scheduled-run artifacts from `cron/output/*/*.md`, and separate `sessions/request_dump_*.json` provider-failure attempts. Metadata-only cron audit records remain indexed but are hidden from the sessions browser by default; its “All cron runs” option reveals them. Sensitive transport, routing, billing, configuration, error, and reasoning fields are excluded. See the [Hermes adapter contract](docs/hermes-adapter.md).
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
- Provides individually authenticated [application accounts](docs/application-authentication.md) with Argon2id password hashes, server-side sessions, login/logout, inactivity and absolute expiry, and immediate disable/reset/revocation enforcement. Accounts and sessions use a persistent SQLite state file separate from the disposable index; local administrator commands retain initial provisioning and recovery.
- Provides [audited browser administration](docs/browser-administration.md) at `/admin/accounts` for accounts, teams, grants, credential resets, session/token revocation, and security review. Changes require a fresh password and exact-target confirmation, preserve last-admin safeguards, and commit with before/after audit facts. Shared password throttling leaves CLI recovery available without automatic lockouts.
- Supports opt-in [TOTP two-factor authentication](docs/totp-mfa.md), one-use recovery codes, and administrator-required enrollment/reset controls. MFA secrets are encrypted with an owner-only, separately backed-up keyring; replay protection, verification throttling, key rotation, and local recovery preserve the authentication boundary.
- Supports opt-in [passkey second factors](docs/passkey-second-factors.md) alongside TOTP. Passwords remain required; configure an exact public HTTPS URL and Secure cookies before enabling WebAuthn. Named devices, shared recovery, replay/counter checks, and administrator resets retain the MFA boundary.
- Includes an offline [OIDC configuration preflight](docs/oidc.md): `agent-lens oidc-check --config oidc.json`. Add `--discover` to explicitly check the pinned issuer's metadata over bounded, verified HTTPS, or `--prepare` to also load protected client credentials and provider signing keys without writing state. With `--prepare --deployment --public-url https://lens.example.test --secure-cookies`, also check declared deployment prerequisites and an existing enabled administrator password record read-only; this does not certify live proxy or MFA readiness. Local administrators can provision exact issuer/subject links using `account link-oidc`, inspect metadata using `account list-oidc`, and recover using `account unlink-oidc`; link changes revoke existing credentials without changing passwords, MFA, or permissions. The internal bridge includes gated local MFA, rotated sessions, bounded browser-bound runtime contexts, provider logout forms, and signed back-channel logout with durable atomic replay and provider-session-specific revocation. State schema v11 preserves local credentials and sessions; back up state/keyring before upgrading. Production routes and federated sign-in remain disabled pending production HTTP routing and deployment verification.
- Lists and serves authenticated [Codex-generated images](docs/generated-images.md) retained under each copied `.codex/generated_images/<native-session-id>/` directory, with compact lazy thumbnail cards for direct transcript links, unlinked desktop-generated media, and the session gallery. Files remain read directly from the immutable copied home, are source-grant scoped, and are never copied into SQLite.
- Enforces [source-scoped access permissions](docs/access-permissions.md): administrators see all sources; viewers, report-only auditors, and operational operators use the union of direct and team grants. [Teams and roles](docs/teams-and-roles.md) define exact capabilities and last-administrator safeguards. Authorization scopes queries before pagination and aggregation.
- Downloads deterministic, authenticated [redacted postmortems](docs/redacted-postmortems.md) from each visible session as Markdown or JSON. Exports contain aggregate outcomes, usage, model/tool observations, and bounded evidence-gap reason codes while excluding transcript bodies, paths, credentials, and other sensitive source fields.
- Reduces cumulative token snapshots into deterministic per-field session totals without double-counting. The authenticated [historical usage dashboard](docs/usage-dashboard.md) groups observation-time deltas by day, week, or month and by session, model, project, host, user, or agent, with cache ratios, recent transcript drill-down, anomaly counts, and explicit partial/unavailable evidence. See [token accounting semantics](docs/token-accounting.md).
- Continues past malformed lines and files, retaining valid records and reporting hard failures, partial files, and malformed-line totals separately.
- Refreshes new or changed sources using BLAKE3 checksums, skipping unchanged successful imports and preserving previous records if an update fails.
- Streams each Codex rollout in two record-at-a-time passes while batching index writes transactionally, bounding memory to individual record size instead of total transcript size. Gemini retains its final active message set to apply upstream checkpoint and rewind semantics. Hermes streams its logical database checksum and loads one canonical session at a time from a consistent source transaction. See [bounded rollout ingestion](docs/ingestion-memory.md), the [Gemini capability boundaries](docs/gemini-adapter.md#capability-boundaries), and the [Hermes adapter contract](docs/hermes-adapter.md).
- Provides a sessions list with known filter values and 50-session pages with stable last-activity navigation that preserves every active filter. The [bounded transcript reader](docs/transcript-sections.md) loads 50-message sections, lazily chunks unusually large messages, minimizes recognized initialization sections, and groups consecutive agent replies into one block with separate paragraphs. Host and user fields are omitted when the index was built from one direct agent home.
- Derives each session's coarse lifecycle status from source-record order. Later completion or interruption events win; subsequent turn or user activity resets an earlier terminal outcome to `unknown`, which does not imply the copied session is currently running. See [session status semantics](docs/session-status.md).

Sessions whose model starts with `codex-auto-review` (case-insensitive) are hidden from the sessions list, model dropdown, and direct transcript URLs. Their indexed data is retained. Restart the server after updating; no refresh or rebuild is required for this visibility rule.

The observed fixture format is documented in [docs/fixture-format.md](docs/fixture-format.md).

Transcript setup blocks are labeled and collapsed even after a session resumes, including repository instructions, permissions, environment, skills, app/plugin instructions and recommendations, app context, image-resize notices, multi-agent roles, and mode/model changes. Interruption notices and user actions remain expanded with distinct labels. Unknown tags and markers quoted within ordinary prose remain conversation content. No full rebuild is needed; run `cargo run -- refresh --data data` to repair fallback titles previously derived from newly recognized setup blocks, then restart the server with `cargo run -- serve`.

## Requirements

- A recent stable Rust toolchain
- OpenSSL development headers and `pkg-config` for native builds, plus the matching OpenSSL runtime (included in the container image)
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

`AGENT_LENS_DATA` supplies the source path for `refresh` and `rebuild` when
`--data` is omitted, which is useful for service installations.

Refresh checksums every source file, including files with unchanged sizes or timestamps. It imports new, changed, unverified, or outdated-parser files and skips successfully indexed matching versions. The CLI reports imported sessions, skipped files, hard file errors, partial files, malformed lines, files seen, and removed sessions/source files. After successful discovery, refresh removes missing source files and their indexed sessions, messages, tools, and usage; discovery errors abort and roll back projection changes while retaining only a fixed sanitised failure code and timing for health diagnostics. A direct-agent refresh is accepted for a new index or one whose previous successful rebuild/refresh used the same direct-home adapter. Changing adapters or switching between direct-home and host/user layouts requires `rebuild`, preventing unlabeled and labeled origins from being mixed. A browser reload only reads SQLite; run this command to index updated copied sources.

Refresh deterministically prefers active session files and then the lexically earliest logical path when duplicate source files claim one identity. It can replace an older noncanonical winner so repeated refresh and a fresh rebuild converge. For coherent periodic copies, publish a completed immutable versioned directory by atomically retargeting a stable directory symlink; the exact [snapshot, append-validation, retry, and duplicate rules](docs/refresh.md#snapshot-publication-contract) are documented with performance results.

Source directories may be symbolic links to nightly backups, including the data root, host/user folders, agent homes, and session/chat subdirectories. Keep the same link paths between refreshes and publish/retarget backups before starting refresh. Broken links or directory loops cancel refresh and preserve the existing index; links to individual files are unsupported. Update the binary and run refresh; no rebuild is needed.

Existing databases are upgraded automatically: files without hashes are reimported once, source/session identities are backfilled, transcript FTS is built once when absent, and legacy paired tool outputs become order-unknown timeline results. No full rebuild is required. Run one refresh after updating; parser-version changes reimport affected sources to recover their current projection. Hermes parser version 3 classifies cron artifacts with and without body output so metadata-only audit records can be hidden by default. Legacy usage and tool rows remain preserved, but deliberately have no calculated summaries, pressure values, or invented turn associations until the relevant refresh. Checksums, normalized records, and subsequent search-index updates are committed together; failed or partial replacements retain the previous transcript and are retried on the next refresh. New partial files still expose their valid records. See [refresh behavior and verification](docs/refresh.md) for archive moves, duplicate identities, source publication, and limitations.

Explicitly discard and rebuild the index from raw data (this also stores BLAKE3 hashes for subsequent refreshes):

```bash
cargo run -- rebuild --data data
# Or rebuild from one home without host/user labels:
cargo run -- rebuild --data /path/to/.codex
cargo run -- rebuild --data /path/to/.gemini
cargo run -- rebuild --data /path/to/.hermes
```

Rebuild imports into a separate staging database, validates it, and only then replaces the existing index. A failed rebuild preserves the last usable index. Stop the web server before rebuilding and restart it afterward; publication refuses to replace an index that is still open. See [safe full rebuilds](docs/rebuild.md) for failure behavior, staging-file cleanup, validation, and performance results.

For personal use on the same machine, start without creating an account:

```bash
cargo run -- serve --local
```

In this mode, `/account` still offers display and browsing preferences, saved only
in the current browser (including All rows/events). Authentication, identity, and
security-management controls remain unavailable; see [local-mode preferences](docs/user-preferences.md#local-mode-preferences).

Open <http://127.0.0.1:3001/sessions> to review activities, usage, diagnostics, and reports. Local mode accepts only a numeric loopback `--bind` address (`127.0.0.1` or `::1`), and anyone who can reach that port can read the indexed content. It creates the owner-only application state database but requires no account or password. No index rebuild is required when switching between modes.

For authenticated access, create the first application account. Put the password in an owner-only temporary file so it does not appear in shell history or process arguments:

```bash
(umask 077; touch /tmp/agent-lens-password)
${EDITOR:-vi} /tmp/agent-lens-password
cargo run -- account create --username alice --password-file /tmp/agent-lens-password
```

Remove the temporary file through your operating system's secure local workflow after provisioning. Password files must be UTF-8, owner-only on Unix, at least 7 characters and at most 1,024 bytes after one trailing newline is removed, contain no control characters, and include at least one uppercase letter, lowercase letter, number, and symbol. The password is stored only as a salted Argon2id PHC hash.

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

The development server defaults to `127.0.0.1:3001`. Open <http://127.0.0.1:3001/sessions>, <http://127.0.0.1:3001/usage>, <http://127.0.0.1:3001/diagnostics>, <http://127.0.0.1:3001/health>, or the machine-readable <http://127.0.0.1:3001/metrics>. Use `--bind` to select another address. Browser requests are redirected to `/login`; signed-in users can change their own password or explicitly sign out from the Account page.

Without `--local`, authentication is required for every data route, including direct transcript URLs, lazy fragments, exports, health, and metrics. The server refuses to start without an enabled account. Browser sessions expire after 30 minutes of inactivity or 12 hours total. Responses use `Cache-Control: no-store`; login and logout are CSRF-protected and login rotates the opaque session identifier.

Accounts and sessions default to the owner-only `agent-lens-state.sqlite`, independently of the disposable `agent-lens.sqlite` index. Use the global `--state-database /path/to/state.sqlite` option or `AGENT_LENS_STATE_DB` for another location. Password reset, disable, enable, and account-wide session revocation use `account reset-password`, `account disable`, `account enable`, and `account revoke-sessions`; see the [complete operational contract](docs/application-authentication.md). Refresh and rebuild never need or modify application accounts.

The first account must be `admin`; later accounts default to `viewer`. Administrators copy stable `src_…` IDs from Source health and manage direct grants with `account grant-source`/`revoke-source`, roles with `account set-role`, and teams with `team` commands. `account permissions` and the signed-in `/account/access` page explain effective access. Only admins and operators can read installation-wide health and metrics. See the [permission workflow](docs/access-permissions.md) and [team commands, role boundaries, and schema-v6 migration](docs/teams-and-roles.md).

For access beyond localhost, put the app behind an HTTPS reverse proxy, restrict direct access to the backend port, and pass `serve --secure-cookies` (or set `AGENT_LENS_SECURE_COOKIES=true`). The built-in server serves HTTP and defaults to loopback; credentials and cookies require HTTPS in transit.

Use a different database path when needed:

```bash
cargo run -- --database /tmp/agent-lens.sqlite rebuild --data data
```

The `AGENT_LENS_DB` environment variable provides the same override.

## Run as a service

Production examples are included for both major Linux init styles:

- systemd uses `agent-lens.service` for the web process and a one-shot
  `agent-lens-refresh.service` triggered every minute by
  `agent-lens-refresh.timer`;
- SysV init uses an `/etc/init.d/agent-lens` service, while cron invokes a
  lock-protected refresh wrapper every minute so refreshes never overlap.

This repository's local host instead uses the optional fifteen-minute calendar
timer override (`:00`, `:15`, `:30`, `:45`) and the current debug build for refresh;
see [changing the interval or paths](docs/service-management.md#changing-the-interval-or-paths).

Both use one `/etc/agent-lens/agent-lens.env` file for database, copied-source,
bind, authentication-mode, cookie, and optional project settings. The shipped
configuration binds to loopback and keeps mutable state beneath
`/var/lib/agent-lens`. See [service management and scheduled refresh](docs/service-management.md)
for complete installation, account provisioning, permissions, logging, interval,
and rebuild instructions.

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

Signed-in users can create and revoke [scoped personal access tokens](docs/personal-access-tokens.md) from the Account area for metrics, redacted report exports, and live-status JSON. Each token has one read scope and a 1–90-day expiry; its bearer value is revealed only when created.

Browser and metrics access follow the [typed capability matrix](docs/capabilities.md), with source grants applied before queries and pagination. Unknown routes and unsupported methods fail closed for all authenticated roles.

Planned features and architecture follow-ups are tracked only in [ROADMAP.md](ROADMAP.md), informed by the [comparison of codex_viewer, codex-dash, and codex-blackbox](docs/feature-comparison.md). The roadmap includes a dependency-ordered M5 account-management plan covering preferences, session/security history, scoped tokens, capability-based authorization, browser administration, stronger identity options, and conditional notifications. Update the roadmap as work is completed or requirements change.

Internal web failures return a generic response. Server diagnostics record only an allowlisted operation, error category, optional database/IO code, HTTP status, and request method; they do not record request URIs, authorization headers, database error text, or transcript content. See [web error handling](docs/web-errors.md).

Multiple application identities and source-scoped permissions are implemented following the [framework decision](docs/multi-user-framework-decision.md). Accounts, roles, grants, sessions, [typed user preferences](docs/user-preferences.md), and [sanitized session/security history](docs/session-security.md) persist independently of the rebuildable agent index. Signed-in users can personalize the interface, set an optional display name, review password age and security history, and revoke individual or all other browser sessions from the Account area. Rows/events per page can be 25, 50, 100, or All; All shows every matching authorized row/event on one page and may take longer for large histories. Message/tool body previews and fixed diagnostic caps remain independent.

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

The repository also pins [Trunk](https://trunk.io) and its Rust, shell, Docker,
TOML, Markdown, whitespace, and secret-scanning checks. Install the Trunk launcher,
then review the complete tree without applying fixes:

```bash
curl https://get.trunk.io -fsSL | bash
trunk check --all --no-fix
```

Trunk manages the repository's pre-commit formatting and pre-push checking hooks after
initialization. Run the complete regression checks before each commit with
`bash scripts/check.sh` (formatting, all-target/all-feature tests, Clippy, packaging,
Trunk's repository-wide linters, and whitespace checks). Every behavioral change must
add or extend regression coverage in the normal `cargo test` suite. For bug fixes,
first demonstrate that the new test fails without the fix, then passes with it.
Documentation-only changes require verification of the affected instructions and links.

Tests use small synthetic or redacted fixtures under `tests/fixtures/`; they do not depend on private data under `data/`. The setup-envelope inventory must match the Codex presentation contract exactly, and every entry is tested for initial and resumed rendered HTML, collapsed state, title derivation, and ordinary conversation preservation. Add each newly observed setup envelope to that fixture first so an unimplemented classification fails the normal test suite.

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
