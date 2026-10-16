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

## Remaining sign-in implementation

Before sign-in can ship, runtime activation must verify HTTPS/Secure cookies,
owner-only client secret handling, a usable local administrator, discovery with
exact issuer matching, provider capabilities, and registered callback/logout URLs.
Discovery, key loading, and protected secret loading are implemented for explicit
preflight only; they must still be wired into runtime activation. Browser integration must bind the engine to a server-owned
session, require CSRF/origin checks for initiating login, avoid logging callback
codes/state, and resolve only the administrator-provisioned issuer/subject link.
Current enabled state, identity link, credential generation, local permissions, and
MFA must be rechecked before establishing a rotated local session. Provider MFA/role
claims must not silently bypass local requirements. Local session revocation/lifetime,
RP-initiated logout, signed replay-protected back-channel logout, and end-to-end
browser/provider regressions are still required. Configuration
preflight success is not evidence of successful discovery or secure authentication.

Standards: [Discovery](https://openid.net/specs/openid-connect-discovery-1_0.html),
[Core](https://openid.net/specs/openid-connect-core-1_0.html),
[RP-initiated logout](https://openid.net/specs/openid-connect-rpinitiated-1_0.html),
and [back-channel logout](https://openid.net/specs/openid-connect-backchannel-1_0.html).
