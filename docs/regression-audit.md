# Historical regression audit

Audit date: 2026-09-09. Scope: the implemented behavior introduced from the initial MVP through `3d69dd2`, tested against the current checkout. This is a coverage audit, not a claim that every historical checkout or every possible input passes.

Run all checks with `bash scripts/check.sh`. Tests use temporary directories and synthetic or redacted records, never private development sources. The audit adds tests only; no migration, index rebuild, or server restart is required.

## Coverage by change

| Change | Regression verification |
| --- | --- |
| `8c0a505` — initial MVP | Existing fixture ingestion and home discovery; new schema-variant, malformed-line, partial-tail, missing-metadata, missing-ID, archived-source, tool-pairing, token-usage, raw-field retention, and source-immutability tests in `src/ingest/regression.rs`. CLI tests rebuild twice, check counts and SQLite integrity, and verify no source changes. Database tests check foreign keys on all five pool connections and persistence after reopening. Route tests exercise stored sessions, message ordering, missing/invalid IDs, empty transcripts, and escaped content. |
| `4ea987d` — collapsible messages and port 3001 | Rendered `<details>` and expanded-state assertions; CLI help verifies the documented loopback port. |
| `42db29e` — consecutive replies | Existing grouping test plus database-backed HTML assertions that both replies remain separate paragraphs inside one block. Setup/notice tests verify intervening messages prevent grouping. |
| `09e9338` — initialization recognition | Existing tests cover recognition independent of position/count, descriptive titles, and initial setup collapsing. Ordinary “You are” prose after conversation begins remains visible. |
| `c10b232` — filters and identity columns | Database-backed route tests cover every filter individually, combined matching and conflicting filters, empty values, SQL-like literal input, newest-first ordering, deduplicated options, selected values, and options outside the current filter. Existing HTML checks verify separate host/user columns. |
| `3690bc0` — transactional rebuild writes | Injected finalization failure verifies the entire ingest rolls back while preexisting rows survive; retry verifies committed run counts and completion timestamp. Repeated CLI rebuild tests verify successful replacement using closed database connections. |
| `b7ce086` — resumed setup and notices | Existing classification cases plus rendered HTML for every supported setup marker after resume; interruption and user-action notices stay expanded. Unknown tags and quoted markers remain visible. |
| `ebed8d7` — authentication | Existing unauthorized/authorized route matrix, malformed and duplicate headers, invalid configuration, and no-store checks; new tests cover populated authenticated routes, mixed-case Basic scheme, downstream credential removal, and startup failing before database creation. |
| `cab77d3` — multi-agent role | Initial and resumed classification and rendered-HTML regression tests, including ordinary prose and assistant output containing the marker. |
| `3d69dd2` — shared regression checks | Run the script itself, validate its Bash syntax, and confirm the full unit and CLI suites are included. |
| `888412f`, `01fd77c`, `2ad5938` — repository policies | Review current `AGENTS.md` and README for push policy, direct development on `develop`, and rebuild-performance requirements; these are workflow instructions, not application behavior. |
| `db7d6d6`, `9812607` — roadmap and multi-user planning | Review README/TODO consistency: roadmap items remain explicitly unimplemented, and multiple application users are a planned requirement rather than an existing feature. |

## Validation results

The suite passes 23 tests: 20 unit/route/database tests and three CLI integration tests. Formatting, Clippy with warnings denied, shell syntax, local documentation links, and diff whitespace checks pass.

Four temporary mutations independently produced test assertion failures: discarding the first grouped reply, reversing session sort order, replacing the ingest transaction with an autocommit connection, and leaving the authorization header visible downstream. Each mutation was restored before the final suite run. These checks demonstrate that the added coverage detects those regressions; they are not exhaustive mutation testing.

## Limits and outstanding work

The passing suite protects the covered current behavior. It does not resolve the existing TODO items concerning preservation of the old index during failed/live rebuilds, silently skipped discovery errors, internal error disclosure, event-order-based status, or bounded memory. The rollback test covers an ingest transaction, not preservation of the old database across the CLI's destructive rebuild step. The parser tests cover representative variants; they do not establish support for all future schemas or accurate aggregate counting of partial-line errors.

No production ingest code changes in this audit. Transaction boundaries, query volume, file I/O, allocations, and memory behavior are unchanged; a throughput benchmark would not measure a new implementation and was not run.
