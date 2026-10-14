# Application access permissions

Agent Lens authorizes application accounts independently of the operating-system users indexed from copied agent homes. The policy has two roles:

- `admin` accounts can read every indexed source and use the operational `/health` and `/metrics` endpoints.
- `viewer` accounts can read only sources explicitly granted by stable Agent Lens source ID. A viewer with no grants sees empty session, usage, and diagnostics views.

The first account in a new state database defaults to `admin`; later accounts default to `viewer`. `account create --role admin|viewer` can select a role explicitly. Accounts created before the permission schema upgrade migrate to `admin`, preserving their existing visibility. There is no browser administration surface: role and grant changes require local access to the application state database through the CLI.

## Grant workflow

An administrator can copy a `src_…` source ID from the authenticated Source health page. The ID represents one discovered agent home and remains stable across refreshes and complete index rebuilds as long as its agent kind, logical host, logical operating-system user, and logical source-home path remain the same.

```bash
cargo run -- account create --username analyst --password-file /run/secrets/analyst-password
cargo run -- account grant-source --username analyst --source-id src_REPLACE_WITH_64_HEX_CHARACTERS
cargo run -- account permissions --username analyst
cargo run -- account revoke-source --username analyst --source-id src_REPLACE_WITH_64_HEX_CHARACTERS
cargo run -- account set-role --username analyst --role admin
cargo run -- account set-role --username analyst --role viewer
```

Granting a source exposes all current and future sessions projected beneath that source, including active and archived transcripts, messages, tool events, turns, usage, diagnostics, live status, and redacted postmortems. It does not grant access to another source on the same host or belonging to the same indexed operating-system user. Stored source grants are retained when an account is temporarily promoted to `admin`, so demotion restores its previous source scope.

Role, grant, revoke, disable, and account deletion effects are evaluated from persistent state on every request; an existing browser session does not need to log in again. Password reset and session revocation remain authentication controls and do not change permissions.

## Enforcement and disclosure behavior

Session lists, transcript search, every filter option, usage totals and breakdowns, diagnostic totals and evidence, transcript pages, message/tool fragments, turn history, live status, and postmortem exports all apply the same source scope. Pagination and aggregates are computed after authorization, so hidden rows do not affect counts, options, ordering, or page boundaries.

An unauthorized stable session or child resource returns the same `404` as an unknown session. This avoids confirming that another account's session exists. Operational health and Prometheus metrics contain installation-wide ingest information and therefore return `403` to viewers. Viewer Basic credentials cannot scrape `/metrics`. The [typed capability matrix](capabilities.md) classifies implemented routes and methods; unknown routes and unsupported methods fail closed for every role until deliberately implemented and tested.

The policy is enforced server-side; hiding host/user controls or links in rendered HTML is never treated as authorization. All permission SQL remains parameterized. Source grants are serialized into one bounded SQLite JSON value per request, avoiding a variable-count SQL placeholder list as grants grow.

## Persistent boundary and recovery

Roles and grants live in `agent-lens-state.sqlite`, not the disposable index. Grants reference stable `src_…` identities rather than index row IDs. Refresh and rebuild do not modify them. State schema v2 adds roles and grants; opening a v1 state database migrates existing accounts to `admin` so an upgrade does not silently remove access. A database from a newer unsupported schema fails closed without downgrading its version. If a logical source identity changes, its old grant safely matches nothing until a local administrator grants the new source ID.

Use the same stopped-service, owner-only backup and restore procedure documented in [application authentication](application-authentication.md). A local administrator who removes all administrator access can recover with `account set-role`; no indexed data rebuild is required.

## Verification

Synthetic two-account regressions cover administrator visibility, viewer empty/default behavior, source-scoped lists, search snippets, filter values, usage, diagnostics, transcripts, live status, turns, messages, tool request/result details, and postmortem exports. They verify `404` for an ungranted stable ID, `403` for viewer health and session/Basic-auth metrics access, fail-closed future routes, immediate grant/revoke/promotion/demotion on an existing session, stable-grant survival, and migration of pre-permission accounts to administrators. No private source or account data is used.
