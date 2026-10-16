# OpenID Connect preparation and identity links

OIDC is under development, not an available sign-in mechanism. It will be opt-in;
local authentication remains unchanged. The offline preflight validates proposed
configuration without opening the state database, reading client secrets, making
network requests, or enabling authentication.

Save this synthetic example outside the raw `data/` tree as `oidc.json`:

```json
{
  "issuer": "https://identity.example.test/tenant",
  "public_url": "https://lens.example.test",
  "client_id": "registered-client-id",
  "client_authentication": {
    "method": "client_secret_basic",
    "secret_file": "/etc/agent-lens/oidc-client-secret"
  },
  "account_linking": "administrator_provisioned_issuer_subject",
  "permissions": "local_only",
  "logout": "rp_initiated_and_back_channel_required",
  "local_break_glass": true
}
```

```sh
cargo run -- oidc-check --config oidc.json
```

To explicitly contact the configured issuer and check discovery metadata:

```sh
cargo run -- oidc-check --config oidc.json --discover
```

Discovery first validates the local configuration, then makes one anonymous HTTPS
GET beneath the configured issuer path at `/.well-known/openid-configuration`.
It sends no client secret, client ID, cookies, or account information, and does not
contact advertised endpoints. Only use configuration supplied by a trusted local
administrator: the issuer determines the network destination, including private
identity-provider networks. Proxy environment variables are ignored. Requests use
the system TLS trust store, require TLS 1.2 or newer, do not follow redirects, have
a five-second connection timeout and ten-second total deadline, and accept at most
256 KiB of JSON. The container includes system CA certificates; on native hosts,
install a system trust store and add any private provider CA through its normal
administrative mechanism. There is no insecure TLS bypass option.

The issuer in the document must match the configured string exactly, including
case and trailing slash. The initial compatibility policy requires HTTPS endpoints
without credentials, query, or fragment; code response type, authorization-code
grant, query response mode, standard public/pairwise subjects, RS256 ID-token signing,
explicit S256 PKCE advertisement, and the configured client authentication method.
The standard defaults apply when optional grant/response-mode/auth-method metadata
is omitted; explicit malformed values fail. Advertised scopes must include `openid`.
RP-initiated logout must have an HTTPS `end_session_endpoint`, and back-channel logout
support must be advertised. Unknown extension fields are tolerated. Diagnostics omit
provider bodies, supplied values, and underlying network errors.

Success only means the discovery metadata meets this policy. It does not validate
signatures or keys, test token/logout endpoints, confirm registration settings, read
the client secret, or enable sign-in. The strict advertisement and endpoint policies
may reject otherwise conforming providers; compatibility must be checked deliberately.

For explicit protocol preparation, including signing keys and client credentials:

```sh
cargo run -- oidc-check --config oidc.json --prepare
```

`--prepare` and `--discover` are mutually exclusive. Preparation validates the local
configuration and reads any configured client secret **before** contacting the
provider. Secret files must be regular files, not symlinks, with owner-only Unix
permissions (for example `0600`). The secret is UTF-8, 1–4096 bytes without control
characters; one final LF or CRLF is removed, but other whitespace is preserved.
Unsupported non-Unix file-protection checks fail closed. Errors omit secret contents
and file paths. No credentials are sent during preparation.

Preparation fetches discovery and its advertised `jwks_uri` using the same bounded,
verified, anonymous HTTPS policy above, then constructs the internal protocol engine.
Only use trusted administrator-supplied configuration: advertised key URLs may point
at different hosts or private networks. Invalid or unsuitable signing keys fail closed.
The engine is discarded on exit: no database, session, token, key cache, or configuration
is written. No schema migration or index rebuild is needed.

Success is **not** deployment readiness or authentication activation. This command
does not verify running Secure cookies, a usable local administrator, client
registration, token/logout endpoint behavior, or browser/MFA/session integration.

To also check declared deployment prerequisites against existing persistent state:

```sh
cargo run -- --state-database /var/lib/agent-lens/agent-lens-state.sqlite oidc-check \
  --config oidc.json --prepare --deployment \
  --public-url https://lens.example.test --secure-cookies
```

`--deployment` requires `--prepare`, `--public-url`, and `--secure-cookies`; these
declarations are explicit flags, not inferred from environment variables or proxy
headers. The deployment public URL must be an HTTPS origin matching the configured
OIDC public origin (equivalent default ports and a final slash are accepted). Future
runtime integration must use the same gate and reject unauthenticated local mode.

Before loading secrets or making network requests, this mode opens the existing
state database read-only, requires schema v10, and requires an enabled local
administrator with a structurally valid Argon2id password credential. It does not
create or migrate state, revoke credentials, write audit events, or open the index.
Missing, unsupported, or malformed state fails closed. Migrate older state using
the normal account/service workflow with a backup first; this increment itself
requires no migration or rebuild. Preparation then uses the same loaded configuration
to fetch and validate discovery and keys, without re-reading a potentially changed file.

This is a **declared-prerequisites preflight**, not a live deployment certification:
it cannot prove password knowledge, MFA possession/keyring availability, actual
reverse-proxy TLS/cookie behavior, or client/logout registration. Test local password
and MFA recovery access manually before any future activation. No sign-in is enabled,
and runtime must recheck current account/deployment state rather than trust this result.

Every field is required; unknown fields and unsupported policy values are rejected.
Use `{"method":"none"}` only for a provider registration supporting public clients.
Secret values are not accepted inline. The secret path must be absolute; preflight
does not check file existence, permissions, or contents. Without `--discover`, it
does not check provider compatibility either.
The preflight commands do not open or migrate either database. Account commands and
server startup migrate persistent state as described below; no index rebuild is needed.

The initial policy deliberately keeps roles, teams, and source grants locally
managed. Administrators provision identity links using exact issuer and subject;
email matching and automatic account creation are not allowed. Local password and
MFA recovery must remain available. Setting `local_break_glass` acknowledges this
policy; the offline command cannot verify that a working administrator exists.

## Local administrator identity links

Local administrator commands can provision links now, without enabling federated
sign-in or contacting a provider. They associate an exact issuer/subject pair with
an existing enabled local account. This is an administrator assertion, not proof of
provider authentication. Obtain the subject from the trusted provider's administration
for this client registration (pairwise subjects can differ between clients); never
substitute an email, display name, or user-supplied claim.

Store the exact subject in a small owner-only regular file outside `data/`, for example
`/etc/agent-lens/oidc-subject`. Do not put it in shell arguments/history. On Unix the
file must have no group/other permissions; symbolic links are rejected. One optional
LF or CRLF file ending is removed; identity whitespace and case are otherwise preserved.
Subjects must contain 1–255 printable ASCII characters. Issuers must be bounded HTTPS
URLs without surrounding whitespace or control characters. Issuer and subject
comparisons are byte-exact, including case and trailing slash; no email fallback or
URL normalization is used for identity lookup.

Using the configuration example above and an already provisioned account:

```sh
agent-lens --state-database /var/lib/agent-lens/agent-lens-state.sqlite \
  account link-oidc --username member --config /etc/agent-lens/oidc.json \
  --subject-file /etc/agent-lens/oidc-subject

agent-lens --state-database /var/lib/agent-lens/agent-lens-state.sqlite \
  account list-oidc --username member

agent-lens --state-database /var/lib/agent-lens/agent-lens-state.sqlite \
  account unlink-oidc --username member --link-id oidc_REPLACE_WITH_LINK_ID
```

Link creation prints a random opaque `oidc_…` ID. Listing returns at most 16 links,
showing ID, issuer, and creation time, never subjects. A pair can belong to only one
local account; each account may have only one subject per exact issuer and at most
16 links overall. A duplicate is an error, not an upsert or reassignment. Replacing
a link requires explicitly unlinking its ID first. Unlinking requires both the target
local username (case-insensitive, like other account commands) and exact link ID;
it remains available for disabled accounts and without
the original provider configuration. These commands use the local state-file trust
boundary, not browser or bearer-token permissions.

Each successful link/unlink increments the target account's credential generation,
removes active browser session payloads, revokes session inventory, and records a
fixed `oidc_linked`/`oidc_unlinked` security event in one write transaction. Existing
tokens also become invalid through the generation change. Failed/conflicting changes
roll back completely, including revocation and audit writes. Local passwords, MFA
enrollment/requirements/recovery material, roles, teams, and grants are unchanged.
Expect to sign in locally again after changing your own links. The final usable
local administrator and its password are not removed by unlinking.

Links are sensitive identity metadata stored in the owner-only persistent state DB,
not the disposable index or copied raw homes. Subjects are stored for exact future
lookup, but omitted from command output and audit events. Audit records retain the
existing 180-day policy; links persist until explicitly removed. These commands do
not read client secrets, verify provider reachability, or expose any new HTTP route.

### Migration and verification

Back up stopped persistent state and its separate MFA keyring before upgrading.
State schema v10 adds `account_oidc_links` and the two fixed audit codes. Existing
accounts, valid sessions/tokens, grants, teams, preferences, TOTP ciphertext/recovery
data, passkey public credentials, and retained audit history are preserved. Merely
upgrading does not revoke credentials; an explicit successful identity change does.
Older binaries refuse schema v10: rollback requires restoring a stopped pre-upgrade
state backup, not changing the version marker. No source refresh or index rebuild
is needed. Restart after deploying; on this host restart and verify
`agent-lens.service` whenever `target/debug/agent-lens` is replaced.

Normal synthetic regressions cover exact matching, SQL-safe values, duplicates,
cross-account denial, concurrent claims, bounded storage/listing, private subject
files, audit-failure rollback, session/token revocation, disabled-account recovery,
preserved MFA requirements/local passwords, index independence, and v9 migration.
Run `bash scripts/check.sh`. Links do not yet provide a browser login path.

## Internal protocol engine (not activated)

An internal authorization-code engine is implemented using
[`openidconnect` 4.0.1](https://docs.rs/openidconnect/4.0.1/openidconnect/).
It is deliberately unreachable from production browser authentication. There is no
new route, login button, enable flag, or CLI login command in this increment. Existing
configuration/discovery checks still do not execute a login or validate a client secret.

The engine accepts bounded discovery/JWKS documents and a caller-loaded confidential
client secret when configured. It rechecks configuration, issuer pinning, discovery
capabilities, and client-authentication/secret consistency. It retains at most 32
understood public keys before selecting RS256 signing keys with 2048–8192-bit RSA
moduli. It never loads signing keys from URLs embedded in a token. Unsupported keys
are ignored; no suitable key, ambiguous signing keys, or an unknown signing key fails
closed. Explicit `--prepare` fetches the documents over verified HTTPS and reads any
client secret from protected storage. Future runtime activation must repeat those
checks; an earlier preflight is not a substitute.

Authorization requests use only `openid` scope, code response type, query response
mode, S256 PKCE, and independently random 256-bit state and nonce. The fixed planned
callback is the configured public origin plus `/login/oidc/callback`; that browser
route is not registered yet. No user-supplied redirect or forwarded host is used.

Transactions live only in process memory, with one outstanding transaction per opaque
server-owned browser binding, a five-minute lifetime, and a 1024-transaction global
ceiling. Starting again replaces that browser's transaction; expired entries are
pruned on start. A callback removes its matching browser transaction under a mutex
before any token request, checks state in constant time, and consumes the transaction
even on failure. Concurrent/replayed callbacks cannot exchange twice. A different
browser binding cannot consume or complete it, and a restart invalidates pending
transactions. State and codes are length-bounded and rejected if malformed.

The token request uses the original redirect and PKCE verifier, with public-client
authentication or client-secret Basic as configured. The HTTP adapter requires TLS
1.2+, system trust, no redirects or ambient proxies, five-second connection and
ten-second total timeouts, HTTP 200 JSON, and a 256 KiB response ceiling (including
unknown-length streams). Requests are bounded to 32 KiB. Provider error bodies and
underlying library/network diagnostics are replaced with fixed error messages.

Verification requires an RS256 signature from the prepared keys, exact issuer,
exactly one audience equal to the client ID, matching `azp` if present, the original
nonce, and unexpired `exp`. The signed `iat` may be at most 60 seconds future-dated
and at most ten minutes old. An `at_hash`, when present, must match the returned
access token. Subject validation retains the exact printable ASCII identity policy.
Email, role, team, and arbitrary extra claims do not authorize anything. The result
contains only verified issuer/subject plus an ID-token logout hint; it has no automatic
Debug/serialization implementation and must not be logged. Access/refresh tokens are
not persisted or used to fetch user information.

Independent synthetic RSA signatures cover successful public/confidential exchange,
PKCE/redirect parameters, signature/key/algorithm substitution, issuer/audience/azp,
nonce/time/subject/hash rejection, one-use state/binding/expiry/concurrency/restart,
provider errors, transaction/key/document bounds, and verified bounded TLS transport.
Run `cargo test oidc::protocol` in an environment allowing loopback test sockets, or
the full `bash scripts/check.sh`. This increment changes no state schema and needs
no refresh or index rebuild; restart after replacing the executable.

## Internal browser/session bridge (not activated)

The internal browser bridge binds the protocol engine to an anonymous server-side
session. No production OIDC routes, login button, or enable flag are registered yet.
Starting requires the existing session CSRF token and exactly one `Origin` header
matching the configured HTTPS origin. Missing, duplicate, null, cross-origin, or
malformed origins fail closed; forwarded host headers and user-supplied bindings
are not trusted. Authenticated account switching and unauthenticated local mode
are rejected. The engine and browser configuration must agree on issuer, client ID,
and callback URL.

A random opaque binding is saved through the normal session store before returning
the authorization URL. Starting again in that anonymous session replaces its pending
protocol transaction. The callback removes and saves the binding before the token
request; the protocol engine independently enforces one-use state under its mutex.
A subsequent request can resume using the server-issued session cookie, while another
browser cannot consume the original transaction. Stale parallel session data cannot
replay an already-consumed protocol transaction. Process restart loses protocol
transactions, so an old browser binding alone is insufficient to resume sign-in.

After cryptographic verification, resolution requires the exact provisioned
issuer/subject link and a currently enabled local account. The candidate contains
the link ID, current credential generation, local role/source grants/preferences,
and verified identity/logout hint. Provider email/role/team claims never grant
permissions or provision an account. Link removal, account disablement, and local
permission changes while the provider flow is pending are respected. Diagnostics
do not echo provider identity values.

The callback returns only an in-memory **candidate**, not an authenticated session.
Provider access tokens, ID tokens, and subject identity are not saved into browser
transaction payloads. The candidate must not be treated as an authorization decision
or logged/serialized.

### Gated local MFA and session completion

Internal completion consumes that candidate by value and allows one attempt within
five minutes, in the same server-issued browser session, with a fresh CSRF/exact-origin
check. Invalid factors require restarting sign-in; no reusable candidate or factor
proof is persisted. A provider's MFA claims cannot satisfy local MFA requirements.

The completion transaction rechecks the enabled local account, exact identity/link ID,
and credential generation under the SQLite writer lock, refreshes local permissions,
and uses the existing local MFA verifier. Enrolled TOTP requires a fresh one-use step
or recovery code, with the existing persistent attempt budget and recovery/audit policy.
Passkey-only accounts fail closed without a valid bound proof. The gated passkey
challenge/completion path below verifies that proof; production browser routing is
still unconnected. Existing password-first passkey login and management are unchanged.

Factor consumption and pending session inventory are committed together. The pending
inventory has an intentionally invalid browser ID and cannot authorize a session,
including if the process stops mid-completion. The old anonymous session is cleared
and its ID explicitly rotated. Saving its local account/generation/inventory payload
does not authorize it until a final conditional transaction rechecks the identity and
revocation state, binds the new browser ID, and records a fixed successful-login audit
event. Audit/binding failures revoke the pending inventory and discard the session.
Concurrent unlink, disablement, or revocation during rotation therefore fails closed.

MFA proof is set only after verifying an enrolled local factor. Required but unenrolled
accounts receive the existing enrollment-restricted session, not unrestricted access;
the normal middleware redirects protected pages to `/account/mfa`, and bearer access
remains denied. Existing session inactivity/absolute expiry, current local permissions,
and account-wide revocation continue to apply. The verified identity/logout hint stays
in memory for future logout integration, not in the browser session payload.

This code is still gated: no production OIDC route or enable flag is registered.
Production routing, secure pending-candidate handling, RP-initiated
and back-channel logout remain required before activation. Run `cargo test oidc_completion`
for signed-provider, local-factor, rotation, enrollment-middleware, audit-failure,
and deterministic revocation-during-rotation regressions.

### Gated password-first passkey completion

An OIDC candidate can start a passkey challenge only in its original browser session,
within its five-minute lifetime, after CSRF/exact-origin checks. Passkeys must already
be explicitly configured for the same HTTPS origin as OIDC. The candidate's local
account password is verified first using the normal shared password-attempt budget;
wrong passwords record a fixed failed-login event and cannot start WebAuthn. A fresh
link/account/generation check also precedes the challenge. OIDC does not turn passkeys
into passwordless authentication: they remain a second factor.

Each candidate has a separate random in-memory `o:` binding, not a request parameter
or the browser ID alone. The existing WebAuthn verifier requires user verification,
the configured origin and challenge, a valid signature from a currently enrolled key,
and current account/credential generation and signature-counter state. The assertion
ticket is one-use and subject to existing challenge lifetime/capacity and MFA-attempt
limits. An assertion cannot move to another candidate, even in the same browser, or
to ordinary password/passkey or account-management challenges.

Only a verified OIDC-bound assertion can mint a 90-second in-memory factor proof for
that candidate/account/generation. Completion consumes that proof through the same
local MFA transaction and fresh identity/session-binding gates as TOTP/recovery,
then explicitly rotates the session. Neither proof nor assertion is persisted into
the session payload. Failure or expiry requires restarting the candidate; provider
claims still cannot stand in for the local factor. Runtime restart loses pending
challenges and proofs.

`cargo test oidc_passkey` exercises signed synthetic-provider flow with real software-
authenticator signatures, password rejection, user-verification policy, same-browser
candidate isolation, CSRF/origin/configuration/expiry checks, assertion tampering,
unlink/disable/revocation, and successful MFA-verified rotated sessions. No schema
change, migration, or index rebuild is needed. Production OIDC routes remain disabled
pending browser/pending-candidate and logout integration.

Run `cargo test browser_` for session-bound signed-provider regressions, or the full
`bash scripts/check.sh`. There is no schema change, migration, or index rebuild.

## Remaining sign-in implementation

### Gated RP-initiated logout

The internal RP-initiated logout bridge requires the same-origin/CSRF-protected local
request and retains the completed sign-in context bound to its exact local session
inventory and credential generation. It revokes the current local session and clears
its cookie payload **before** preparing any provider handoff. A stale,
foreign-session, wrong-issuer, or wrong-client context cannot preserve local login
or cause another identity's provider logout. Other local sessions remain unchanged.
Local logout does not depend on the provider being reachable, accepting the request,
or returning a callback; signing out locally is not proof of provider sign-out.

The discovered, validated HTTPS `end_session_endpoint` is pinned into the protocol
engine. The prepared HTML form uses POST, not a redirect URL containing credentials.
Its hidden fields are the previously verified `id_token_hint`, configured `client_id`,
fixed `post_logout_redirect_uri`, and independent 256-bit state. Values are HTML-escaped;
the response sets `Cache-Control: no-store`, `Referrer-Policy: no-referrer`, and a CSP
allowing form submission only to the discovered provider origin, with no scripts,
base URL, or framing. The user explicitly continues provider sign-out or returns
to local sign-in. Future route/access-log configuration must not capture the sensitive
form body or logout hints. The context/request deliberately have no Debug/serialization
implementation, and the hint is not written into browser session payloads.

Register the exact planned return URL, `<public_url>/login/oidc/logout/callback`, at
the provider before any future activation. It is derived only from the configured
public origin, never from query parameters, forwarded headers, or a requested return
destination. The logout callback route is not registered yet.

After local revocation, the bridge explicitly rotates to a fresh anonymous session
and saves only a random logout binding. In-memory state is independently browser-bound,
one-use, constant-time checked, and expires after five minutes. The global pending
ceiling is 1024; expired entries are pruned on start. Wrong state consumes that
browser's transaction; another browser cannot consume the legitimate one. Restart
loses pending logout transactions without restoring local login. Callback handling
never authenticates, revokes another session, or logs out a later local login.

Run `cargo test oidc_logout` for signed-provider/session integration regressions and
`cargo test oidc::logout` for state limits, expiry/restart, configuration/hint rejection,
and rendered-form escaping. No schema change, migration, or rebuild is required.
Production routing, secure logout-context lifecycle/restart fallback, provider/browser
deployment validation, and signed replay-protected back-channel logout still block
OIDC activation.

### Remaining activation gates

Before sign-in can ship, runtime activation must verify HTTPS/Secure cookies,
owner-only client secret handling, a usable local administrator, discovery with
exact issuer matching, provider capabilities, and registered callback/logout URLs.
Discovery, key loading, and protected secret loading are implemented for explicit
preflight only; they must still be wired into runtime activation. The internal browser
bridge implements server-owned session binding, CSRF/origin initiation checks, and
provisioned identity resolution, but production HTTP routing/rendering and safe
callback error handling/logging are not registered yet.
The gated completion implements fresh enabled/link/generation/local-permission checks,
TOTP/recovery and password-first passkey verification, and rotated session establishment,
but production routing and pending-candidate handling remain unconnected. Provider
MFA/role claims must not silently bypass local requirements. RP-initiated logout is
implemented internally but needs production routing and secure context retention.
Signed replay-protected back-channel logout and end-to-end
browser/provider regressions are still required. Configuration
preflight success is not evidence of successful discovery or secure authentication.

Standards: [Discovery](https://openid.net/specs/openid-connect-discovery-1_0.html),
[Core](https://openid.net/specs/openid-connect-core-1_0.html),
[RP-initiated logout](https://openid.net/specs/openid-connect-rpinitiated-1_0.html),
and [back-channel logout](https://openid.net/specs/openid-connect-backchannel-1_0.html).
