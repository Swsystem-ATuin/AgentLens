# Teams and intermediate roles

State schema v6 adds stable teams, many-to-many account membership, and team source grants. Application accounts retain one explicit capability role. Teams grant source access; they do not assign or elevate roles.

## Role policy

| Role | Source content | Usage/diagnostic metadata and redacted exports | Installation health and metrics | Source scope |
| --- | --- | --- | --- | --- |
| `admin` | Yes | Yes | Yes | All sources |
| `viewer` | Yes | Yes | No | Direct and team grants |
| `auditor` | No | Yes | No | Direct and team grants |
| `operator` | Yes | Yes | Yes | Direct and team grants for source content/reports |

All four roles can manage their own profile, preferences, password, sessions, and tokens, inspect their own security history, and explain their effective access. Only administrators can use [audited browser administration and installation security review](browser-administration.md). Source-refresh mutations and annotations remain reserved. Local CLI operations retain their filesystem recovery boundary.

Auditors start at `/usage` and receive `403` on session lists/search, transcripts, messages, tool arguments/results, turns, generated images, and live-status routes. Reports still include their existing metadata, such as session titles, model names, project labels, origins, timestamps, and counts. The auditor role does not claim PII detection or field masking; broader redacted-content disclosure policy remains M5.5c. Operators receive installation-wide operational metadata even for sources outside their content grants; grant this role only when that access is intended. Intermediate-role capabilities live in the shared [capability policy](capabilities.md), and personal access tokens intersect their scope with the current role on every request.

## Additive source access

Effective source access is the sorted, deduplicated union of direct grants and grants from every team an account belongs to. There are no explicit deny grants, inherited roles, nested teams, or precedence rules. A duplicate grant has no extra effect. Removing a direct grant, team grant, or membership removes only that contribution; access remains if another contribution covers the source. Renaming a team preserves its identifier and permissions. Deleting a team atomically removes its memberships and grants through foreign-key cascades and preserves direct grants and grants from other teams.

Every authenticated request resolves current effective grants before lists, counts, pagination, session lookups, reports, or bearer endpoints execute. Existing browser sessions and tokens observe membership/grant changes on the next request. Missing or ungranted session IDs retain `404`; a role without the required capability receives `403`. Grants use stable source IDs, so rebuilding the disposable agent index preserves team membership and source grants.

`/account/access` explains the current account's capabilities and each source grant's direct or team origin. Source origins have deterministic source-ID/team-ID ordering, use the selected 25/50/100-row page size, and fetch one continuation row. Team names are escaped in HTML. An admin's stored grants are shown as retained exceptions, with an explicit explanation that its role currently covers all sources. Other accounts' memberships and grants are not exposed by this page.

## Local management

Create a team and copy the returned `team_…` identifier into subsequent commands. Identifiers are randomly generated independently of names and remain stable across renames. Names contain 1–80 non-control characters after trimming outer whitespace, and are unique using SQLite's ASCII case-insensitive comparison.

```bash
cargo run -- team create --name 'Example analysts'
cargo run -- team add-member --team-id team_REPLACE_WITH_RETURNED_ID --username analyst
cargo run -- team grant-source --team-id team_REPLACE_WITH_RETURNED_ID --source-id src_REPLACE_WITH_64_HEX_CHARACTERS
cargo run -- team inspect --team-id team_REPLACE_WITH_RETURNED_ID
cargo run -- account permissions --username analyst
cargo run -- account set-role --username analyst --role auditor
cargo run -- account set-role --username analyst --role operator
cargo run -- team rename --team-id team_REPLACE_WITH_RETURNED_ID --name 'Renamed analysts'
cargo run -- team revoke-source --team-id team_REPLACE_WITH_RETURNED_ID --source-id src_REPLACE_WITH_64_HEX_CHARACTERS
cargo run -- team remove-member --team-id team_REPLACE_WITH_RETURNED_ID --username analyst
cargo run -- team delete --team-id team_REPLACE_WITH_RETURNED_ID
cargo run -- team list --page 1
```

Provision accounts first using the [account creation workflow](application-authentication.md). Team inspection lists sorted members and source grants; account permissions shows deduplicated effective sources followed by all grant origins. Team lists return 50 rows per page. Commands accept the global `--state-database` option and never open the disposable index. Creation, rename, deletion, membership changes, and team grant changes append fixed audit events atomically with the mutation. Events retain the stable team ID without storing arbitrary names or source content; membership events also link to the affected account. Global team events remain in the owner-only ledger pending the later audited browser administration surface.

## Administrator safeguards and migration

The first account must be an administrator. Database triggers reject disabling, deleting, or demoting the last enabled administrator, including concurrent operations and direct SQL updates. A disabled administrator does not count as the remaining administrator. Create, enable, or promote another administrator before removing the current one. These guards do not prevent password reset, normal session/token revocation, or local role recovery in a legacy installation that already lacks an enabled administrator. Team membership cannot grant or remove administrator status.

Before upgrading, back up the stopped persistent state database as described in [application authentication](application-authentication.md). Opening a v1–v5 state database migrates it to v6. SQLite requires replacing the account table to expand its role CHECK constraint. Migration uses a dedicated connection, verifies all foreign-key references before committing, restores enforcement, and closes that connection even on cancellation. Existing account IDs, passwords, generations, grants, preferences, live sessions, tokens, and retained security events are preserved; failed integrity validation rolls back the account replacement. New connections keep foreign keys enabled. No source refresh or index rebuild is required. Restart the service after deploying; on this host, also restart whenever `target/debug/agent-lens` is replaced.

## Verification

Run `bash scripts/check.sh`. Synthetic regressions cover all four roles and token ceilings, union deduplication, overlapping grants, rename/delete/member removal, account isolation, escaped grant explanations and bounded pages, immediate browser/token permission changes, auditor transcript denial, operator operational access, concurrent last-admin removals, raw SQL safeguards, CLI commands, v5 migration preserving sessions/tokens/preferences/grants, migration rollback, and foreign-key enforcement across the connection pool. Tests do not read private development fixtures.
