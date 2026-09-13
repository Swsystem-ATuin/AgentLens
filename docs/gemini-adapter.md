# Gemini CLI source adapter

Agent Lens supports Gemini CLI as its second source adapter. The contract is pinned to Gemini CLI commit [`9c1b0a610534d6f8120964cf2672c07807d8fc90`](https://github.com/google-gemini/gemini-cli/tree/9c1b0a610534d6f8120964cf2672c07807d8fc90), specifically the official [recording types](https://github.com/google-gemini/gemini-cli/blob/9c1b0a610534d6f8120964cf2672c07807d8fc90/packages/core/src/services/chatRecordingTypes.ts), [recording service](https://github.com/google-gemini/gemini-cli/blob/9c1b0a610534d6f8120964cf2672c07807d8fc90/packages/core/src/services/chatRecordingService.ts), and [storage layout](https://github.com/google-gemini/gemini-cli/blob/9c1b0a610534d6f8120964cf2672c07807d8fc90/packages/core/src/config/storage.ts). A future upstream format change must be handled by a parser-version update and synthetic regression before it is treated as supported.

## Discovery and privacy boundary

For a host/user data root, the adapter discovers:

```text
data/{hostname}/{os_user}/.gemini/tmp/{project}/chats/**/*.{jsonl,json}
```

A single `.gemini/` directory can also be supplied directly. Direct-home rebuilds and refreshes omit host and operating-system user values in the same way as direct Codex homes. A host/user root may contain Codex and Gemini homes together; the agent kind and source home are part of session identity, so equal native session IDs do not collide.

Only chat recordings below `tmp/*/chats/` are read. The adapter does not inspect Gemini settings, OAuth material, environment files, project registries, or other `.gemini` state. Raw homes remain immutable.

## Supported recording semantics

Current recordings are append-style JSONL. The first metadata record supplies `sessionId`, `projectHash`, `startTime`, and optional `summary`, `directories`, and `kind`. Message records have `id`, `timestamp`, `type`, content, and optional Gemini-only model, tool-call, thought, and token fields. Legacy `.json` recordings containing metadata plus a `messages` array are also accepted.

A tiny synthetic shape is checked in at [`tests/fixtures/gemini/session.jsonl`](../tests/fixtures/gemini/session.jsonl). It contains placeholder text only. The adapter implements upstream loader behavior relevant to the final transcript:

- a later message with the same ID replaces the prior value while retaining its position;
- `$rewindTo` removes the named message and everything after it;
- `$set.messages` replaces the active message checkpoint, while other `$set` keys update metadata;
- malformed JSONL lines are skipped and counted with bounded, content-free diagnostics;
- unknown metadata fields, message fields, and record types do not abort the file.

User messages become ordinary user conversation. `gemini` messages become assistant conversation, followed by any embedded tool calls/results and a non-additive per-turn token observation. `info` messages are labelled “Gemini information” and collapsed; `warning` and `error` messages are labelled and remain open. This classification applies equally to initial and resumed messages and does not alter stored content.

## Capability boundaries

The adapter records supported conversation text, the last observed Gemini model, embedded tool calls/results, and per-message token observations. The first recorded workspace directory is used as best-effort project attribution because the format does not separately identify a primary working directory. A recording summary is used as its advisory title, with the first user message as fallback.

Gemini token objects are turn-local observations, not cumulative session counters. They remain visible as raw per-turn evidence, but Agent Lens deliberately leaves session totals and historical usage deltas unavailable rather than summing values that lack stable accounting semantics. Gemini `thoughts` tokens map to the generic reasoning-output field; thought content itself is not projected into the transcript. The upstream `tool` token subset has no generic storage field and remains available only in retained raw JSON.

The recording format does not provide a trustworthy terminal lifecycle event, context-window pressure, rate-limit state, requested-model history, or explicit turn-completion boundaries. Gemini sessions therefore retain lifecycle `unknown`, no inferred end time, and unavailable unsupported diagnostics. `lastUpdated` is metadata about file activity only. Absence of these capabilities is not treated as a parse error.

Gemini rewind and checkpoint semantics require retaining the final active message set in memory. Memory is proportional to active transcript content rather than merely the largest JSONL record, and normalization briefly queues the resulting events. Codex keeps its bounded two-pass streaming behavior. Aggregate synthetic measurements are recorded in [source adapter boundaries](source-adapters.md#compatibility-and-verification).

## Updating an index

No schema migration is required. Run `refresh` to discover Gemini homes alongside existing sources, or `rebuild` to create a fresh disposable index. Changing an existing index between host/user and direct-home layouts still requires `rebuild`; changing from a direct `.codex` home to a direct `.gemini` home also changes that recorded layout.
