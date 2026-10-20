# Audited browser administration

Signed-in administrators can open `/admin/accounts` from Account. Viewers, auditors, operators, personal access tokens, metrics Basic credentials, and identity-free local mode cannot use this surface. The shared capability policy grants `AccountsAdminister` and `AuditRead` only to administrators. Source refresh, annotations, automatic lockout, public registration, and outbound reset links remain unavailable.

## Workflow and confirmation

[TOTP MFA](totp-mfa.md) is optional by default. Account details show enrollment and requirement state and offer require-enrollment, release-requirement, and MFA reset controls. These are audited account mutations, require the administrator's own current password and fresh factor when enrolled, and revoke the target's existing credentials. Removing the requirement does not disable an enrolled authenticator; reset preserves the requirement.

[Passkey second factors](passkey-second-factors.md) count as enrolled MFA too. Reset removes all TOTP/passkey credentials and recovery codes, preserving the requirement. An enrolled administrator may confirm a sensitive action using a fresh session-bound passkey proof instead of TOTP or a recovery code.

- `/admin/accounts` lists accounts and provisions a new account with an explicit role and policy-compliant initial password.
- `/admin/accounts/ACCOUNT_ID` shows the target's enabled state, role, direct/team grant origins, and token metadata. Select enable/disable, role change, password reset, all-session revocation, direct source grant/revocation, or individual token revocation. The value field supplies only the selected operation's role, stable source ID, or public token ID.
- `/admin/teams` creates and lists teams. `/admin/teams/TEAM_ID` supports rename/delete, membership changes, and team source grants, and shows bounded member/source lists. Teams still grant sources, not capabilities; independent grants survive removal.
- `/admin/audit` combines retained security events with actor-attributed browser mutation facts. Stable account/team IDs link historical facts even after a team is deleted. Ordinary CLI and self-service events retain their existing fixed codes and need not have an actor or before/after values.

Every mutation is a POST with a session-bound CSRF token. Supply your own current password on **each** submission: there is no reusable administrative unlock window. Type the exact target username or current team name to confirm the identity being changed; creation requires typing the new name again. The application never redisplays supplied passwords. For an account operation, a hidden role/enabled/generation snapshot rejects a stale form after those fields change. Reload the target after changes. Membership and source operations are serialized and idempotent, with before/after facts reflecting the state actually changed.

Password reset replaces the password immediately and revokes all browser sessions and tokens, as the local CLI does. This is an administrator-initiated credential reset, not an emailed link, forced-change enrollment, or notification system. Deliver initial/replacement passwords through a separate trusted channel; never put them in a URL or audit comment. Revoking all sessions also invalidates tokens through the account generation. Disabling revokes credentials; re-enabling does not restore them. Individual token revocation never reveals its bearer value or digest.

Source IDs can be copied from Source health. Existing accounts can also be managed locally:

```bash
cargo run -- account permissions --username analyst
cargo run -- account enable --username recovery-admin
cargo run -- account set-role --username recovery-admin --role admin
cargo run -- team list --page 1
```

Provision `recovery-admin` first if it does not exist, using the owner-only password-file workflow in [application authentication](application-authentication.md). CLI operations require filesystem access to the state database and do not depend on a browser session or the browser throttle.

## Atomic security boundary

Enrolled administrators must confirm a fresh authenticator or recovery code for every mutation. Required-but-unenrolled accounts cannot administer anything until setup completes. Account stale-state snapshots include MFA enrollment and requirement state as well as role/enabled/generation fields.

After password confirmation, the domain layer takes SQLite's writer lock and rechecks the actor's enabled state, credential generation, current capabilities, and the originating session's inventory identity, expiry, activity, and revocation state. Stale administrators and revoked sessions cannot commit a mutation even if middleware initially accepted them. The same transaction resolves and confirms the target, applies the mutation, appends the affected-account/team security event, and records administrative before/after facts. An audit failure rolls back the mutation. Existing database guards prevent disabling or demoting the last enabled administrator, including concurrent requests; CLI recovery remains available. Self-disable, self-demotion, password reset, and own-session revocation take effect immediately and can terminate administrative access.

State schema v7 adds `admin_events` with an allowlisted operation vocabulary and an update-rejecting trigger. Rows contain the actor ID, optional affected account/team IDs, an optional stable source/public-token subject, and bounded before/after facts: roles, enabled/granted/member state, generation numbers, or team names. No password, password hash, bearer value, digest, cookie, session-store identifier, transcript, arbitrary header, or free-form error is recorded. Confirmed idempotent operations may record identical before/after values. Application logs use existing fixed failure categories, never form values or underlying SQL errors.

History retains 180 days, pruned on startup and mutations. All pages use the account's 25/50/100-row preference plus one continuation row with deterministic ordering. Account detail pages page grants and tokens together; team detail pages page members and sources together. Each table is bounded independently, and a next page appears while either has more rows. Input forms use no client script; names and audit facts are HTML-escaped. Existing no-store/security headers also cover these pages.

## Failed-login defense without account lockout

All browser login attempts, administrator password confirmations, token-creation password confirmations, self-service password changes, and metrics Basic authentication share a **60-attempt/60-second budget per server process**. The budget is consumed before expensive password verification, including successful attempts, and is bounded in memory. Exhaustion returns `429` and `Retry-After: 60` for known, unknown, and disabled identities alike. Normal login failure remains generic and verifies a dummy hash for unknown identities. Failed administrative confirmation records the existing fixed failed-login event for the known actor.

This is a process-wide backstop, not distributed brute-force protection. Other clients can exhaust it until the window resets; a restart resets the budget. It neither collects peer/proxy addresses nor creates account-specific timers, username-keyed state, or automatic account lockouts. Existing signed-in browsing and local CLI recovery remain usable during throttling. Internet-facing deployments should additionally enforce a trusted reverse proxy's rate limits and HTTPS; do not infer client identity from untrusted forwarding headers. Automatic lockout remains disabled to avoid an attacker-controlled persistent denial of service against a named administrator.

## Migration and verification

Back up stopped persistent state and its MFA keyring before upgrading. The current binary migrates v1–v10 through [v11 durable OIDC session/logout state](oidc.md#durable-provider-session-revocation-gated), preserving state and audit facts. OIDC links are managed only through the local CLI, not browser administration. No source refresh or index rebuild is required. Restart after deploying; on this host, always restart `agent-lens.service` after replacing the debug binary and verify it is active.

Run `bash scripts/check.sh`. Synthetic tests cover browser account/team operations, CSRF, per-operation password confirmation, exact-target and stale-state checks, non-admin/token/local-mode denial, stale actor/session enforcement inside the writer transaction, concurrent updates, last-administrator safeguards, rollback on audit failure, append-only facts, secret-free/escaped HTML, pagination, credential invalidation, and shared throttling without account lockout. No private fixtures are read.
