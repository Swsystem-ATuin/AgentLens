# Ordered turn and model history

Each session page links to an authenticated, paginated turn history at `/sessions/{public_id}/turns`. The newest 50 turns are shown first, with stable links back to the source-ordered timeline evidence that supports each boundary or model observation.

## Boundary and outcome semantics

Turn reconstruction follows JSONL source sequence, never timestamp order. `task_started` and `turn_started` open a turn; `turn_context`, `event_msg/user_message`, and a user-role `response_item/message` can also open work when no turn is active. `task_complete` and `turn_complete` close the active turn as `complete`, while `turn_aborted` closes it as `interrupted`. A terminal event without an opening is retained as a terminal-only turn with an unknown start. A distinct structured turn ID closes an open prior turn as `unclosed` before opening the next one.

An opening after a terminal boundary is labeled resumed. A new turn after an unclosed turn is not called resumed because no completed or interrupted boundary supports that claim. Trailing tools, accounting events, file modification time, and a recent last timestamp neither open work nor prove completion or current activity. End-of-file work without terminal evidence remains explicitly `unclosed`.

Duration is displayed only when both opening and terminal timestamps parse successfully and the terminal time is not earlier than the opening time. It is defined as terminal event time minus opening event time. Missing, malformed, or reversed timestamps produce `Unavailable`; source order still determines the outcome.

## Models and tools

`turn_context.model` is requested-model evidence. A later different requested model within the same turn is labeled `Requested model changed`; repeated identical context is not. An observed `model_reroute` is separately labeled `Observed served-model fallback` and uses its structured destination model. Requested and served models are never inferred from one another. Some Codex versions do not persist reroute events, so absence is shown as `Not observed`, not proof that no fallback occurred.

Tool calls and results inherit the active reconstructed turn ordinal. The history summarizes calls plus observed, failed, and unknown result outcomes. Orphan results use the current active turn when available. These associations are source-order observations; AgentLens does not infer a turn from timestamps or retroactively attach tools outside an observed boundary.

## Index upgrade and verification

The automatic schema upgrade adds turn tables and nullable tool-to-turn associations. Legacy rows are preserved, but historical turn boundaries are not invented from them. The current Codex parser version 7 causes one `refresh` to reimport older rollouts and populate exact turn and pressure evidence; a full rebuild is optional.

Synthetic tests cover terminal-only and unclosed work, resumed sessions, distinct turn IDs, requested-model changes versus served fallbacks, successful/failed/unknown tools, deliberately reversed timestamps, missing timestamps and events, source-ordered evidence links, pagination, repeatable rebuilds, and legacy upgrades.

The ingest change was measured with release builds on 20 generated sessions containing 10,000 turns, 31,000 boundary/model events, 10,000 calls, and 10,000 results (about 7.5 MiB of JSONL). Three alternating warm-cache rebuilds used separate SQLite outputs and no private data.

| Build | Median elapsed | Range | Sampled peak RSS range |
| --- | ---: | ---: | ---: |
| Before turn history (`84e2ff3`) | 2.200 s | 2.135–2.223 s | 13,152–13,256 KiB |
| Ordered turn history | 4.341 s | 4.239–4.377 s | 13,240–13,452 KiB |

The additional 10,000 summary rows and 31,000 evidence rows increased median rebuild time by about 97%. The bounded reducer retains only the active turn and reduced the initial implementation's 6.037-second median by about 28%; sampled memory remained effectively flat. All three rebuilt indexes produced identical aggregate projections and passed SQLite integrity checks. These deliberately turn-dense warm-cache measurements are workload-specific.
