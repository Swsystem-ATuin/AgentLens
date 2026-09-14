# Hermes Agent request-snapshot adapter

Hermes Agent support is intentionally limited to provider request snapshots copied beneath:

```text
data/{hostname}/{os_user}/.hermes/sessions/request_dump_*.json
```

A single `.hermes/` directory can also be supplied directly. Directory symbolic links are supported under the same immutable-snapshot contract as the other adapters. The adapter ignores `sessions.json` and unrelated files in the directory.

This contract was observed on 2026-09-15 and compared with the upstream [Hermes session-storage documentation](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/developer-guide/session-storage.md). Current Hermes versions describe `.hermes/state.db` as the canonical transcript store. The `request_dump_*.json` files are failure diagnostics containing the cumulative provider request at one failed attempt, not authoritative session exports. Agent Lens does not currently read `state.db`, profiles, the legacy routing mirror, or exported Hermes JSONL.

## Normalization

Each request dump becomes one request-attempt session. The stable external identity combines the recorded Hermes session ID with a digest of the dump filename so multiple retry snapshots do not overwrite each other. Agent Lens retains:

- the snapshot timestamp as observed activity, without claiming it is session creation or completion time;
- the requested model;
- OpenAI Responses-style `instructions` and `input` messages, function calls, and function-call outputs;
- Anthropic-style `system` and `messages` text, tool-use, and tool-result blocks;
- a fixed, sanitised provider-failure notice derived from a small allowlist of known reason codes.

The first readable user message supplies the bounded fallback title. Instructions render collapsed, ordinary user/assistant conversation remains open, and the final sanitised failure notice renders open. Unknown fields, reasoning items, provider tool definitions, and unsupported content blocks are ignored. A malformed JSON snapshot is a localized file error and does not abort other files. Lifecycle status, cwd, turn completion, and token usage remain unavailable rather than being inferred from a failed request.

## Credential boundary

Hermes request dumps can contain plaintext credentials and provider responses. Agent Lens never copies request headers, request URLs, error objects, response text, encrypted reasoning, or arbitrary unknown fields into SQLite. Normalized raw JSON is reconstructed only from the selected message, tool, model, timestamp, and allowlisted failure-reason fields.

Message and tool bodies are transcript evidence and can themselves contain private or secret material. Protect the raw copy and both Agent Lens databases accordingly. The synthetic regression fixture places credential sentinels in excluded transport fields and verifies they do not appear in session metadata or normalized raw records. Tests never read the private development source.

## Refresh and rebuild

No schema migration is required. After adding a `.hermes` home to a host/user source tree, run:

```bash
cargo run -- refresh --data data
```

For a directly supplied home, use `cargo run -- refresh --data /path/to/.hermes`. Switching an existing direct-home index between agent types requires `rebuild`.

## Performance review

The release build was measured before and after the adapter against the same warm, read-only development source tree, writing separate disposable indexes under `/tmp`. The baseline ignored the then-unknown Hermes home and indexed 182 sessions, 10,107 messages, and 17,728 tool calls in 15.047 seconds with 24,508 KiB peak RSS. The adapter build additionally indexed all 33 Hermes request dumps (3,826,681 bytes), producing 215 sessions, 10,249 messages, and 17,959 tool calls in 28.144 seconds with 24,880 KiB peak RSS. A Hermes-only direct-home rebuild took 13.463 seconds with 16,160 KiB peak RSS.

The added elapsed time is dominated by reading request dumps through the configured external directory link and transactionally indexing each attempt; peak memory increased by 372 KiB for the combined workload. Each snapshot is parsed independently in two stable-file passes, so allocation is bounded by one decoded request dump and its normalized events rather than total Hermes source size. These single-run local measurements are not general latency guarantees, and process/filesystem cache effects were not isolated. Counts and resource measurements disclose no transcript content, paths, identifiers, or credentials.
