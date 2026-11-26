# Contributing to AgentLens

AgentLens aims to make activity across agents and multi-agent workflows observable.
Codex, Gemini CLI, and Hermes are supported today. Integrations for other agents,
platform testing, accessibility improvements, documentation, and small reproducible
bug reports are welcome. Follow our [code of conduct](CODE_OF_CONDUCT.md).

## Choose and discuss work

Check [existing issues](https://github.com/Swsystem-ATuin/AgentLens/issues) and the
[roadmap](ROADMAP.md) before starting. Use an issue form to report a bug, propose a
feature or adapter, or describe an accessibility barrier. For substantial changes,
explain the problem and proposed behavior in an issue so maintainers can discuss
scope before implementation. Report vulnerabilities through [SECURITY.md](SECURITY.md),
not a public bug report.

## Set up and verify

The [README](README.md#personal-usage-quick-start-linuxmacos) explains native build
dependencies and the personal launcher. For isolated development, use synthetic
agent homes and temporary databases rather than your real transcripts.

```bash
cargo fmt
cargo test
cargo clippy --all-targets --all-features -- -D warnings
bash scripts/check.sh
```

The full check script also requires the repository's pinned Trunk tools and
checks packaging, Markdown, shell scripts, secrets, and whitespace. See
[development checks](README.md#development) for installation and workflow details.
Run `bash scripts/check.sh` before each commit. Tests must never depend on the
private development sources under `data/`.

## Keep source evidence private and rebuildable

- Never commit real prompts, credentials, source code from transcripts, identifying
  paths, unredacted agent artifacts, copied homes, local SQLite files, or build output.
- Use tiny synthetic or fully redacted fixtures in `tests/fixtures/`. Review any
  screenshots and command output before posting them to GitHub.
- Raw agent files are immutable. Keep application metadata outside agent homes.
  Treat SQLite as a disposable index that can be rebuilt from those files.
- Make parsers tolerant of unknown fields, malformed lines, partial recordings,
  and format variants. Retain useful evidence and localize parse errors.

## Implement and test changes

Keep domain boundaries generic. Source-specific interpretation belongs in the
[source adapter layer](docs/source-adapters.md), not generic routes or reporting.
For a new adapter, document the sanitized recording format, discovery locations,
identity rules, parser version, and usage semantics. Add normal-suite tests for
unknown fields, malformed/partial files, and relevant schema variants. New agent
support also needs platform testing and any corresponding launcher discovery.

Every behavioral change needs automated regression coverage. For a bug fix,
demonstrate the test failing without the fix and passing with it. Test observable
behavior; transcript changes need rendered-HTML coverage for initial and resumed
sessions and preservation of ordinary conversation. For ingest changes, review
transaction boundaries and resource use and benchmark representative rebuilds
before and after, reporting aggregate results without private content.

Documentation-only changes need verification of the affected commands and links.
Update the README when behavior, commands, configuration, or architecture changes.
Record noteworthy changes under `Unreleased` in [CHANGELOG.md](CHANGELOG.md), and
state migration, refresh, or rebuild requirements. Use parameterized SQL, keep
foreign keys enabled, and add context to fallible operations without exposing
private data. See [AGENTS.md](AGENTS.md) for the complete repository requirements.

AI-assisted contributions are welcome. Contributors remain responsible for
understanding, reviewing, testing, and licensing everything they submit. Explain
material uncertainty and validation gaps; do not post private agent histories as
proof of work.

## Submit changes

During the MVP, `develop` is the integration branch and `main` holds releases.
Work directly on `develop`; feature branches are currently deferred. External
contributors can fork the repository, commit on their fork's `develop`, and open
a pull request targeting this repository's `develop`. Use the pull request template
to explain the problem, resulting behavior, validation, and remaining risks.

Maintainers with both configured remotes must push each commit to `origin` and
`s4w`, then verify matching branch tips as described in the
[repository workflow](README.md#branching-and-releases). External contributors do
not need access to the private mirror. If a build replaces the local development
service binary on the maintainer host, restart and verify that service before
handoff, as required by AGENTS.md.

Contributions are provided under the project's [GPL-3.0-or-later license](LICENSE).
Only submit material you have the right to contribute. No contributor license
agreement is currently required.
