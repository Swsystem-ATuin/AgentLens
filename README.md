# Agent Lens

Agent Lens is a local, self-hosted Rust web application for indexing and browsing coding-agent activity. Codex is the first supported source, while the storage model uses generic agents, hosts, users, and instances so other agents can be added later.

Raw agent files are always the source of truth. The SQLite database is a disposable local index: it can be deleted and rebuilt without changing anything under `data/`.

Source-specific behavior is isolated behind [source adapters](docs/source-adapters.md). Codex discovery, JSONL interpretation, lifecycle rules, parser versioning, and presentation markers live in the Codex adapter; normalized database orchestration and web rendering remain source-neutral. The web application separately defines its [route, query, and presentation boundaries](docs/web-architecture.md).

## Current functionality

- Discovers Codex homes at `data/{hostname}/{os_user}/.codex/`, or accepts one `.codex/` directory directly.
- Reads rollout JSONL from `sessions/` and `archived_sessions/` recursively.
- Normalizes sessions, messages, tool calls, token usage, source files, and ingest runs into SQLite.
- Continues past malformed lines and files, retaining valid records and reporting hard failures, partial files, and malformed-line totals separately.
- Refreshes new or changed sources using BLAKE3 checksums, skipping unchanged successful imports and preserving previous records if an update fails.
- Streams each rollout in two record-at-a-time passes while batching index writes transactionally, bounding memory to individual record size instead of total transcript size. See [bounded rollout ingestion](docs/ingestion-memory.md).
- Provides a sessions list with known host, user, model, and status values as dropdown filters and 50-session pages with stable newest-first navigation that preserves active filters. The [bounded transcript reader](docs/transcript-sections.md) loads 50-message sections, lazily chunks unusually large messages, minimizes recognized initialization sections, and groups consecutive agent replies into one block with separate paragraphs. Host and user fields are omitted when the index was built from one direct `.codex/` home.
- Derives each session's coarse lifecycle status from source-record order. Later completion or interruption events win; subsequent turn or user activity resets an earlier terminal outcome to `unknown`, which does not imply the copied session is currently running. See [session status semantics](docs/session-status.md).

Sessions whose model starts with `codex-auto-review` (case-insensitive) are hidden from the sessions list, model dropdown, and direct transcript URLs. Their indexed data is retained. Restart the server after updating; no refresh or rebuild is required for this visibility rule.

The observed fixture format is documented in [docs/fixture-format.md](docs/fixture-format.md).

Transcript setup blocks are labeled and collapsed even after a session resumes, including repository instructions, permissions, environment, skills, multi-agent roles, and mode/model changes. Interruption notices and user actions remain expanded with distinct labels. Unknown tags and markers quoted within ordinary prose remain conversation content. This display change needs no index rebuild; restart the server with `cargo run -- serve` after updating.

## Requirements

- A recent stable Rust toolchain
- Either copied Codex homes beneath `data/{hostname}/{os_user}/.codex/`, or one copied `.codex/` home

Fixture data and generated SQLite indexes are intentionally excluded from Git.

## Run locally

Create or refresh the index from raw data:

```bash
cargo run -- refresh --data data
# Or index one home without host/user labels:
cargo run -- refresh --data /path/to/.codex
```

Refresh checksums every source file, including files with unchanged sizes or timestamps. It imports new, changed, unverified, or outdated-parser files and skips successfully indexed matching versions. The CLI reports imported sessions, skipped files, hard file errors, partial files, malformed lines, files seen, and removed sessions/source files. After successful discovery, refresh removes missing source files and their indexed sessions, messages, tools, and usage; discovery errors abort the run. A direct `.codex` refresh is accepted for a new index or one whose previous rebuild/refresh also used direct-home mode. Changing between direct-home and host/user layouts requires `rebuild`, preventing unlabeled and labeled origins from being mixed. A browser reload only reads SQLite; run this command to index updated copied sources.

Source directories may be symbolic links to nightly backups, including the data root, host/user folders, `.codex`, and session subdirectories. Keep the same link paths between refreshes and publish/retarget backups before starting refresh. Broken links or directory loops cancel refresh and preserve the existing index; links to individual files are unsupported. Update the binary and run refresh; no rebuild is needed.

Existing databases are upgraded automatically: files without hashes are reimported once. No full rebuild is required. Checksums and normalized records are committed together; failed or partial replacements retain the previous transcript and are retried on the next refresh. New partial files still expose their valid records. See [refresh behavior and verification](docs/refresh.md) for archive moves, duplicate identities, source publication, and limitations.

Explicitly discard and rebuild the index from raw data (this also stores BLAKE3 hashes for subsequent refreshes):

```bash
cargo run -- rebuild --data data
# Or rebuild from one home without host/user labels:
cargo run -- rebuild --data /path/to/.codex
```

Rebuild imports into a separate staging database, validates it, and only then replaces the existing index. A failed rebuild preserves the last usable index. Stop the web server before rebuilding and restart it afterward; publication refuses to replace an index that is still open. See [safe full rebuilds](docs/rebuild.md) for failure behavior, staging-file cleanup, validation, and performance results.

Create the local authentication file and restrict its permissions:

```bash
(umask 077; cp -n auth.example.json auth.local.json)
chmod 600 auth.local.json
```

Edit `auth.local.json` and fill in `username` and `password` with your own username and a strong, unique password. Both placeholders are deliberately empty, so the example cannot be used to log in. The real file and its backup files (`auth.local.json.*`) are ignored by Git; never commit credentials.

Start the web server:

```bash
cargo run -- serve
```

The development server defaults to `127.0.0.1:3001`. Open <http://127.0.0.1:3001/sessions>. Use `--bind` to select another address. Your browser will prompt for the configured username and password before any page is accessible.

Authentication is required for every route, including direct transcript URLs. Missing, malformed, or empty credentials prevent startup. Responses use `Cache-Control: no-store`. Credentials are loaded at startup; restart after changing them. Browsers may retain Basic authentication credentials until closed; there is no application logout button.

To use a file outside the checkout, pass `--auth-file /path/to/auth.json` after `serve`, or set `AGENT_LENS_AUTH_FILE`. Protect that file with owner-only permissions and keep it out of source control. Refresh and rebuild commands do not require credentials. The checkpoint schema upgrade is automatic; no manual migration is needed.

For access beyond localhost, put the app behind an HTTPS reverse proxy and restrict direct access to the backend port. [HTTP Basic authentication requires HTTPS to protect credentials in transit](https://developer.mozilla.org/en-US/docs/Web/HTTP/Guides/Authentication). The built-in server serves HTTP and defaults to loopback.

Use a different database path when needed:

```bash
cargo run -- --database /tmp/agent-lens.sqlite rebuild --data data
```

The `AGENT_LENS_DB` environment variable provides the same override.

## Development

Planned features and architecture follow-ups are tracked in [ROADMAP.md](ROADMAP.md), informed by the [comparison of codex_viewer, codex-dash, and codex-blackbox](docs/feature-comparison.md). Update the roadmap as work is completed or requirements change. The former TODO checklist has been consolidated into the roadmap.

Internal web failures return a generic response. Server diagnostics record only an allowlisted operation, error category, optional database/IO code, HTTP status, and request method; they do not record request URIs, authorization headers, database error text, or transcript content. See [web error handling](docs/web-errors.md).

Multiple application users are a planned requirement. The current shared username/password is interim; individual identities, access permissions, and persistent account storage are tracked in the roadmap and are not implemented yet.

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

The canonical repository is <https://git.s4w.uk/infrastructure/agent-lens.git>.

## Privacy and data handling

Codex transcripts may include prompts, source code, tool output, paths, and other sensitive material. Do not commit files from `data/`, generated databases, or unredacted excerpts. Ingest treats source homes as read-only and only writes to the selected SQLite index.
