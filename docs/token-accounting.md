# Token accounting semantics

Agent Lens stores source token observations and derives session totals without summing cumulative snapshots. The normalized contract supports two scopes:

- A cumulative snapshot reports counters for the session as observed at one source record. It is additive only after conversion to per-field deltas.
- A per-turn snapshot reports the source's latest-turn observation. Codex rollouts do not currently provide a stable turn identity on the token-count record, so repeated observations cannot be distinguished safely from two equal-sized turns. These values are retained as non-additive evidence and are never summed into the session total.

Codex `total_token_usage` maps to the cumulative scope and `last_token_usage` maps to the per-turn scope. Token-count events with `info: null`, missing objects, missing fields, and unknown fields remain ordered raw observations rather than disappearing. Reasoning is read from `reasoning_output_tokens` or the observed `reasoning_tokens` alias.

## Cumulative reduction

Accounting follows source-record sequence, not timestamps. This makes out-of-order clocks harmless and treats a resumed session as one accounting history unless its counters actually decrease.

Each field is reduced independently:

1. The first valid non-negative value contributes its full value.
2. A later greater value contributes the difference from the previous valid value.
3. An identical snapshot contributes zero and is marked duplicate.
4. A decrease starts a new counter segment; the new value contributes in full and the observation is marked reset.
5. A missing or invalid field contributes `NULL`, not zero, and does not replace the last valid counter value.

The session summary is the checked sum of those deltas across reset segments. If a field was never reported validly, its summary remains `NULL`. Integer overflow also leaves that field unavailable and marks the observation inconsistent. This reduction is deterministic across rebuilds and keeps memory bounded to five counter states plus the current decoded record.

`input_tokens`, `output_tokens`, and the source-reported `total_tokens` are separate counters. Agent Lens does not replace an absent or inconsistent reported total with a derived value. `cached_input_tokens` is retained as a subset of input and is never added to it. `reasoning_output_tokens` is retained as a subset of output and is never added to output or total.

An observation is marked inconsistent when it contains a negative value, cached input exceeds input, reasoning output exceeds output, `input + output` overflows, or a reported total differs from `input + output`. The original values remain available as evidence; an anomaly flag does not silently rewrite them.

## Stored evidence and limitations

Every `usage_events` row retains the stable source sequence when known, timestamp, cumulative values, per-turn values, computed cumulative deltas, presence/duplicate/reset/missing/inconsistent flags, and raw JSON. `session_usage` stores the per-field reduced totals and exact counts of raw snapshots, per-turn observations, resets, duplicates, missing observations, and inconsistent observations.

These are lifetime session totals. A snapshot timestamp is evidence of when a cumulative value was observed, not proof that all preceding usage occurred in that period. The [historical usage dashboard](usage-dashboard.md) aggregates individual deltas by observation time and states that attribution limitation; it does not assign a session summary to its final day. Values remain unavailable rather than displaying an invented zero.

The schema upgrade is automatic, but old usage rows lack source sequence and computed deltas. Agent Lens preserves them as order-unknown raw evidence and deliberately creates no summary from them. Parser version 5 causes the next `refresh` to reimport rollouts once and produce trustworthy accounting; a full rebuild is optional. Current refresh replaces an affected session projection and is regression-tested against a fresh rebuild. A future truly incremental R4.1 implementation must either seed the reducer from verified prior ordered state or recompute the affected session, and must retain the same parity tests.

## Performance verification

Release binaries at R2.5 commit `f2d6497` and with R3.1 were measured on 20 generated sessions containing 2,000 messages and 10,020 token-count observations. The observations included 10,000 cumulative/per-turn pairs, 200 duplicate snapshots, 40 resets, and 20 unavailable `info: null` events. Three alternating warm-cache rebuilds used the same immutable 4.3 MiB synthetic source and separate SQLite outputs. A `/proc` sampler polled resident memory; no private fixture was read.

| Build | Median elapsed | Range | Sampled peak RSS | Index bytes |
| --- | ---: | ---: | ---: | ---: |
| Before correct accounting (`f2d6497`) | 0.840 s | 0.817–0.844 s | 13,152–13,224 KiB | 6,287,360 |
| Correct accounting | 0.903 s | 0.890–0.924 s | 13,200–13,468 KiB | 6,463,488 |

The median elapsed increase was about 7.5%, and the index grew by 176,128 bytes (about 2.8%). Source passes, file reads, the outer transaction, and per-file savepoints are unchanged. Each usage record now performs one wider parameterized insert and bounded in-process delta reduction; each session with observations adds one summary insert. Sampled memory ranges substantially overlap, though polling can miss brief peaks.

All three resulting summary projections had the same SHA-256 digest. Each contained 20 sessions and reconciled to 994,000 input, 198,800 output, 397,600 cached-input subset, 49,700 reasoning-output subset, and 1,192,800 reported total tokens, alongside the expected observation/anomaly counts. These generated warm-cache measurements are workload-specific, not production latency guarantees.
