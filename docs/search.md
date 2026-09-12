# Session and transcript search

The sessions page has one `q` field that composes with every existing filter and is preserved by previous/next links. A session matches when the query appears in its readable title, native session ID, configured project label, or transcript.

Title, native-ID, and project matching is a case-insensitive literal substring comparison using SQLite's built-in `lower` behavior. Transcript matching uses the SQLite FTS5 `unicode61` tokenizer. Whitespace-separated search terms are safely quoted and combined with `AND`; punctuation is interpreted by the tokenizer rather than as FTS query syntax. Queries are trimmed, empty input disables search, and input is limited to 200 Unicode characters.

Transcript hits show one highlighted preview linked to the complete matching message. Highlight markup is generated after escaping every stored character, so transcript HTML cannot inject markup. Metadata-only matches do not invent a message preview. When both setup/configuration and ordinary conversation messages match, the renderer prefers an ordinary match among the eight highest-ranked candidates for that visible session.

## Bounds and access scope

Search keeps the normal 50-session page and fetches at most 51 rows to detect another page. Preview lookup runs only for the 50 visible, already-authorized session IDs and returns at most eight candidates per session (400 rows). Each candidate exposes at most 2,048 characters for setup classification, and the displayed FTS snippet is capped at 512 characters after SQLite selects a 24-token fragment. Search therefore cannot expand a page into an unbounded transcript response.

Rendered regressions use 105 matching sessions and require each 50-result response to remain below 96 KiB. A separate 20,000-character single-token message requires the search response to remain below 64 KiB. The same hidden-model predicate applies before preview lookup, and regressions verify that a hidden transcript cannot appear as a result or snippet. There is currently no separate total-match count; the Messages column remains the indexed message count for each visible session.

## Disposable FTS index and migration

`message_search` is an external-content FTS5 projection of normalized messages. Insert, update, and delete triggers keep it synchronized during refresh, inside the same SQLite transaction as the normalized projection. Cascade deletion is covered so removed sources cannot leave searchable text behind.

On first opening an older disposable database, Agent Lens creates and backfills FTS once. No raw source is changed and no manual rebuild is required. Full rebuilds omit per-message FTS maintenance in the private staging database, bulk-build the projection after ingest, restore its triggers, then perform the existing integrity checks and publication. Repeated rebuilds and refresh deletion are tested for search parity.

## Rebuild performance review

Release builds immediately before and after R2.3 were measured on the existing synthetic 60,000-message, 65.2 MB rollout. Three warm-cache rebuilds per binary used the same immutable generated source and separate SQLite paths. Elapsed time covers startup, staged import, search-index construction, integrity validation, and publication; a `/proc` sampler polled resident memory. No private data was used.

| Implementation | Median elapsed | Observed range | Peak RSS range | Database size |
| --- | ---: | ---: | ---: | ---: |
| Before transcript FTS | 6.493 s | 6.391–6.597 s | 14,700–15,104 KiB | 248,111,104 bytes |
| Bulk-built transcript FTS | 7.948 s | 7.796–8.692 s | 13,320–13,428 KiB | 249,094,144 bytes |

The median elapsed overhead was about 22.4%, while this highly repetitive corpus added 983,040 bytes (about 0.4%) to the database. Sampled peak RSS did not increase, but polling can miss brief peaks. Raw-file passes, normalized insert volume, and the ingest transaction are unchanged. Rebuild adds one SQLite bulk FTS construction before validation; refresh adds one transactional FTS maintenance operation per inserted, updated, or deleted message. FTS allocations remain SQLite-managed and index size will vary substantially with real transcript vocabulary, so these generated warm-cache numbers are not storage or latency guarantees.

Run `bash scripts/check.sh` from the repository root to repeat the regression checks.
