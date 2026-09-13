# Source health and optional collection

Agent Lens exposes source health from its disposable SQLite index at the authenticated `/health` route and through the local `health` CLI command:

```bash
cargo run -- --database agent-lens.sqlite health
```

Neither operation reads copied agent homes, starts a refresh, opens a network connection, or executes source metadata. Opening the page therefore cannot initiate SSH. Run `refresh` or `rebuild` separately to update the observations.

## Health semantics and privacy

The view reports the latest ingest's status, fixed failure reason, start/finish time, duration, file/error counts, aggregate `ok`/`partial`/`error` source counts, and at most 100 source-file observations per page. Failed refresh discovery, layout validation, and traversal retain a small `failed` run record after all projection changes roll back; their only details are `source_discovery`, `source_layout_mismatch`, `source_traversal`, or `ingest_failed`. Each source row has an opaque, reproducible source-file ID, the existing source ID, source adapter and area, projection status, observation time, and freshness. A transcript link appears only when an indexed transcript exists and uses the stable public session ID; its label never includes session content. Direct-home indexes omit host and user exactly as the other dashboards do.

Freshness uses the index server's clock:

- `recent`: observed no more than 15 minutes ago, allowing up to five minutes of negative age for ordinary clock drift;
- `stale`: observed more than 15 minutes ago;
- `clock skew`: the source observation is over five minutes in the future;
- `unavailable`: absent or unparsable observation time.

These labels describe copied-source observations. They never claim that a machine is online/offline or that an agent is running. Raw paths and parser errors are not rendered or printed. Failures are reduced to the fixed reason codes `malformed_records`, `duplicate_identity`, `source_changed_during_read`, `invalid_session_metadata`, `source_io`, and `import_failed`. The CLI is likewise bounded to 100 displayed observations (with one extra row queried only to detect omission) and contains no transcript-derived titles or bodies.

No rebuild is required. The disposable index automatically adds the nullable ingest failure-reason column; existing observation timestamps are populated by the next refresh.

## Collector design gate

Optional remote collection is deliberately not implemented yet. It must be push-based and separately enabled; the browser and read-only server routes will never shell out, follow an attach command, or initiate SSH. An implementation must satisfy this contract before it can be enabled:

1. A collector writes a versioned JSON summary to an application-owned spool, using a temporary file plus `fsync` and atomic rename. Version 1 is an allowlist containing only a stable random collector ID, stable source ID, collector epoch, monotonic sequence, generated time, and minimal session summaries: stable source-native session ID, observation time, coarse lifecycle evidence, and `transcript_available`. Host/user labels are optional policy-controlled aliases. Paths, prompts, messages, tool inputs/results, raw metadata, environment values, credentials, authentication files, and attach commands are forbidden.
2. Collection requires an explicit local policy naming allowed source adapters, permitted aliases, redacted fields, destination, maximum summary size, interval, and retention. The collector refuses unknown fields at its export boundary so a source-format expansion cannot silently widen collection.
3. Transport must authenticate both endpoints (mutual TLS or an equivalently authenticated, integrity-protected delivery). Secrets live outside summaries and copied homes. The receiver authorizes collector IDs and records only sanitised rejection codes.
4. Deduplication keys are `(collector_id, epoch, sequence)`. Replays with the same payload digest are idempotent; the same key with a different digest is rejected. Lower sequences are rejected after a bounded replay window. Epoch rotation requires reauthorization and does not change the preserved source ID.
5. Received summaries and summary-only projections have explicit count/age limits and expiry. Suggested defaults are 10,000 sessions per collector, a seven-day replay window, and 30-day summary retention. Expiry removes only the disposable projection, never raw sources.
6. Receiver time is authoritative for delivery freshness. Generated times over five minutes in the future are labelled `clock skew`; a collector becomes `stale` after its configured interval plus grace and `unavailable` after expiry. “Collector unavailable” means no timely authenticated summary was received, not that its machine is offline.
7. Summary-only sessions retain the supplied stable source identity in a separate projection kind. Lists label them `summary only`; transcript routes are absent and display `Transcript unavailable`. If transcript-backed data later arrives for the same source/session identity, it enriches that projection rather than creating a duplicate.

Synthetic acceptance tests for a future implementation must cover authenticated replay, conflicting duplicate keys, epoch rotation, expired/stale collectors, future clock skew, retention limits, summary-to-transcript enrichment, and malicious metadata containing credential paths or executable attach strings.

## Ingest performance review

Retaining failed-run health moves the existing run insert immediately before discovery and adds one successful-run layout update. It does not add source-file reads, per-record queries, or allocations. A debug-build benchmark over 1,000 tiny synthetic two-message rollouts compared the pre-change and current binaries on the same warm filesystem:

| Operation | Median before | Median after | Peak RSS before / after | Process read/write chars before / after |
| --- | ---: | ---: | ---: | ---: |
| Rebuild (3 runs) | 1,219 ms | 1,210 ms | 16,672 / 16,716 KiB | 2,407,398 / 2,415,650 read; 11,263,355 / 11,267,723 write |
| Unchanged refresh (5 runs) | 1,200 ms | 1,221 ms | 15,828 / 15,924 KiB | 1,497,400 / 1,505,652 read; 4,527,482 / 4,531,602 write |

The approximately 8 KiB of additional process reads and 4 KiB of writes reflect the extra small run record transaction. Elapsed and RSS differences were within roughly 2% and 100 KiB; physical `read_bytes`/`write_bytes` remained zero under the warm filesystem cache. The health page separately renders at most 100 rows and its regression response stays below 32 KiB.
