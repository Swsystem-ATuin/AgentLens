# Observed Codex fixture format

The development fixture follows `data/{hostname}/{os_user}/.codex/`. It currently contains active rollout files under `sessions/YYYY/MM/DD/*.jsonl`, plus `history.jsonl`, `session_index.jsonl`, and internal SQLite/cache files. No `archived_sessions` or `instance-board` directory was present during inspection.

Rollout files are JSON Lines. Each record has `timestamp`, `type`, and `payload`. Observed record types include:

- `session_meta`: session UUID, timestamp, cwd, originator, CLI version, source, provider, and instructions.
- `turn_context`: turn UUID, model, cwd, date/timezone, sandbox and collaboration settings.
- `response_item/message`: role plus a content array containing text-bearing items.
- `response_item/function_call` and `custom_tool_call`, paired with corresponding output records by `call_id`. Arguments/inputs and outputs may be strings or structured JSON. Calls to `request_user_input` represent explicit questions and paired outputs represent answers.
- `response_item/reasoning`: reasoning summary/content; deliberately not displayed as transcript messages.
- `event_msg/token_count`: cumulative `total_token_usage` and latest-turn `last_token_usage` plus rate limits. Either usage object or the enclosing `info` may be missing/null. Cached input is a reported subset of input; reasoning output is a reported subset of output.
- lifecycle/user-facing events such as `task_started`/`turn_started`, `task_complete`/`turn_complete`, `turn_aborted`, `user_message`, and `agent_message`.
- optional structured `model_reroute` evidence with source and destination models. Some source versions do not persist this transient event, so its absence does not prove that no fallback occurred.

Session lifecycle status is reduced in source-record order, not timestamp order. `task_complete` records completion and `turn_aborted` records interruption. A later `task_started`, `turn_context`, `event_msg/user_message`, or user-role `response_item/message` is evidence that work resumed and resets the coarse status to `unknown` until another terminal event appears. Other records, including trailing token counts, do not change the last outcome. See [session status semantics](session-status.md).

Turn history uses the same source ordering and accepts the `turn_started`/`turn_complete` aliases. It retains terminal-only and end-of-file unclosed turns, uses structured turn IDs to separate adjacent work, records `turn_context.model` as requested-model evidence, and keeps a persisted reroute's served model distinct. Timestamp order never changes boundaries or outcomes. See [ordered turn and model history](turn-history.md).

`history.jsonl` records contain `session_id`, Unix timestamp `ts`, and `text`. Optional `session_index.jsonl` records contain `id`, `thread_name`, and `updated_at`. AgentLens streams this file as advisory title metadata: valid `id`/`thread_name` string pairs can name sessions already discovered from rollouts, while unknown fields and malformed or partial records are ignored. It never creates a session from the advisory index. `history.jsonl` is not currently indexed because rollout files already contain canonical transcript data.

Parsing is intentionally tolerant: unknown fields and event types are retained in stored raw JSON where applicable and ignored for normalization. Valid records in a new file survive malformed JSON or UTF-8 lines, and the source is marked `partial`. Diagnostics record only source line numbers and the generic reason `invalid JSON or UTF-8`, never the malformed content. Totals remain exact, but stored examples are capped at 20 with an omitted count. Rebuild and refresh summaries count partial files and malformed lines separately from hard file failures. File-level failures are recorded on `source_files` and do not abort the ingest run. Rollouts use the [bounded two-pass importer](ingestion-memory.md).

Token-count observations are retained even when `info` is null. Cumulative counters are converted to source-ordered deltas, while latest-turn values remain non-additive evidence because the raw token record has no stable turn identity. Missing fields remain null; duplicates, resets, inconsistent totals, and invalid subset relationships are flagged without rewriting raw values. See [token accounting semantics](token-accounting.md).

Tool outputs are normalized as their own records with source sequence, timestamp, call ID, pairing relation, output, explicit-failure classification, and retained raw JSON. Pairing selects the latest preceding call with the same non-empty ID; repeated IDs therefore remain deterministic, while missing or unmatched IDs produce visible orphan outputs. A paired output is only evidence that output was observed. Failure requires an explicit `success: false`, `is_error: true`, or `failed`/`error` status on the output record or its output object. See [tool and question timeline semantics](tool-timeline.md).

Resuming a session can append setup as ordinary message records: a user message beginning `# AGENTS.md instructions for `, a developer message beginning `<permissions instructions>`, and a user message beginning `<environment_context>`. Other observed setup prefixes are `<skills_instructions>`, `<multi_agent_mode>`, `<multi_agent_role>`, `<collaboration_mode>`, and `<model_switch>`. These can occur after conversation has begun. Display classification does not change their stored content or message counts.

Messages beginning `<turn_aborted>` occur with user or developer roles and indicate interruptions. A user message beginning `<user_action>` can contain review activity with `<context>`, `<action>`, and `<results>` children. These notices remain expanded in the transcript. Display classification does not derive lifecycle status from message text.

These raw-format and presentation rules are implemented only in the [Codex source adapter](source-adapters.md). Generic ingestion receives normalized records and does not inspect these Codex fields or markers.
