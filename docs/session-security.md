# Session inventory and security history

Authenticated accounts can review their browser sessions and security history at `/account/sessions`. This application-owned metadata lives in the owner-only `agent-lens-state.sqlite` database, separate from the disposable agent index and immutable source homes.

## Session inventory

State schema version 4 adds `account_sessions`. Each row links one account to a private `tower-sessions` store identifier and a separate random inventory identifier used by revocation forms. The cookie/session-store identifier is never rendered. The inventory records sign-in time, last valid activity, absolute expiry, session generation, revocation time, and a fixed client summary.

The page labels the confirmed browser as `current`; other usable records as `active`; records beyond the 30-minute inactivity or 12-hour absolute limit as `expired`; and explicitly closed records as `revoked`. A valid request updates its own last activity only after account, generation, lifetime, and revocation checks pass. Existing pre-v4 sessions receive an `Unknown client (upgraded session)` inventory row on their first valid request rather than being signed out during migration.

Individual revocation and “revoke all other sessions” are authenticated `POST` operations with session-bound CSRF validation and account-scoped SQL predicates. The current identifier cannot be targeted by individual revocation, and the bulk operation excludes it. Revocation marks the inventory row and deletes the corresponding server-side session payload in one transaction; concurrent requests also fail the inventory check once that transaction wins. Password changes rotate and preserve the confirmed current session while revoking the rest. Logout marks and deletes the current session.

The configured preference page size bounds inventory queries to 25, 50, or 100 rows plus one continuation row. Revoked or expired history is retained for 30 days after its last activity and pruned at startup and when security events are written.

## Client and network metadata

Raw `User-Agent` headers are never stored. Login reduces the value to an allowlisted client family (`Edge`, `Firefox`, `Chrome`, `Safari`, `curl`, `Wget`, `Other client`, or `Unknown client`) and, when recognized, an allowlisted operating-system family. The result is short and contains no version, extension, device, or arbitrary header text.

Network addresses are not collected or displayed. Agent Lens does not currently have a trusted-proxy configuration that can distinguish a direct peer from a verified forwarding hop, so it deliberately ignores `Forwarded` and `X-Forwarded-For`. Adding addresses requires an explicit proxy trust boundary, source labeling, and a separate retention review.

## Security events

The `security_events` table accepts only fixed event codes for account creation, successful and failed sign-ins, password changes and administrative resets, session revocation, account enable/disable, role changes, source grants, and profile changes. A database trigger rejects updates, making retained rows append-only; the only deletion path is retention pruning. Event rows contain an account reference, fixed code, timestamp, and no free-form detail.

Failures for a known account are visible only to that account. Unknown or syntactically invalid usernames produce the same browser response and timing-oriented dummy password verification, and their events remain unlinked; no external response reveals whether an account exists. The ledger never stores passwords or hashes, cookie/session/token values, request headers, network addresses, transcript data, paths, or error text.

Security history uses the account's 25/50/100 page-size preference and is retained for 180 days. Retention is enforced at startup and whenever an event is appended. The account page also displays a derived password age and permits an optional display name of at most 80 non-control characters. Display names do not replace usernames and are HTML-escaped on output. Notification email remains deferred until M5.7 defines its purpose and lifecycle.

## Upgrade and verification

Schema version 4 introduced session inventory and security events. The current binary migrates earlier schemas through that change and [schema v5 for personal access tokens](personal-access-tokens.md), preserving retained audit history. Back up the stopped state database first, then restart the service; no agent-index rebuild or source refresh is required.

Run `bash scripts/check.sh`. Synthetic tests cover v1-to-v4 migration, legacy-session adoption, identifier rotation, inactivity and absolute expiry, immediate payload deletion, current-session preservation, individual and bulk revocation, CSRF, account isolation, escaped profile output, client-header sanitization, bounded pagination, append-only/allowlisted events, known and unknown failed sign-ins, and 30/180-day retention. Fixtures contain no private account or request data.
