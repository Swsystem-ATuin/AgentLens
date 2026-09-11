# Session status semantics

AgentLens stores a coarse lifecycle outcome for each Codex rollout. It derives that value by scanning valid JSON records in source-file order. Timestamps do not reorder records: they can be absent, duplicated, or inconsistent, while append order captures the observed event sequence.

The reducer starts at `unknown` and applies these transitions:

| Record | New status |
| --- | --- |
| `event_msg/task_complete` | `complete` |
| `event_msg/turn_aborted` | `aborted` |
| `event_msg/task_started` | `unknown` |
| `turn_context` | `unknown` |
| `event_msg/user_message` | `unknown` |
| user-role `response_item/message` | `unknown` |
| Any other known or unknown record | Preserve the current status |

Consequently, the last terminal event for the latest observed turn wins. Completion followed by interruption is `aborted`; interruption followed by completion is `complete`. Turn or user activity after either terminal outcome means the session resumed, so the earlier outcome no longer describes the session as a whole. It becomes `unknown` until a later terminal event is observed.

`unknown` means that the indexed records do not contain a terminal outcome for the latest opened work. It does not mean the agent is currently running, because a copied file has no live-process or freshness guarantee. Trailing accounting or tool records do not reopen a session. Transcript-only `<turn_aborted>` text remains a presentation notice; lifecycle status uses structured events rather than matching arbitrary message content.

For an initially imported partial file, status reflects the valid records that survived parsing and must be considered alongside `source_files.parse_status`. A failed partial replacement retains the previous projection and status under the normal transactional refresh rules.

## Upgrade and verification

This normalization change increments the parser version to 2. Run either command with the updated executable:

```bash
cargo run -- refresh --data data
# or, with the web server stopped:
cargo run -- rebuild --data data
```

The first refresh reimports rollouts indexed by an older parser version automatically. There is no schema migration. Refresh can update a database while the server is running; later requests see the committed statuses. See [refresh behavior](refresh.md) and [safe rebuilds](rebuild.md).

Synthetic regression cases cover completion then interruption, interruption then completion, resumption through each supported opening record, unrelated trailing records, missing lifecycle events, and deliberately reversed timestamps. The regression fails under the former presence-based implementation because any historical completion incorrectly overrides a later interruption or resumed turn.

Run the complete project checks from the repository root:

```bash
bash scripts/check.sh
```

## Performance review

Release builds of the former presence checks and the ordered reducer were compared on the same local copied-source corpus: 146 rollout files containing 209,290,891 bytes (199.6 MiB). Three sequential full rebuilds per implementation used warm filesystem caches, separate temporary SQLite paths, and no concurrent build. A `/proc` sampler polled peak resident memory every 10 ms. Only aggregate measurements and outcome counts were inspected.

| Implementation | Median elapsed | Observed elapsed range | Median sampled peak RSS |
| --- | ---: | ---: | ---: |
| Presence-based status | 5.812 s | 5.702–6.081 s | 104,108 KiB |
| Ordered status | 5.680 s | 5.585–5.705 s | 104,244 KiB |

The ordered build was about 2.3% faster at the median, but the small difference should be treated as run-to-run noise. Median sampled peak RSS increased by 136 KiB (about 0.13%); polling may miss brief peaks. Aggregate outcomes changed from 139 complete and 7 aborted sessions to 138 complete and 8 aborted sessions, confirming that ordering affected one indexed lifecycle result without exposing which session or any content.

Transaction boundaries, SQL query volume, file reads, decoded-record retention, and database writes are unchanged. The new reducer performs one pass over the already buffered per-file records and allocates no collection; the former logic performed up to two passes. Full-file buffering remains the separate R1.1 work item.
