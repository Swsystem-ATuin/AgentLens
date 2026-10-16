# Agent Lens API reference

This is the reference for the supported, read-only automation API. It covers all
currently supported bearer-token endpoints. Browser pages and their internal JSON
or form handlers are not additional automation APIs.

## Base URL and endpoint index

The default development base URL is `http://127.0.0.1:3001`. Shared deployments must
use their configured HTTPS reverse proxy; do not transmit tokens over untrusted HTTP.
Paths below are relative to that base URL and have no `/v1` prefix.

| Method | Path | Required token scope | Eligible account roles | Successful response |
| --- | --- | --- | --- | --- |
| GET, HEAD | `/api/sessions/{stable_id}/postmortem` | `reports:read` | admin, viewer, auditor, operator | Redacted JSON report |
| GET, HEAD | `/api/sessions/{stable_id}/live-status` | `content:read` | admin, viewer, operator | Indexed source status JSON |
| GET, HEAD | `/metrics` | `metrics:read` | admin, operator | Prometheus text |

These endpoints have no defined query parameters or request body and no pagination.
Use HEAD for response headers without a successful response body. Unsupported methods
and paths are denied by the bearer allowlist with 403; do not rely on receiving 405
or use OPTIONS as API discovery. There is currently no session-list/search API,
transcript API, write API, account/token-management API, or OpenAPI/Swagger endpoint.

`stable_id` is the source-scoped public session ID found in browser session links,
not an integer SQLite ID or an upstream agent's native session ID. Treat it as opaque
and URL-encode it as a single path segment. IDs reconstruct from the same raw source
identity after a rebuild; they are not credentials. This API does not enumerate them.

## Authentication and authorization

Create a personal access token while signed in at `/account/tokens`. Creation requires
password confirmation, CSRF, and a fresh factor if MFA is enrolled. Select the endpoint's
scope and an expiry of 1–90 days (default 30). Each token has exactly one scope, so
separate scopes require separate tokens. The bearer value is shown only once.

Send exactly one header:

```http
Authorization: Bearer al_pat_REPLACE_WITH_YOUR_TOKEN
```

For `/api/` requests, browser cookies, HTTP Basic authentication, and query-string
credentials do not authenticate the caller. A supplied bearer credential also takes
precedence over a browser session on `/metrics`; a wrong token cannot fall back to
a more privileged cookie. Duplicate Authorization headers are rejected.

Each request checks the current account's enabled state, credential generation,
role/capabilities, token scope, expiry, and revocation. Session endpoints additionally
require current access to the session's source, from direct or team grants (admins
have all-source access). An auditor can read reports but not live status. Role or
grant removal takes effect on the next authorization check. Disabled accounts,
expired/revoked tokens, and credentials invalidated by password/reset, MFA changes,
or account-wide session revocation cannot authenticate; re-enabling an account does
not restore invalidated tokens. Requests already authorized can complete.

Required-but-unenrolled MFA accounts cannot use tokens. A valid token is otherwise a
noninteractive credential: no password, factor, or CSRF token is sent per API request.
Tokens do not authorize browser pages, account administration, mutations, or source
refresh. See [token management and lifecycle](personal-access-tokens.md) for provisioning,
storage, audit, creation limits, and recovery details.

`/metrics` additionally supports a current admin/operator browser session or individual
account HTTP Basic authentication. Basic is unavailable for accounts with enrolled or
required MFA; use a scoped token instead. API examples below use bearer authentication.

Explicit loopback-only `serve --local` mode bypasses account authentication and serves
these read endpoints without tokens. It is a development exception, not a deployment
authentication strategy. Do not expose it through a shared reverse proxy.

## GET /api/sessions/{stable_id}/postmortem

Returns a deterministic aggregate-only report computed from the current committed
index. It does not refresh sources or contact an agent. Success is 200 with
`Content-Type: application/json; charset=utf-8` and
`Content-Disposition: attachment; filename="agent-lens-postmortem.json"`.

The complete format-version-1 shape is shown below using synthetic values:

```json
{
  "format_version": 1,
  "redaction": {
    "profile": "aggregate-only",
    "excluded": [
      "prompts and messages",
      "source code",
      "tool arguments and outputs",
      "raw source records",
      "titles and native session IDs",
      "filesystem paths and projects",
      "host and operating-system user names",
      "credentials and account identifiers",
      "user annotations"
    ]
  },
  "session": {
    "stable_id": "ses_SYNTHETIC_EXAMPLE",
    "agent_kind": "codex",
    "indexed_status": "unknown",
    "source_projection_status": "ok"
  },
  "turns": {
    "observed": 0,
    "completed": 0,
    "interrupted": 0,
    "unclosed": 0,
    "terminal_only": 0,
    "resumed": 0
  },
  "usage": {
    "input_tokens": null,
    "cached_input_tokens": null,
    "output_tokens": null,
    "reasoning_output_tokens": null,
    "reported_total_tokens": null,
    "observations": 0,
    "counter_resets": 0,
    "missing_observations": 0,
    "inconsistent_observations": 0
  },
  "models": {
    "requested_model_observations": 0,
    "requested_model_changes": 0,
    "observed_served_model_fallbacks": 0
  },
  "tools": {
    "requests": 0,
    "results_observed": 0,
    "failures_observed": 0,
    "outcomes_unknown": 0
  },
  "pressure": {
    "context_observations": 0,
    "rate_limit_observations": 0
  },
  "evidence_gaps": {
    "context_observations_missing_values": 0,
    "rate_limit_observations_missing_values": 0,
    "operational_events_missing_valid_timestamps": 0
  },
  "reason_codes": []
}
```

Counts and known token totals are JSON integers backed by signed 64-bit values.
The five token-total fields are nullable: null means unavailable, not zero. Clients
whose number type cannot represent every 64-bit integer should use a suitable JSON
parser. Cached/reasoning token fields are subsets, not additional totals to add blindly.

`session.agent_kind` is `codex` or `other` (including non-Codex agents).
`indexed_status` is `complete`, `aborted`, or `unknown`;
`source_projection_status` is `ok`, `partial`, `error`, or `unknown`.
These are recorded evidence, not claims of present process activity or task quality.

`reason_codes` contains at most 14 objects, each with `code` (string), `count`
(positive integer), and `explanation` (fixed explanatory string). Only observed
positive counts are included; an empty array is valid. Codes are:

```text
turn_interrupted
turn_unclosed
turn_opening_missing
requested_model_changed
served_model_fallback_observed
usage_counter_reset
usage_values_missing
usage_values_inconsistent
tool_failure_observed
tool_outcome_unknown
context_evidence_missing
rate_evidence_missing
timestamp_evidence_missing
source_projection_not_complete
```

The report includes no export time, transcript, prompts, source code, tool bodies,
paths, titles, host/account identifiers, exact model names, or annotations. Its only
session-identifying value is the pseudonymous stable ID. Aggregates can still be
sensitive; review before sharing. See [redaction semantics](redacted-postmortems.md).
The browser JSON/Markdown download routes documented there are not bearer alternatives;
there is no Markdown representation on this automation endpoint.

## GET /api/sessions/{stable_id}/live-status

Returns 200 with `Content-Type: application/json`. All six fields are present:

```json
{
  "revision": 42,
  "connection": "connected",
  "last_successful_import": "2026-01-01T12:00:00Z",
  "source_observed_at": "2026-01-01T12:00:05Z",
  "source_state": "recent",
  "source_projection_status": "ok"
}
```

| Field | Type | Meaning |
| --- | --- | --- |
| `revision` | integer (signed 64-bit) | This source file's ingest-run ID, or 0 if unavailable. Compare for change; it is not a session event count or installation-wide revision. A rebuild may reset/reuse values. |
| `connection` | string | Always `connected` in a successful server response. Browser-side paused/disconnected states are not additional API values. |
| `last_successful_import` | string or null | Stored finish timestamp of the complete ingest run that last projected this source, when available. |
| `source_observed_at` | string or null | Stored timestamp of the last transactional observation, including unchanged checksums or localized parse errors. |
| `source_state` | string | `recent` for a parseable observation aged 0–120 seconds; `stale` for older or future-dated observations; `unavailable` if missing/unparseable. |
| `source_projection_status` | string | `ok`, `partial`, `error`, or `unknown`. |

Timestamps are stored strings, not reformatted account-local display values; do not
assume a fixed fractional-second precision. This endpoint does not return transcript
content, refresh the index, read raw files, or prove that an agent is running. A
successful connection can coexist with stale or incomplete source evidence. Poll
conservatively within the limits below; see [live-reader semantics](live-reading.md).

## GET /metrics

Returns 200 and `Content-Type: text/plain; version=0.0.4; charset=utf-8`, not JSON.
The response uses Prometheus HELP/TYPE descriptors and newline-terminated samples.
All metrics are gauges of the disposable index; values can decrease after deletion
or rebuild. Optional unavailable samples are omitted, not represented as zero.

Metric families are `agentlens_indexed_sessions`, `agentlens_indexed_source_files`,
`agentlens_indexed_tool_outcomes`, `agentlens_indexed_turn_outcomes`,
`agentlens_ingest_runs`, `agentlens_last_ingest_info`,
`agentlens_last_ingest_duration_seconds`, `agentlens_last_ingest_files`,
`agentlens_last_ingest_errors`, `agentlens_last_ingest_finished_timestamp_seconds`,
`agentlens_indexed_usage_tokens`, and `agentlens_indexed_usage_sessions`.
Labels use fixed vocabularies and exclude session IDs, paths, prompts, and raw model
names. The [metric catalog](operational-metrics.md#metric-contract) defines each
family, label, availability rule, and Prometheus configuration.

## Errors, headers, and rate limits

Errors under `/api/` have `Content-Type: application/json` and this fixed envelope:

```json
{"error":"authentication_required"}
```

| HTTP status | `error` | Meaning |
| --- | --- | --- |
| 401 | `authentication_required` | Missing, malformed, invalid, expired, revoked, or otherwise invalidated credentials. |
| 403 | `access_denied` | Wrong scope, insufficient current role/capability, or unsupported bearer path/method. |
| 404 | `not_found` | Unknown, hidden, or ungranted session; these cases are deliberately indistinguishable. |
| Other 4xx except 429 | `invalid_request` | Other client request error after middleware/routing validation. |
| 429 | `rate_limited` | A shared process or per-token request budget is exhausted. |
| 5xx | `internal_error` | Server failure; internal details are not returned. |

Do not infer status solely from authentication: allowlist checks can return 403 before
a bearer value is verified. `/metrics` bearer-authentication failures also use JSON
codes, but non-bearer authentication and handler errors there are not guaranteed to
use the `/api/` JSON envelope. Check HTTP status and Content-Type before decoding.

All responses carry `Cache-Control: no-store`, a restrictive Content-Security-Policy,
and `Referrer-Policy: no-referrer`. Missing/invalid bearer authentication includes
`WWW-Authenticate: Bearer realm="Agent Lens API"`. Rate-limited responses include
`Retry-After: 60`; wait at least that long and avoid tight automatic retry loops.

Each token is limited to 60 authorized requests per UTC calendar minute, persisted
across process restarts. Successful authentication counts even if resource lookup
then returns 404; denied scopes do not consume that token budget. All bearer attempts
and authenticated-mode `/api/` requests additionally share a 600-attempt/60-second
budget per server process, including missing/invalid credentials. One client's traffic
can therefore temporarily affect others. HEAD requests consume the same budgets.
These token budgets do not apply in explicit local mode. No ETag/conditional-request
or streaming protocol is defined; requests observe the committed index when queried.

## Request examples

Provision separate tokens for the three scopes. Store each complete header line
(`Authorization: Bearer YOUR_TOKEN`) in its own owner-readable-only file. Do not put
bearer values in shell history, URLs, source control, or process arguments. Replace
the base URL, stable ID, and file paths with those for your deployment:

```bash
api_base='http://127.0.0.1:3001'
session_id='ses_REPLACE_WITH_STABLE_ID'

curl --fail-with-body --silent --show-error \
  --header @/run/secrets/agent-lens-reports-token-header \
  "$api_base/api/sessions/$session_id/postmortem"

curl --fail-with-body --silent --show-error \
  --header @/run/secrets/agent-lens-content-token-header \
  "$api_base/api/sessions/$session_id/live-status"

curl --fail-with-body --silent --show-error \
  --header @/run/secrets/agent-lens-metrics-token-header \
  "$api_base/metrics"
```

Use `--head` with the appropriate request to inspect headers. Avoid `--verbose` or
request tracing with real credentials, and do not automatically forward Authorization
to redirects. A 401 normally requires replacing/recovering credentials, a 403 requires
checking scope and current permissions, and a 404 does not reveal whether a session exists.

## Compatibility and verification

This reference describes the current implementation, not an additional API service.
No configuration change, migration, source refresh, or index rebuild is needed for
this documentation. There is no global API-version negotiation; the postmortem's
`format_version` versions that report only. Clients should tolerate unknown JSON fields
and handle unavailable values explicitly rather than treating them as zero.

The normal regression suite checks bearer scope/cookie precedence, source isolation,
errors, revocation, rate limits, deterministic redaction, live-status projections, and
metric privacy using synthetic temporary data. Run `bash scripts/check.sh` to verify.
