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

## Remaining sign-in implementation

Before sign-in can ship, runtime activation must verify HTTPS/Secure cookies,
owner-only client secret handling, a usable local administrator, discovery with
exact issuer matching, provider capabilities, and registered callback/logout URLs.
The discovery gate is implemented for explicit preflight only; it must still be wired
into runtime activation. Protocol work must cover authorization code with PKCE, state/nonce and ID-token
validation, bounded single-use transactions, local session revocation/lifetime,
RP-initiated logout and signed replay-protected back-channel logout. Configuration
preflight success is not evidence of successful discovery or secure authentication.

Standards: [Discovery](https://openid.net/specs/openid-connect-discovery-1_0.html),
[Core](https://openid.net/specs/openid-connect-core-1_0.html),
[RP-initiated logout](https://openid.net/specs/openid-connect-rpinitiated-1_0.html),
and [back-channel logout](https://openid.net/specs/openid-connect-backchannel-1_0.html).
