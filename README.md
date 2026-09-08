# Agent Lens

Agent Lens is a local, self-hosted Rust web application for indexing and browsing coding-agent activity. Codex is the first supported source, while the storage model uses generic agents, hosts, users, and instances so other agents can be added later.

Raw agent files are always the source of truth. The SQLite database is a disposable local index: it can be deleted and rebuilt without changing anything under `data/`.

## Current functionality

- Discovers Codex homes at `data/{hostname}/{os_user}/.codex/`.
- Reads rollout JSONL from `sessions/` and `archived_sessions/` recursively.
- Normalizes sessions, messages, tool calls, token usage, source files, and ingest runs into SQLite.
- Continues past malformed files and records source-level parsing failures.
- Batches index writes in a transaction so full rebuilds remain efficient as transcript volume grows.
- Provides a sessions list with known host, user, model, and status values as dropdown filters, plus a collapsible transcript drill-down UI that minimizes recognized initialization sections and groups consecutive agent replies into one block with separate paragraphs.

The observed fixture format is documented in [docs/fixture-format.md](docs/fixture-format.md).

Transcript setup blocks are labeled and collapsed even after a session resumes, including repository instructions, permissions, environment, skills, and mode/model changes. Interruption notices and user actions remain expanded with distinct labels. Unknown tags and markers quoted within ordinary prose remain conversation content. This display change needs no index rebuild; restart the server with `cargo run -- serve` after updating.

## Requirements

- A recent stable Rust toolchain
- A copied Codex home beneath `data/{hostname}/{os_user}/.codex/`

Fixture data and generated SQLite indexes are intentionally excluded from Git.

## Run locally

Rebuild the index from raw data:

```bash
cargo run -- rebuild --data data
```

Start the web server:

```bash
cargo run -- serve
```

The development server defaults to `127.0.0.1:3001`. Open <http://127.0.0.1:3001/sessions>. Use `--bind` to select another address.

Use a different database path when needed:

```bash
cargo run -- --database /tmp/agent-lens.sqlite rebuild --data data
```

The `AGENT_LENS_DB` environment variable provides the same override.

## Development

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

Tests use a small redacted rollout fixture under `tests/fixtures/`; they do not depend on private data under `data/`.

## Branching and releases

During initial MVP development, changes are committed directly to `develop`:

- `main` contains released work.
- `develop` contains all ongoing MVP work.
- Feature branches are intentionally deferred until the project needs them.

Record user-visible changes in [CHANGELOG.md](CHANGELOG.md) under `Unreleased`. Releases follow semantic versioning where practical.

The canonical repository is <https://git.s4w.uk/infrastructure/agent-lens.git>.

## Privacy and data handling

Codex transcripts may include prompts, source code, tool output, paths, and other sensitive material. Do not commit files from `data/`, generated databases, or unredacted excerpts. Ingest treats source homes as read-only and only writes to the selected SQLite index.
