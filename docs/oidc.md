# OpenID Connect configuration preflight

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

Every field is required; unknown fields and unsupported policy values are rejected.
Use `{"method":"none"}` only for a provider registration supporting public clients.
Secret values are not accepted inline. The secret path must be absolute; preflight
does not check file existence, permissions, contents, or provider compatibility.
There are no database migrations or index rebuild requirements.

The initial policy deliberately keeps roles, teams, and source grants locally
managed. Administrators will provision identity links using exact issuer and subject;
email matching and automatic account creation are not allowed. Local password and
MFA recovery must remain available. Setting `local_break_glass` acknowledges this
policy; the offline command cannot verify that a working administrator exists.

Before sign-in can ship, runtime activation must verify HTTPS/Secure cookies,
owner-only client secret handling, a usable local administrator, discovery with
exact issuer matching, provider capabilities, and registered callback/logout URLs.
Protocol work must cover authorization code with PKCE, state/nonce and ID-token
validation, bounded single-use transactions, local session revocation/lifetime,
RP-initiated logout and signed replay-protected back-channel logout. Configuration
preflight success is not evidence of successful discovery or secure authentication.

Standards: [Discovery](https://openid.net/specs/openid-connect-discovery-1_0.html),
[Core](https://openid.net/specs/openid-connect-core-1_0.html),
[RP-initiated logout](https://openid.net/specs/openid-connect-rpinitiated-1_0.html),
and [back-channel logout](https://openid.net/specs/openid-connect-backchannel-1_0.html).
