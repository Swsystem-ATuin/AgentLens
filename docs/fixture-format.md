# Observed Codex fixture format

The development fixture follows `data/{hostname}/{os_user}/.codex/`. It currently contains active rollout files under `sessions/YYYY/MM/DD/*.jsonl`, plus `history.jsonl`, `session_index.jsonl`, and internal SQLite/cache files. No `archived_sessions` or `instance-board` directory was present during inspection.

Rollout files are JSON Lines. Each record has `timestamp`, `type`, and `payload`. Observed record types include:

- `session_meta`: session UUID, timestamp, cwd, originator, CLI version, source, provider, and instructions.
- `turn_context`: turn UUID, model, cwd, date/timezone, sandbox and collaboration settings.
- `response_item/message`: role plus a content array containing text-bearing items.
- `response_item/function_call` and `custom_tool_call`, paired with corresponding output records by `call_id`.
- `response_item/reasoning`: reasoning summary/content; deliberately not displayed as transcript messages.
- `event_msg/token_count`: cumulative and last token usage plus rate limits.
- lifecycle/user-facing events such as `task_started`, `task_complete`, `turn_aborted`, `user_message`, and `agent_message`.

Session lifecycle status is reduced in source-record order, not timestamp order. `task_complete` records completion and `turn_aborted` records interruption. A later `task_started`, `turn_context`, `event_msg/user_message`, or user-role `response_item/message` is evidence that work resumed and resets the coarse status to `unknown` until another terminal event appears. Other records, including trailing token counts, do not change the last outcome. See [session status semantics](session-status.md).

`history.jsonl` records contain `session_id`, Unix timestamp `ts`, and `text`. `session_index.jsonl` records contain `id`, `thread_name`, and `updated_at`. These are useful enrichment sources but are outside the first vertical slice because rollout files already contain canonical transcript data.

Parsing is intentionally tolerant: unknown fields and event types are retained in stored raw JSON where applicable and ignored for normalization. Malformed lines mark a source `partial`; file-level failures are recorded on `source_files` and do not abort the ingest run.

Resuming a session can append setup as ordinary message records: a user message beginning `# AGENTS.md instructions for `, a developer message beginning `<permissions instructions>`, and a user message beginning `<environment_context>`. Other observed setup prefixes are `<skills_instructions>`, `<multi_agent_mode>`, `<multi_agent_role>`, `<collaboration_mode>`, and `<model_switch>`. These can occur after conversation has begun. Display classification does not change their stored content or message counts.

Messages beginning `<turn_aborted>` occur with user or developer roles and indicate interruptions. A user message beginning `<user_action>` can contain review activity with `<context>`, `<action>`, and `<results>` children. These notices remain expanded in the transcript. Display classification does not derive lifecycle status from message text.
