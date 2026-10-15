# Multi-user framework and authentication decision

Status: accepted and implemented for M1-M3 on 2026-09-14.

## Decision

Agent Lens will continue as a Rust, Axum, Askama, SQLx, and SQLite application for the first complete multi-user access model. Multiple users do not by themselves justify rewriting the tested importer, query layer, privacy boundaries, and server-rendered interface in Django.

M1 replaces the interim shared HTTP Basic credential with established server-side session authentication and locally provisioned application accounts. The implementation uses maintained Rust components including [`tower-sessions`](https://docs.rs/tower-sessions/latest/tower_sessions/) and RustCrypto password hashing rather than inventing a cookie or password protocol. It provides a login form, explicit logout, absolute and inactivity expiry, revocation of one or all sessions, and disabled-account enforcement on every request.

Accounts will use an opaque stable application identity and a unique login name. They are distinct from indexed operating-system users. Passwords will be stored only as salted Argon2id PHC strings with parameters measured on the deployment hardware; plaintext, reversible encryption, and the current fast SHA-256 comparison are not acceptable password storage. [OWASP password-storage guidance](https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html) recommends a slow password hash and currently prefers Argon2id; its work-factor guidance also requires tuning against the actual server rather than selecting an unmeasured cost. The [RustCrypto `argon2` crate](https://docs.rs/argon2/latest/argon2/) provides Argon2id and PHC password-hash support.

There will be no public registration, email dependency, security questions, or browser-based forgotten-password flow in the initial self-hosted design. Provisioning, password reset/recovery, account disablement, and session revocation will be explicit local administrator CLI operations. Password input must be interactive or accepted through a protected file descriptor, never a command-line argument. A password reset and account disablement revoke existing sessions.

Accounts, password hashes, server-side sessions, roles, and stable source grants now live in a persistent application-state SQLite database with its own schema, without sharing lifecycle or row identifiers with the disposable agent index. M2 selects and enforces source-scoped viewers plus all-access administrators. M3 persists those mappings by stable source identity and verifies migration, rebuild survival, backup, and recovery independently of the disposable index.

OpenID Connect is being developed as an opt-in integration with mandatory configuration before activation, rather than requiring a real provider before development starts. The [offline configuration preflight](oidc.md) is available; federated sign-in is not yet implemented. Initial policy requires administrator-provisioned issuer+subject links, locally managed roles/teams, local break-glass access, and both RP-initiated and back-channel logout. Provider compatibility must be verified before deployment. [RP-initiated logout](https://openid.net/specs/openid-connect-rpinitiated-1_0.html) and [provider-to-application back-channel logout](https://openid.net/specs/openid-connect-backchannel-1_0.html) are separate standardized capabilities. Runtime work must include discovery and issuer pinning, Authorization Code with PKCE and state/nonce validation, stable subject mapping using issuer plus subject, local session lifetime, and provider-triggered revocation.

## Framework comparison

| Requirement | Continue Rust/Axum | Rewrite in Django | Decision impact |
| --- | --- | --- | --- |
| Login, logout, expiry, and revocation | Established Axum authentication and server-side session crates fit the existing middleware boundary; implementation and integration tests are still required | Django provides mature authentication, sessions, login/logout views, password handling, and administration | Django has a stronger built-in account-management surface, but Rust has adequate established components for this bounded workflow |
| Per-request permissions | Existing routes, SQL queries, stable source/session identities, and hidden-session policy can accept one typed authorization context | Django permissions are mature, but source-scoped object visibility would still require custom query enforcement | A rewrite does not remove the application-specific M2 policy work |
| Persistent state versus disposable index | A second SQLx pool and migration boundary preserve the current raw-source/rebuild contract | Separate Django models/database routing would also be needed to keep account state out of rebuilds | Equivalent architectural requirement; no Django advantage offsets migration risk |
| Existing behavior and verification | Retains the implemented adapters, ingest transactions, FTS, bounded readers, diagnostics, metrics, exports, and 122 automated tests | Requires porting and revalidating every parser, query, rendering, security, privacy, and performance contract | Rewriting creates substantial regression risk without user-visible benefit |
| Operations | Remains one compiled service plus two SQLite files; local CLI can own provisioning and recovery | Adds a Python/Django runtime and migration/admin operations, while the Rust importer must be retained or ported | The current deployment remains simpler in one Rust service |
| Team and feature shape | Current repository and contribution rules are Rust-first; the UI is read-heavy and server-rendered | Django becomes attractive for a team centered on Python or a large CRUD/admin product | Neither condition is currently confirmed |

[Django's documented authentication system](https://docs.djangoproject.com/en/5.2/topics/auth/default/) does cover users, passwords, sessions, login/logout, and permissions. That reduces commodity account-management code, but it does not define Agent Lens's source visibility policy or preserve its rebuild boundary automatically. The deciding cost is therefore the wholesale port and duplicate regression burden, not whether Django can authenticate users.

## Implementation boundaries

- Authentication identifies the application user. Authorization separately decides which indexed records that identity may read.
- Route protection remains default-deny. Login and narrowly required static assets are the only anonymous browser surface; health data, metrics, exports, lazy fragments, live probes, and direct transcript URLs remain protected.
- Session cookies contain only an opaque random identifier. Server-side state holds the account association and expiry. Cookies are `HttpOnly`, `SameSite=Lax` or stricter, and `Secure` whenever HTTPS is used. Login rotates the identifier; logout deletes server state and expires the cookie.
- State-changing browser requests require POST and CSRF protection. Return targets are restricted to same-origin local paths.
- Disabled or missing accounts and revoked, expired, malformed, or unknown sessions fail closed. Sensitive credential/session values are excluded from logs and errors.
- Authentication responses retain `Cache-Control: no-store`, the restrictive content-security policy, and the HTTPS reverse-proxy requirement for non-loopback access.
- Automated regressions must cover separate synthetic accounts, invalid login, identifier rotation, logout, both expiry modes, individual and account-wide revocation, password reset/disable invalidation, restart persistence, rebuild survival, CSRF, open-redirect rejection, and protection of every route.

## Reconsideration triggers

Re-open the framework decision only when evidence changes materially: a committed organization-wide Django/Python operating model; a large administrator-managed CRUD surface beyond accounts and permissions; a required identity integration that is well-supported in Django but cannot be supported safely in the Rust stack; or measured concurrency/operational requirements that invalidate the single-service SQLite design. Reconsidering must compare a staged migration and regression plan, not assume that authentication alone warrants a rewrite.

## Operational effect

M1 and M2 now implement the selected authentication and source-scoped authorization model. Existing pre-M2 accounts migrate to administrators to preserve visibility; later accounts default to viewers. No agent-index rebuild is required. See [application authentication](application-authentication.md) and [access permissions](access-permissions.md) for current commands and migration behavior.
