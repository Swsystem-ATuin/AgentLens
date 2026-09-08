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

`history.jsonl` records contain `session_id`, Unix timestamp `ts`, and `text`. `session_index.jsonl` records contain `id`, `thread_name`, and `updated_at`. These are useful enrichment sources but are outside the first vertical slice because rollout files already contain canonical transcript data.

Parsing is intentionally tolerant: unknown fields and event types are retained in stored raw JSON where applicable and ignored for normalization. Malformed lines mark a source `partial`; file-level failures are recorded on `source_files` and do not abort the ingest run.
