# Application authentication

For a single developer reviewing their own local index, `cargo run -- serve --local` serves every view without a username or password. This mode accepts only numeric loopback bind addresses and grants access to anyone who can connect to that local port. It creates the normal owner-only state database but does not require an account. Omit `--local` to restore the authenticated mode; no index rebuild or account migration is needed.

Agent Lens uses locally provisioned application accounts and opaque server-side sessions. Application identities are independent of the operating-system users found in copied agent homes. The persistent state database defaults to `agent-lens-state.sqlite`; the normalized agent index remains disposable in `agent-lens.sqlite`.

Authentication identifies the application account; [source-scoped authorization](access-permissions.md) separately determines which indexed records it can read. Administrators see all sources and operational endpoints. Viewers see only explicitly granted stable source IDs.

## Provisioning and recovery

Administrative password input is accepted only through `--password-file`, never a command-line value. On Unix, regular password files are rejected unless group and other permission bits are clear. One final LF or CRLF is removed; other control characters are rejected. New passwords require at least 7 characters with at least one uppercase letter, lowercase letter, number, and non-whitespace symbol; the maximum is 1,024 bytes and no password is silently truncated. Use a unique password. For example:

```bash
(umask 077; touch /tmp/agent-lens-password)
${EDITOR:-vi} /tmp/agent-lens-password
cargo run -- account create --username alice --password-file /tmp/agent-lens-password
```

Usernames contain 1 to 64 ASCII letters, digits, dots, underscores, or hyphens and are case-insensitively unique. There is no public registration, email requirement, or security-question recovery. The first account must be `admin`; later accounts default to `viewer`, with `account create --role` allowing an explicit role. Database guards protect the last enabled administrator. Local administrators use:

```bash
cargo run -- account reset-password --username alice --password-file /tmp/agent-lens-password
cargo run -- account revoke-sessions --username alice
cargo run -- account disable --username alice
cargo run -- account enable --username alice
```

Any signed-in account can change its own password from the Account page after supplying its current password and matching confirmation. The same password policy is enforced in the browser and administrative commands. A successful self-service change retains a newly rotated current session and immediately invalidates the account's other sessions. An administrative password reset and disabling an account invalidate all existing sessions. Explicit revocation does the same without changing the password or enabled state. Enabling an account does not restore invalidated sessions. Logout deletes the current server-side session. Existing passwords remain usable until they are changed or reset.

Passwords are stored as salted Argon2id PHC strings using version 19, 19 MiB memory, two iterations, and one lane, following [OWASP's current minimum Argon2id profile](https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html). The login path also performs a dummy Argon2id verification for unknown usernames and returns the same generic failure message for an unknown account, wrong password, or disabled account. Passwords, hashes, cookies, CSRF tokens, and authorization headers are excluded from application logs and browser errors.

On the development host, the optimized synthetic account regression completed eight Argon2id hash/verification operations plus temporary SQLite state work in 0.53 seconds, an upper-bound average of about 66 milliseconds per operation. This confirms the selected minimum profile remains comfortably below OWASP's one-second verification guidance on this host; deployments with materially different hardware should measure before increasing the cost.

## Browser and session behavior

Unauthenticated HTML navigation redirects to `/login` with a percent-encoded same-origin return path. Non-browser requests receive `401`; absolute URLs, scheme-relative targets, and newline-bearing return targets are rejected. Successful login rotates the session identifier before associating the application identity.

Sessions use the maintained [`tower-sessions` server-side abstraction](https://docs.rs/tower-sessions/latest/tower_sessions/) with its SQLx SQLite store. They live in the persistent state database and use an `HttpOnly`, `SameSite=Lax` cookie named `agent_lens_session`. They expire after 30 minutes without a valid request and after 12 hours regardless of activity. Account enabled state, revocation generation, and app-owned [session inventory](session-security.md) state are rechecked from persistent state on every request, so restart, password change or reset, disablement, and administrator or self-service revocation fail closed immediately. Login, password-change, profile, preference, session-revocation, and logout forms use session-bound random CSRF tokens and accept POST only.

The `Secure` cookie attribute is off for the default loopback HTTP development server. For any HTTPS reverse-proxy deployment, prevent direct backend access and enable it explicitly:

```bash
cargo run -- serve --secure-cookies
# or AGENT_LENS_SECURE_COOKIES=true cargo run -- serve
```

All responses, including login and authentication failures, retain `Cache-Control: no-store`, the restrictive content-security policy, and `Referrer-Policy: no-referrer`.

## Prometheus authentication

`/metrics` accepts either a current browser session or HTTP Basic credentials for one enabled application account. Basic credentials are Argon2id-verified on every scrape, stripped before the metrics handler, and affected immediately by password reset or account disablement. This preserves ordinary Prometheus `basic_auth` configuration without restoring shared plaintext credential storage. Use HTTPS and a conservative scrape interval; password verification is intentionally expensive.

Only `admin` and `operator` accounts can read `/metrics`; viewer/auditor session or Basic credentials receive `403`. Authorized accounts can alternatively use a [personal access token](personal-access-tokens.md) with `metrics:read`, allowing automated scrapes without sharing an account password. Other bearer scopes cannot scrape metrics.

## Persistent state, rebuilds, and backup

The global `--state-database` option or `AGENT_LENS_STATE_DB` selects the state file for both `account` and `serve` commands:

```bash
cargo run -- --state-database /srv/agent-lens/state.sqlite account create --username alice --password-file /run/secrets/new-password
cargo run -- --state-database /srv/agent-lens/state.sqlite serve --secure-cookies
```

The state schema is created automatically, uses SQLite WAL for concurrent account/grant reads and short session writes, and restricts the main file to mode `0600` on Unix. Schema version 3 added [typed preferences](user-preferences.md); v4 added session inventory and security events; v5 added [personal access tokens](personal-access-tokens.md); v6 adds [teams and intermediate roles](teams-and-roles.md). Existing v1–v5 databases migrate automatically, preserving retained state, and valid legacy sessions are adopted lazily. The file contains password hashes and live session material and must be backed up as sensitive data. Stop the server and all account commands, make a filesystem-level copy of the state database, then restart. Restore only while stopped, preserve owner-only permissions and ownership, and use the same `--state-database` path. Do not copy only a live main SQLite file while write-ahead or journal files may exist.

Deleting or rebuilding `agent-lens.sqlite` does not touch application state. Deleting the state database removes all accounts and sessions and makes `serve` fail closed until a new account is provisioned. No source refresh or index rebuild is required when adding, resetting, disabling, enabling, or revoking an account. Existing deployments must provision an account, stop the old server, update their launch command to use the intended state path and cookie setting, and then start the new binary; the former `auth.local.json` file and `--auth-file` option are no longer used.

## Verification

Synthetic regressions cover the shared complexity policy, Argon2id storage without plaintext, case-insensitive account identity, administrative reset, self-service password change, individual/other/account-wide session revocation, disable/enable, fail-closed startup, session identifier rotation, inventory expiry, invalid and successful login, generic failures, CSRF rejection, explicit logout, inactivity and absolute expiry, restart persistence, retention, account isolation, sanitized output, protected routes, same-origin return targets, and Prometheus Basic authentication. Tests use only synthetic credentials and temporary databases.
