# Context, rate pressure, and diagnostics

The authenticated `/diagnostics` page reports bounded historical evidence from copied source files. It does not query a provider, claim a current subscription allowance, automatically block work, or assign task-quality scores.

## Context and rate observations

Codex `event_msg/token_count` records are retained even when `payload.info` is null. AgentLens stores these nullable source fields on the ordered usage observation:

- `info.model_context_window` and `info.last_token_usage.total_tokens`, where the latter is the reported latest active context size rather than cumulative session usage;
- rate-limit ID/name and plan label where present;
- primary and secondary used percentages, window minutes, and either absolute `resets_at` or older relative `resets_in_seconds` evidence;
- the source's `rate_limit_reached_type` when present.

The context display is `last_token_usage.total_tokens / model_context_window`; it does not use cumulative session totals. Missing halves remain unavailable. Negative values, nonpositive windows, context use above the window, and percentages outside 0–100 are labeled inconsistent rather than corrected.

Every row identifies the session and agent; host and user are shown only for a host/user index. The source-provided limit name or ID and plan label are displayed without treating either as an account entitlement. Rows link to the exact source-ordered diagnostics timeline event.

Freshness is evaluated when the page is requested. An observation no more than 15 minutes old is labeled recent. Older, future-dated, or unparseable timestamps are stale or unavailable. A rate reading whose absolute reset has passed is expired; relative resets are evaluated from the observation timestamp. Expired and stale readings remain historical evidence and are never presented as live state.

## Deterministic summaries

Four source-derived totals exclude hidden Codex auto-review sessions:

- interrupted turns count structured interruption outcomes, without claiming that the task failed;
- incomplete turns count reconstructed `unclosed` turns, without claiming that work is currently active;
- accounting anomalies count usage observations with a reset, missing required values, or inconsistent totals/subsets;
- observed tool failures count results carrying explicit failure evidence; missing results remain unknown.

Each category lists at most 20 recent evidence links with stable tie-breaking. Pressure tables return at most 50 observations. The transcript's `diagnostics` filter is separate from the ordinary all-events timeline so accounting traffic does not displace conversation.

## Upgrade, verification, and performance

The automatic schema migration adds nullable pressure columns without inventing values for legacy usage rows. Codex parser version 7 causes one `refresh` to repopulate them; no full rebuild is required. Synthetic regressions cover missing `info`, absent/null rate data, absolute and relative resets, sparse and unknown fields, inconsistent ranges, freshness states, direct-home origin suppression, hidden sessions, exact evidence links, bounds, legacy migration, and rebuild parity.

Release builds before and after this change were measured on 20 generated sessions containing 10,000 token-count events (5.2 MiB), including 1,000 rate-only events with null token info. Three alternating warm-cache rebuilds used separate SQLite outputs and no private data.

| Build | Median elapsed | Range | Median sampled peak RSS | Index bytes |
| --- | ---: | ---: | ---: | ---: |
| Before pressure fields (`4d48185`) | 0.900 s | 0.887–0.902 s | 13,268 KiB | 7,294,976 |
| Context/rate evidence | 1.022 s | 1.000–1.057 s | 13,412 KiB | 7,839,744 |

The wider parameterized usage insert increased median elapsed time by about 13.6%, sampled median RSS by about 1.1%, and the generated index by 544,768 bytes (about 7.5%). It adds no extra per-event row or file pass. All three resulting indexes had identical aggregate projections and passed SQLite integrity checks. These warm-cache results are workload-specific, and polling can miss brief memory peaks.
