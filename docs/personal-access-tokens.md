# Personal access tokens

For the complete automation contract, response fields, errors, and examples, see the
dedicated [API reference](api.md). This document covers token provisioning and lifecycle.

Signed-in users manage personal access tokens at `/account/tokens`. Creating a token requires the current password and a session-bound CSRF token. Password confirmation shares the [process-wide password-attempt budget](browser-administration.md). Revocation is an account-scoped, CSRF-protected POST. Administrators can inspect metadata and revoke another account's tokens through the audited browser surface, never reveal their secrets. Local mode has no token-owning identity and redirects management to Account.

## Automation endpoints and scopes

With [MFA](totp-mfa.md), token creation also requires a current authenticator or recovery code. Existing tokens are revoked when enrollment, enforcement, recovery-code replacement, or reset changes credential generation. Required-but-unenrolled accounts cannot create or authenticate tokens. Newly created tokens remain noninteractive read credentials: they do not require a factor per request and replace password-only metrics Basic authentication for MFA accounts.

The initial automation use cases are Prometheus scraping, collecting redacted per-session reports, and polling source observation status. Each token has exactly one read scope:

| Scope | Accepted GET/HEAD endpoint | Response | Account requirement |
| --- | --- | --- | --- |
| `metrics:read` | `/metrics` | Prometheus text | Current admin or operator role |
| `reports:read` | `/api/sessions/{stable_id}/postmortem` | Redacted postmortem JSON | Current access to the session's source |
| `content:read` | `/api/sessions/{stable_id}/live-status` | Live-status JSON | Current access to the session's source |

The live-status response describes indexed revision, import/observation timestamps, and source/projection state. It is evidence from copied sources, not a claim about a running agent. Postmortems retain the existing aggregate-only redaction contract in [postmortem exports](redacted-postmortems.md).

Use `Authorization: Bearer al_pat_…`. Query parameters and cookies do not authenticate `/api/` in authenticated deployments. An explicitly supplied bearer credential takes precedence over a browser cookie; invalid or insufficient tokens cannot fall back to a more privileged browser session. Duplicate authorization headers are rejected. Tokens cannot open HTML pages, manage accounts or credentials, or invoke mutations. The existing browser and individual-account Basic authentication options for `/metrics` remain available. Explicit loopback local mode retains its unauthenticated data access.

API failures contain fixed JSON error codes: `authentication_required` (401), `access_denied` (403), `not_found` (404), `invalid_request` (other client errors), `rate_limited` (429), or `internal_error` (server errors). Missing and ungranted session IDs both return the same 404 JSON. Responses retain `Cache-Control: no-store` and the restrictive security headers.

Store a header file with owner-only permissions containing `Authorization: Bearer YOUR_TOKEN`, then make requests without placing the credential in shell history or process arguments:

```bash
curl --fail --header @/run/secrets/agent-lens-token-header \
  http://127.0.0.1:3001/api/sessions/ses_REPLACE_WITH_STABLE_ID/postmortem
curl --fail --header @/run/secrets/agent-lens-token-header \
  http://127.0.0.1:3001/api/sessions/ses_REPLACE_WITH_STABLE_ID/live-status
curl --fail --header @/run/secrets/agent-lens-metrics-token-header \
  http://127.0.0.1:3001/metrics
```

The first two requests require separate tokens with their respective scopes. Shared deployments must use the HTTPS reverse-proxy deployment described in [application authentication](application-authentication.md).

## Credential lifecycle and storage

Tokens contain an independently random 64-bit public identifier and a 256-bit secret, generated with the operating system's cryptographic random source. The complete `al_pat_…` value is returned only in the successful creation response. Subsequent lists show the identifier, description, scope, creation/expiry time, last authentication time, and active/expired/revoked state. Lost bearer values cannot be retrieved; revoke and replace them.

State schema v5 stores only the public identifier, a versioned 32-byte BLAKE3 digest with an application-specific derivation context, and typed metadata in `personal_access_tokens`. Digest checks use constant-time comparison; unsupported digest versions fail closed. Descriptions must contain 1–80 non-control characters, and expiry must be 1–90 days (default 30). Creation and revocation append fixed security events in the same transaction as the mutation. Audit history and application logs never contain bearer values, digests, passwords, or authorization headers.

Every request intersects the token scope with the current account's [capabilities](capabilities.md), then uses current source grants before resource lookup. Tokens never gain a capability absent from their account. Account disablement, password change/reset, and account-wide session revocation invalidate existing tokens through the account generation. Re-enabling an account does not restore them. Individual browser logout or self-service “revoke other sessions” does not invalidate tokens. Role demotion removes metrics and unrestricted source access immediately; current source grant removal takes effect on the next request. Token revocation is explicit and immediately prevents subsequent authentication, including concurrent requests whose final authentication check occurs after revocation commits. Requests already authorized can complete.

Creation rechecks account enabled state, role, and the generation confirmed during password verification. Authentication's final atomic update rechecks enabled state, role, generation, expiry, revocation, and request budget before updating last-used metadata. Last used records successful authentication, including authenticated requests whose requested session is missing or ungranted.

## Bounds, limits, and retention

Each account can create at most 10 tokens per rolling hour and retain at most 100 unrevoked, unexpired tokens. These limits are enforced in the creation transaction. The token page uses the account's 25/50/100-row preference plus one continuation row; synthetic rendered regressions enforce a 256 KiB response bound. Revoked metadata is pruned 30 days after revocation, and expired metadata 30 days after expiry, at startup and token creation. Security events retain the established 180-day policy.

Authorized tokens are limited to 60 requests per UTC calendar minute, with the budget persisted in SQLite and shared across process restarts. Bearer attempts and authenticated-mode API requests additionally share a 600-attempt/60-second budget per server process, including malformed, missing, invalid, and unsupported credentials. This process-wide budget uses bounded memory and collects no network addresses. Exhaustion returns 429 with `Retry-After: 60`; it affects other API clients sharing the process until the window resets. Denied scopes do not update last-used metadata or consume the token's authorized-request budget.

## Deployment and verification

Back up stopped state and its MFA keyring before upgrading. Schema v5 introduced tokens; current builds migrate v1–v9 through [schema v10](oidc.md#local-administrator-identity-links), preserving tokens and retained history. Passkey-protected token creation requires a fresh factor proof, TOTP, or recovery code; passkey changes invalidate old tokens like other MFA changes. Explicit OIDC link/unlink operations also invalidate existing tokens through credential generation; merely migrating does not. Restart; no source refresh or index rebuild is required. On this host, restart `agent-lens.service` whenever the debug executable is replaced.

Run `bash scripts/check.sh`. Synthetic tests cover storage without bearer values, one-time reveal, unknown digest versions, scope/role denial, source isolation, cookie precedence, duplicate headers, CSRF, password confirmation, escaped descriptions, account isolation, expiry, revocation, disable/re-enable, stale confirmation, rate and creation limits, retention, bounded pagination, audit events, and v4 migration. Tests use temporary databases and synthetic credentials.
