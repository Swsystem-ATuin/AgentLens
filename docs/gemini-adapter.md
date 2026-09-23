# Gemini CLI source adapter

Agent Lens supports Gemini CLI as its second source adapter. The contract is pinned to Gemini CLI commit [`f50ba8608e489cc94b44b209e0920a9d072020ad`](https://github.com/google-gemini/gemini-cli/tree/f50ba8608e489cc94b44b209e0920a9d072020ad), specifically the official [recording types](https://github.com/google-gemini/gemini-cli/blob/f50ba8608e489cc94b44b209e0920a9d072020ad/packages/core/src/services/chatRecordingTypes.ts), [recording service](https://github.com/google-gemini/gemini-cli/blob/f50ba8608e489cc94b44b209e0920a9d072020ad/packages/core/src/services/chatRecordingService.ts), [session conversion](https://github.com/google-gemini/gemini-cli/blob/f50ba8608e489cc94b44b209e0920a9d072020ad/packages/core/src/utils/sessionUtils.ts), and [storage layout](https://github.com/google-gemini/gemini-cli/blob/f50ba8608e489cc94b44b209e0920a9d072020ad/packages/core/src/config/storage.ts). A future upstream format change must be handled by a parser-version update and synthetic regression before it is treated as supported.

## Discovery and privacy boundary

For a host/user data root, the adapter discovers:

```text
data/{hostname}/{os_user}/.gemini/tmp/{project}/chats/**/*.{jsonl,json}
```

A single `.gemini/` directory can also be supplied directly. Direct-home rebuilds and refreshes omit host and operating-system user values in the same way as direct Codex homes. A host/user root may contain Codex and Gemini homes together; the agent kind and source home are part of session identity, so equal native session IDs do not collide.

Only chat recordings below `tmp/*/chats/` are read. The adapter does not inspect Gemini settings, OAuth material, environment files, project registries, or other `.gemini` state. Raw homes remain immutable.

## Supported recording semantics

Current recordings are append-style JSONL. The first metadata record supplies `sessionId`, `projectHash`, `startTime`, and optional `summary`, `directories`, and `kind`. Message records have `id`, `timestamp`, `type`, content, and optional Gemini-only model, tool-call, thought, and token fields. Current checkpoints can represent tools as `content[].functionCall` and following `content[].functionResponse` parts; older records can instead embed calls and results in `toolCalls`. Legacy `.json` recordings containing metadata plus a `messages` array are also accepted.

A tiny synthetic shape is checked in at [`tests/fixtures/gemini/session.jsonl`](../tests/fixtures/gemini/session.jsonl). It contains placeholder text only. The adapter implements upstream loader behavior relevant to the final transcript:

- a later message with the same ID replaces the prior value while retaining its position;
- `$rewindTo` removes the named message and everything after it;
- `$set.messages` replaces the active message checkpoint, while other `$set` keys update metadata;
- malformed JSONL lines are skipped and counted with bounded, content-free diagnostics;
- unknown metadata fields, message fields, and record types do not abort the file.

Ordinary user messages become user conversation. Internal `<session_context>` and `<hook_context>` user records remain available as descriptively labelled, collapsed evidence, but do not open turns or supply fallback titles. Pure function-response records become tool results rather than empty user messages. `gemini` messages become assistant conversation, followed by modern content-part or legacy embedded tool calls/results and a non-additive per-turn token observation. Thought-marked text is not displayed. `info` messages are labelled “Gemini information” and collapsed; `warning` and `error` messages are labelled and remain open. This classification applies equally to initial and resumed messages and does not alter stored raw evidence.

## Capability boundaries

The adapter records supported conversation text, the last observed Gemini model, modern or legacy tool calls/results, and per-message token observations. The first recorded workspace directory is used as best-effort project attribution. Current main-session metadata can omit `directories`, so the first directory in the structured “Workspace Directories” section of `<session_context>` is used as a fallback; arbitrary conversation text is never interpreted as a path. A recording summary is used as its advisory title, with the first ordinary user message as fallback.

Gemini token objects are turn-local observations, not cumulative session counters. They remain visible as raw per-turn evidence, but Agent Lens deliberately leaves session totals and historical usage deltas unavailable rather than summing values that lack stable accounting semantics. Gemini `thoughts` tokens map to the generic reasoning-output field; thought content itself is not projected into the transcript. The upstream `tool` token subset has no generic storage field and remains available only in retained raw JSON.

The recording format does not provide a trustworthy terminal lifecycle event, context-window pressure, rate-limit state, requested-model history, or explicit turn-completion boundaries. Gemini sessions therefore retain lifecycle `unknown`, no inferred end time, and unavailable unsupported diagnostics. `lastUpdated` is metadata about file activity only. Absence of these capabilities is not treated as a parse error.

Gemini rewind and checkpoint semantics require retaining the final active message set in memory. Memory is proportional to active transcript content rather than merely the largest JSONL record, and normalization briefly queues the resulting events. Codex keeps its bounded two-pass streaming behavior. Aggregate synthetic measurements are recorded in [source adapter boundaries](source-adapters.md#compatibility-and-verification).

## Parser-version 2 performance review

Release binaries immediately before and after the current-part importer were measured on one generated 4,711,422-byte chat containing 10,000 ordinary user messages, 10,000 modern function calls, and 10,000 matching function responses, plus one session-context record. Three alternating warm-cache rebuilds used separate SQLite outputs. The baseline took 1.090 seconds median (0.967–1.127 seconds); parser version 2 took 2.404 seconds median (2.209–2.615 seconds). No private source was read.

The prior parser incorrectly wrote 20,001 messages and no tools. The corrected projection writes 10,001 messages, 10,000 calls, and 10,000 paired results, increasing the generated index from 5,406,720 to 7,077,888 bytes. The roughly 121% median elapsed increase therefore measures 10,000 additional normalized rows plus result-to-call pairing queries, not parsing overhead alone. Source passes and transaction boundaries are unchanged. Gemini still retains its active message set and queued normalized events; the duplicate guard additionally retains IDs only for legacy embedded results, not for current function responses. These deliberately tool-dense local measurements are not general latency guarantees.

## Updating an index

No schema migration is required. Parser version 2 makes the next `refresh` reimport existing Gemini recordings to repair setup presentation, titles, turns, workspace attribution, and modern tool evidence; `rebuild` creates the same projection from a fresh disposable index. Changing an existing index between host/user and direct-home layouts still requires `rebuild`; changing from a direct `.codex` home to a direct `.gemini` home also changes that recorded layout.
