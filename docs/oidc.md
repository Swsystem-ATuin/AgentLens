# OpenID Connect configuration and discovery preflight

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
There are no database migrations or index rebuild requirements.

The initial policy deliberately keeps roles, teams, and source grants locally
managed. Administrators will provision identity links using exact issuer and subject;
email matching and automatic account creation are not allowed. Local password and
MFA recovery must remain available. Setting `local_break_glass` acknowledges this
policy; the offline command cannot verify that a working administrator exists.

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
