# Bounded rollout ingestion

Agent Lens imports each rollout in two streaming passes instead of retaining every decoded JSON record for the file.

The first pass hashes the exact source bytes and derives only session-level metadata: native ID, timestamps, working directory, first turn-context model, lifecycle status, raw session metadata, and malformed-line diagnostics. The second pass reopens the unchanged file and writes messages, tool calls, outputs, and usage records one JSON value at a time. File metadata is checked before, between, and after the passes. Any failure rolls back the existing per-file savepoint, preserving the previous projection under the normal refresh rules.

Memory now grows with the largest individual JSONL record and retained session-metadata record, rather than the total number of records in a rollout. Message content and raw JSON serialization are also bounded to the current record. Malformed-line totals remain exact, while at most 20 sanitised line examples are stored; additional lines are represented by an omitted-count suffix. Individual record size is not capped, so a single exceptionally large JSON record can still require substantial memory.

Tool calls and ordered results are inserted as they are read. Results pair to the latest preceding matching call through SQLite rather than an in-memory map. The supporting `(session_id, call_id, sequence)` index is created automatically whenever the database opens. Duplicate call IDs retain deterministic latest-preceding-call behavior, and unmatched outputs remain visible as orphan results. Usage accounting similarly retains only five counter states plus the current record; see [token accounting semantics](token-accounting.md).

## File I/O and query behavior

- Full rebuilds and new or unverified refresh sources read each imported rollout twice: once for summary/hash validation and once for normalized writes.
- A verified changed source is read three times: checkpoint verification, summary/hash validation, and normalized writes.
- An unchanged verified source is read once for checksum verification and is not parsed.
- Transaction and savepoint boundaries, normalized insert counts, and cleanup queries are unchanged.
- Output pairing executes one indexed result insert and compatibility update per output. Usage events execute one ordered insert with bounded delta reduction plus one summary insert per session. Their lookups and uniqueness checks use supporting sequence/call indexes instead of file-sized in-memory maps.

The extra sequential read is the deliberate tradeoff for removing file-sized decoded-record retention. Raw sources remain immutable.

## Performance verification

Release builds at baseline commit `74e4855` and with bounded ingestion were measured on one synthetic rollout containing 60,000 messages and 65,220,094 bytes. Three alternating warm-cache rebuilds per binary used the same source and separate SQLite paths. Elapsed time includes startup, two-phase database publication, and integrity validation. A `/proc` sampler polled resident memory. The corpus contained only generated placeholder text; no private fixture was read.

| Implementation | Median elapsed | Observed range | Peak RSS range |
| --- | ---: | ---: | ---: |
| Whole-file decoded records | 7.513 s | 7.429–7.678 s | 213,508–213,688 KiB |
| Two-pass record streaming | 6.140 s | 6.096–6.518 s | 14,744–14,860 KiB |

Median elapsed time improved by about 18.3%. Sampled peak RSS fell by roughly 93%, from about 208.6 MiB to 14.5 MiB. These warm-cache results favor sequential rereads and do not predict cold or remote storage. Polling can miss brief memory peaks, but the approximately 199 MiB separation is much larger than the sampling variation.

Regression coverage verifies normalized refresh/rebuild parity, lifecycle order, first-context model semantics, partial JSON/UTF-8 handling, bounded diagnostics with exact totals, transactional replacement rollback, duplicate tool-call IDs, unmatched outputs, source immutability, and checksum behavior. Run `bash scripts/check.sh` from the repository root.
