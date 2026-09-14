# Opt-in live reading

Session pages offer a disabled-by-default **Live updates** checkbox. Enabling it stores only the opt-in flag in browser `sessionStorage`, scoped by the stable session ID. No request is made while it is disabled. The setting follows the same session across transcript filters and sections for the lifetime of that browser tab.

Live reading observes the disposable SQLite index; it does not read raw rollouts, run `refresh`, contact an agent process, open SSH, or claim that copied work is active. Publish or copy sources using the [snapshot contract](refresh.md#snapshot-publication-contract), then run `cargo run -- refresh --data ...` separately. Readers see the new projection only after that transaction commits.

## Update protocol and bounds

An enabled visible page requests the authenticated `/sessions/{stable_id}/live-status` JSON endpoint every five seconds. Hidden tabs and failed probes use a ten-second interval. The probe performs one stable-ID-indexed query, returns no transcript content, and applies the same authentication, hidden-session policy, no-store response header, and generic error handling as the transcript.

The revision advances after a successful ingest transaction. Only then does the browser fetch its current URL and replace the `data-live-content` region. That URL retains the selected event filter and section number; a live update never jumps to the newest section. The replacement remains bounded to the normal 51-row query/50-event render ceiling. Open/closed state is restored by stable event anchor, and the nearest source-ordered anchor and viewport offset restore reading position. New events use their server-rendered default state. An archived move keeps the stable session URL and updates its archived label.

The client serializes probes, retries after errors, and labels the index connection `paused`, `connecting`, `connected`, or `disconnected`. A later successful probe reconnects automatically. These are browser-to-index connection states, not agent-process states.

## Import and freshness labels

The live bar keeps three concepts separate from the session's evidence-derived lifecycle status:

- **Last successful import** is the commit time of the ingest run that last projected this session's source file. The revision is that source file's ingest-run ID, so a viewer cannot infer refresh activity belonging only to another source.
- **Source observed** is updated transactionally whenever refresh checks that source, including an unchanged checksum or a localized parse error. It is `recent` for an age from zero through 120 seconds, `stale` when older or future-dated, and `unavailable` when absent or unparsable.
- **Source projection** is the allowlisted `ok`, `partial`, `error`, or `unknown` checkpoint state.

None of these values proves a task or process is running. A browser connection can be healthy while source evidence is stale; a recent source observation can describe a completed or interrupted session.

Existing indexes gain the nullable `source_files.observed_at` column automatically. Run one refresh to populate observations, then restart the server to load the live-reader route and script. No full rebuild or parser-version refresh is required.

## Polling and C1 decision

Polling was selected over SSE because changes originate in a separate CLI process and the server has no in-process publication channel. SSE would retain one connection per reader while still polling SQLite or adding cross-process notification machinery. The revision probe makes unchanged traffic one short query and performs a bounded HTML fetch only after a committed run.

HTMX was evaluated under C1 and not added. It could replace the transcript region, but restoring open details, the viewport anchor, retry state, and opt-in persistence would still require custom JavaScript. The dedicated dependency-free script is under 5 KiB and leaves filtering, pagination, and rendering server-owned. The Content Security Policy permits only this authenticated same-origin script and same-origin fetches; inline and third-party scripts remain blocked.

On a release build with one tiny synthetic session, 500 sequential authenticated loopback probes returned a 184-byte body with 0.957 ms mean and 1.148 ms p95 latency. A 500-request bounded transcript sample returned 8,301 bytes with 1.874 ms mean and 2.238 ms p95. A separate 1,000-request, 20-way probe run measured 0.988 ms mean and 1.152 ms p95. At five seconds, 100 visible readers produce about 20 probes per second; transcript fetches occur only after a revision. These warm local measurements are not remote-network guarantees.

Recording observations adds one small update for each unchanged source. On the same generated 40-session, 60,000-message corpus used for the R4.1 review, current medians were 6.025 seconds for rebuild, 0.142 seconds for unchanged refresh, and 0.495 seconds for one changed file. The corresponding R4.1 medians were 6.127, 0.141, and 0.492 seconds; sampled peak RSS changed by 44–92 KiB. The overlapping values do not establish a material regression. File passes, decoded-record allocation, and normalized transcript writes are unchanged.

## Verification

Synthetic regressions cover automatic legacy migration, observation updates for unchanged checksums, recent/stale state, authenticated and hidden-session status access, bounded rendered controls and script delivery, whole-file replacement, readers concurrent with refresh, atomic tool call/result visibility, revision changes, archived moves, and retry/state-preservation code paths. Tests use no private source data.
