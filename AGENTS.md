# Agent development guide

These instructions apply to the entire repository.

## Project principles

- Treat this as a multi-agent observability application. Keep domain names and boundaries generic; isolate Codex-specific parsing behind source-specific modules.
- Raw files under `data/` are immutable sources of truth. Never edit, rename, delete, or write metadata into a copied agent home.
- Treat SQLite as a disposable index. Schema and ingest behavior must support a complete rebuild from raw sources.
- Expect private content in fixtures and transcripts. Never add real prompts, credentials, source code, or identifying data to tests, logs, documentation, or commits.
- Prefer tolerant parsing: ignore unknown fields, retain useful raw records, and record localized errors rather than aborting an ingest run.

## Repository hygiene

- While the initial MVP is under development, commit all work directly to `develop`; do not create feature branches. Continue to reserve `main` for released work.
- Push every commit to both `origin` (GitLab) and `github` (GitHub) in the same task. Verify the updated branch points to the same commit on both remotes before handing off; report any push failure and remaining divergence.
- Keep `README.md` accurate when commands, architecture, configuration, or user-visible behavior change.
- Add noteworthy changes to `CHANGELOG.md` under `Unreleased` in the same change that introduces them.
- Never commit `data/*`, local SQLite indexes, credentials, build output, or unredacted agent artifacts.
- Preserve unrelated working-tree changes.

## Local development service

- On this host, whenever a build or other operation replaces `target/debug/agent-lens`, restart the local `agent-lens` service with `systemctl restart agent-lens.service` and verify that it is active before handing off. If service access requires escalation, request it through the command approval mechanism. Report any restart failure.

## Rust expectations

- Format changes with `cargo fmt`.
- Run `cargo test` for every behavioral change.
- Run `cargo clippy --all-targets --all-features -- -D warnings` before handing off substantial work.
- Avoid panics in production paths. Add context to fallible filesystem, parsing, database, and network operations.
- Keep route handlers thin and move parsing, querying, and domain logic into focused modules as the application grows.
- Use parameterized SQL and keep foreign-key enforcement enabled.
- Review full-rebuild performance whenever import or ingest code changes. Check transaction boundaries, query volume, file I/O, allocations, and memory growth; benchmark representative data before and after changes that could affect throughput or resource use, and report the results without exposing private content.

## Tests

- Every change must include regression verification in the same commit. Behavioral changes must add or extend automated tests for the affected behavior; bug fixes must demonstrate that the regression test fails without the fix and passes with it. For documentation-only changes, validate the affected instructions and links and report that validation.
- Run `bash scripts/check.sh` before committing each change. Keep regression cases in the normal `cargo test` suite, use descriptive failures, and test observable behavior rather than duplicating implementation logic.
- For transcript display changes, verify rendered HTML as well as classification, covering initial and resumed sessions and preservation of ordinary conversation.
- Derive only tiny, synthetic or fully redacted fixtures from observed source shapes.
- Cover unknown fields, malformed lines, partial files, and schema variants when extending an agent parser.
- Tests must not read from the private development fixture under `data/`.

## Documentation

- Document newly observed raw formats in `docs/` without copying sensitive values.
- Include runnable commands and state any migration or rebuild requirements.
