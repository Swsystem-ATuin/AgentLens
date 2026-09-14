# Operational metrics

Agent Lens exposes an authenticated Prometheus text-format endpoint at `/metrics`. It follows the official [Prometheus exposition format 0.0.4](https://prometheus.io/docs/instrumenting/exposition_formats/): UTF-8, newline-terminated samples, `HELP`/`TYPE` descriptors, and the response content type `text/plain; version=0.0.4; charset=utf-8`.

The endpoint accepts a current browser session or HTTP Basic credentials for an enabled individual application account. Other browser routes use server-side session authentication. Metrics authentication does not refresh sources, contact agents, or read raw homes. For a manual check, avoid putting the password in shell history:

```bash
curl --user alice http://127.0.0.1:3001/metrics
```

Prometheus supports Basic authentication when scraping targets. A local example using separately protected secret files is:

```yaml
scrape_configs:
  - job_name: agent_lens
    metrics_path: /metrics
    static_configs:
      - targets: ["127.0.0.1:3001"]
    basic_auth:
      username_file: /run/secrets/agent_lens_username
      password_file: /run/secrets/agent_lens_password
```

The username and password files contain one provisioned Agent Lens account's credentials. Password reset or account disablement takes effect on the next scrape. Use HTTPS through the documented reverse-proxy boundary if the scraper is not on the same trusted host. The core UI and all CLI workflows remain independent of Prometheus. No Grafana dashboard is included because there is no deployment requirement for one.

## Metric contract

Every exported value is a gauge because it describes the current disposable index or history retained inside that index. Values can decrease after source deletion or rebuild and are not process-lifetime counters.

| Metric | Meaning |
| --- | --- |
| `agentlens_indexed_sessions{agent,status}` | Visible session count |
| `agentlens_indexed_source_files{agent,state,freshness}` | Source projection and observation-health count |
| `agentlens_indexed_tool_outcomes{agent,outcome}` | Observed result, failed result, other result, or call-without-result count |
| `agentlens_indexed_turn_outcomes{agent,outcome}` | Complete, interrupted, unclosed, or other turn count |
| `agentlens_ingest_runs{status,reason}` | Ingest run records retained in this index |
| `agentlens_last_ingest_info{status,reason}` | Latest ingest state, when one exists |
| `agentlens_last_ingest_duration_seconds` | Latest finished ingest duration, when available |
| `agentlens_last_ingest_files` / `agentlens_last_ingest_errors` | Latest ingest source/error counts |
| `agentlens_last_ingest_finished_timestamp_seconds` | Latest finish time, when available |
| `agentlens_indexed_usage_tokens{agent,model_family,kind}` | Known cumulative indexed token totals |
| `agentlens_indexed_usage_sessions{agent,model_family,kind,availability}` | Counts that distinguish known from unavailable token totals |

Token kinds are `input`, `output`, `cached_input`, `reasoning_output`, and `total`. Missing values produce no token sample and increment the corresponding `availability="unavailable"` gauge; they are never converted to zero. Per-turn-only Gemini observations therefore remain unavailable as cumulative totals.

## Privacy and cardinality bounds

Metric labels use fixed buckets only:

- `agent`: `codex`, `gemini`, or `other`;
- session status: `complete`, `aborted`, `unknown`, or `other`;
- source state: `ok`, `partial`, `error`, or `other`;
- freshness: `recent`, `stale`, `clock_skew`, or `unavailable` using the same thresholds as source health;
- tool outcome: `observed`, `failed`, `unknown`, or `other`;
- turn outcome: `complete`, `interrupted`, `unclosed`, or `other`;
- model family: `gpt`, `o-series`, `codex`, `gemini`, `unknown`, or `other`;
- ingest status and reasons use the fixed source-health vocabulary.

Exact model strings are deliberately reduced to families before grouping. The response never includes prompts, message or title text, native or public session IDs, hostnames, operating-system users, source IDs, paths, parser errors, model strings outside fixed families, tool names, inputs, or results. Codex hidden-model policy is applied to session, usage, tool, and turn aggregates. Source-health counts still include every discovered file because failed sources may not have a session or model to classify.

These vocabularies bound the response independently of database cardinality. An adversarial synthetic regression enforces a response smaller than 128 KiB and injects private sentinel values into every excluded field to verify that none is rendered. The large benchmark below exercises broad combinations of every label dimension.

## Restart, refresh, and rebuild semantics

Restarting the server does not reset metrics: a new process reads the same index and reconstructs the same samples. Refresh changes gauges only after its transaction commits. Source removal can reduce counts.

A full rebuild creates a new disposable index from raw sources. Source-derived session, source, tool, turn, and supported usage gauges reconstruct deterministically; the ingest-run history belongs to the index and starts again with the rebuild run. Latest-ingest timestamps and measured duration naturally differ. Synthetic tests cover restart equality and repeated full-rebuild parity.

No schema migration, source refresh, or rebuild is required to enable the endpoint. Restart the server after updating the binary so the new route is registered.

## Query and load verification

One scrape runs seven read-only aggregate queries in a single SQLite read transaction. Every metric family therefore observes one committed projection even if a refresh commits during the scrape. Queries group directly into fixed buckets and never retrieve transcript content. They do not alter ingestion transactions, source-file passes, normalized writes, allocations, or rebuild performance.

A release-build stress measurement used a 63,750,144-byte synthetic SQLite index containing 100,000 sessions, source files, usage summaries, tool calls, and turns, 80,000 tool results, and 1,000 ingest runs across all fixed label buckets. Ten sequential authenticated loopback scrapes took 4.702 seconds median with a bimodal 2.533–6.541-second range. Every response was byte-identical at 17,257 bytes and 190 lines. This intentionally large aggregate-scan workload establishes response/cardinality and coherent-snapshot behavior, not a recommended scrape interval or a general latency guarantee. Use a conservative interval for similarly large indexes and monitor scrape duration from Prometheus itself.
