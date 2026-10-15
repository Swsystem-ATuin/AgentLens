# Application access permissions

Agent Lens authorizes application accounts independently of the operating-system users indexed from copied agent homes. The policy has four roles:

- `admin` accounts can read every indexed source and use the operational `/health` and `/metrics` endpoints.
- `viewer` accounts can read sources granted directly or through teams by stable source ID. No grants means empty session and report views.
- `auditor` accounts can read scoped reports, not transcripts or their child routes; report metadata is not a PII-masking guarantee.
- `operator` accounts have viewer content/report access plus installation-wide health and metrics, but no unrestricted source access or account administration.

The first account must be `admin`; later accounts default to `viewer`. `account create --role admin|viewer|auditor|operator` selects a role explicitly. Pre-permission accounts migrate to `admin`, preserving visibility. There is no browser administration surface: role, team, and grant changes require local CLI access. The signed-in `/account/access` page explains one's own capabilities and grant origins. See [teams and roles](teams-and-roles.md) for exact capabilities, metadata disclosure, additive union semantics, and commands.

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

Granting a source permits the role's capabilities across its current and future sessions: viewers/operators receive content and reports, auditors receive reports only. It does not grant another source on the same host or indexed user. Direct and member-team grants form a sorted, deduplicated union; removing one origin preserves any independent grant. Grants survive temporary promotion to `admin`.

Role, grant, revoke, disable, and account deletion effects are evaluated from persistent state on every request; an existing browser session does not need to log in again. Password reset and session revocation remain authentication controls and do not change permissions.

## Enforcement and disclosure behavior

Session lists, transcript search, every filter option, usage totals and breakdowns, diagnostic totals and evidence, transcript pages, message/tool fragments, turn history, live status, and postmortem exports all apply the same source scope. Pagination and aggregates are computed after authorization, so hidden rows do not affect counts, options, ordering, or page boundaries.

An ungranted session or child resource returns the same `404` as an unknown session. Role-denied routes return `403`, including auditor content routes and viewer/auditor installation-wide health and metrics. Admin/operator Basic credentials or appropriately scoped tokens can scrape `/metrics`. The [typed capability matrix](capabilities.md) classifies routes and methods; unknown routes and unsupported methods fail closed for every role.

The policy is enforced server-side; hiding host/user controls or links in rendered HTML is never treated as authorization. All permission SQL remains parameterized. Source grants are serialized into one bounded SQLite JSON value per request, avoiding a variable-count SQL placeholder list as grants grow.

## Persistent boundary and recovery

Roles and grants live in `agent-lens-state.sqlite`, not the disposable index. Grants reference stable `src_…` identities rather than index row IDs. Refresh and rebuild do not modify them. State schema v2 adds roles and grants; opening a v1 state database migrates existing accounts to `admin` so an upgrade does not silently remove access. A database from a newer unsupported schema fails closed without downgrading its version. If a logical source identity changes, its old grant safely matches nothing until a local administrator grants the new source ID.

Use the stopped-service, owner-only backup and restore procedure in [application authentication](application-authentication.md). Current [schema v6](teams-and-roles.md) preserves existing state and adds teams and roles without an index rebuild. Database guards reject disabling, deleting, or demoting the last enabled administrator, including concurrent updates. Local CLI provisioning, enabling, or promoting an alternative administrator remains the break-glass boundary.

## Verification

Synthetic two-account regressions cover administrator visibility, viewer empty/default behavior, source-scoped lists, search snippets, filter values, usage, diagnostics, transcripts, live status, turns, messages, tool request/result details, and postmortem exports. They verify `404` for an ungranted stable ID, `403` for viewer health and session/Basic-auth metrics access, fail-closed future routes, immediate grant/revoke/promotion/demotion on an existing session, stable-grant survival, and migration of pre-permission accounts to administrators. No private source or account data is used.
