# Tool and question timeline

Session pages interleave messages, tool requests, tool results, explicit user-input questions, and their answers in source-record order. Every item has a stable source-sequence anchor. Tool request/result identity uses the source `call_id` plus an indexed relation to the latest preceding matching request, so repeated IDs do not attach an output to a later or older request. Results without a usable matching request remain visible as orphan results.

Codex `request_user_input` calls are presented as questions and their paired outputs as answers. A question without an observed output remains visible as requested with an unknown outcome; Agent Lens does not infer that it was answered or failed. Other calls are collapsed by default, while questions and answers start open. The event selector can show all events, messages, tools, or questions/answers. Consecutive assistant messages are grouped only when no tool event occurs between them.

## Outcome semantics

The timeline uses evidence labels rather than treating a paired output as successful execution:

- `requested · outcome unknown` means no matching output is indexed.
- `requested · output observed` means at least one matching output exists, without claiming success.
- `requested · failed` and `failed` require explicit source evidence such as `success: false`, `is_error: true`, or a `failed`/`error` status.
- `output observed` describes a retained output with no explicit failure evidence.

The legacy `tool_calls.status = complete` value remains a compatibility summary meaning that an output was paired. New explicitly failed outputs set the compatibility status to `failed`. Neither value is used to claim task success in the timeline.

Structured arguments and outputs are retained as compact JSON. String values are preserved verbatim, including malformed JSON-like text. Missing IDs, missing outputs, unknown fields, and orphan outputs do not abort import. Raw result JSON, source sequence, timestamp, source call ID, pairing identity, and whether exact sequence is known are retained in the disposable index. Legacy output-only call rows are migrated into result rows marked `exact legacy order unavailable` because their original output sequence cannot be reconstructed.

## Bounds and navigation

A timeline query fetches at most 51 events and renders 50 to detect the next section. Each event preview contains at most 2,048 SQLite characters. Longer tool arguments, results, and messages link to authenticated detail routes that read at most 16,384 characters per part. Tool names and call IDs projected into pages are capped at 160 characters. Filters are parameterized, pagination preserves the selected event kind, and detail routes first resolve the public stable session ID so one session cannot retrieve another session's event.

`EXPLAIN QUERY PLAN` on the generated benchmark index selects the message, tool-call sequence, and tool-result sequence indexes and merges the three ordered branches without a temporary sort. The 51-event limit can therefore stop the merged scan; deeper offset sections still read their skipped events.

Rendered regressions cover a 60-event tool-only timeline with 5,000-character bodies. The 50-event first response must remain below 160 KiB and the 10-event second response below 48 KiB. Separate tests exercise both lazy parts of 20,000-character arguments and outputs, invalid page bounds, source ordering, anchors, escaping, filters, and ordinary conversation.

## Index upgrade and refresh

The schema upgrade is automatic. Existing paired call outputs are exposed as order-unknown result rows without duplication on subsequent opens. The Codex parser version is 4, so the first `refresh` after updating reimports each rollout once to recover exact result sequence and raw result provenance. A full rebuild is optional and remains the repair path. Raw source files are never changed.

## Rebuild performance review

Release binaries immediately before and after this change were measured on 20 generated sessions containing 10,000 messages, 10,000 tool calls, and 10,000 paired outputs (about 11 MiB of JSONL). Three alternating warm-cache rebuilds used the same immutable synthetic source and separate SQLite outputs. Elapsed time includes startup, staged import, search-index construction, validation, and publication; a `/proc` sampler polled resident memory. No private fixture was read.

| Build | Median elapsed | Range | Sampled peak RSS | Index bytes |
| --- | ---: | ---: | ---: | ---: |
| Before ordered results (`425272a`) | 1.885 s | 1.768–1.913 s | 13,292–13,412 KiB | 18,173,952 |
| Ordered result timeline | 2.499 s | 2.469–2.500 s | 13,316–13,532 KiB | 25,346,048 |

The median elapsed increase was about 32.6% and the generated index grew by 7,172,096 bytes (about 39.5%). This is expected for the deliberately tool-heavy corpus: every output now adds a parameterized result insert, pairing-index lookup, compatibility-status update, and retained raw result record. The importer keeps its outer transaction and per-file savepoints; source passes and file reads are unchanged, and result handling retains only the current decoded record. Sampled memory ranges overlap. These warm-cache results are workload-specific and polling can miss brief peaks.
