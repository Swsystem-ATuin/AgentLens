# Capability authorization

The shared policy in `src/authorization.rs` maps live account roles to typed capabilities and classifies implemented HTTP routes by method and path. Browser sessions and metrics Basic authentication use this evaluator after checking the current account. Unknown routes and unsupported methods return `403` for all roles. Local loopback mode retains its explicit unrestricted development behavior.

| Capability | Admin | Viewer | Auditor | Operator | Current surface |
| --- | --- | --- | --- | --- | --- |
| ContentRead | Yes | Scoped | No | Scoped | Sessions, search, transcripts, turns, live status, artifacts, message/tool details |
| ReportsRead | Yes | Scoped | Scoped | Scoped | Usage, diagnostic metadata, redacted postmortem exports |
| OperationsRead | Yes | No | No | Yes | Installation-wide health and metrics |
| AllSourcesRead | Yes | No | No | No | Bypass explicit source grants for authorized data reads |
| AccountSelfManage | Own | Own | Own | Own | Profile, preferences, password, logout, session revocation, effective-access explanation |
| AuditSelfRead | Own | Own | Own | Own | Session inventory and security history |
| SourceRefresh | No | No | No | No | Reserved for a future authenticated refresh surface |
| AnnotationsWrite | No | No | No | No | Reserved for future annotations |
| AccountsAdminister | Yes | No | No | No | Confirmed, CSRF-protected account and team administration |
| AuditRead | Yes | No | No | No | Bounded installation security and administration history |
| TokensManage | Own | Own | Own | Own | Personal access token creation, listing, and revocation |

Source refresh and annotation capabilities remain reserved without HTTP grants. [Browser administration](browser-administration.md) uses recent password confirmation, CSRF, transactional current-capability/session checks, and atomic audit facts. Local CLI commands retain their filesystem recovery boundary. Identity-free local mode cannot administer accounts or review their audit history.

## Source and account scope

Capabilities authorize actions; source grants limit the records those actions can see. [Team grants and direct grants](teams-and-roles.md) combine as a deterministic union. Parameterized query predicates use `AllSourcesRead` for installation-wide visibility and otherwise apply effective stable source grants before filtering, counts, aggregates, and pagination. A scope without content or report access supplies neither an unrestricted predicate nor source grants. Session and child-resource lookups retain `404` for ungranted identifiers. Self-service handlers derive the account ID from the authenticated session and retain CSRF validation for mutations.

The capability set supports intersection with an explicit credential ceiling. [Personal access tokens](personal-access-tokens.md) use the live account's set intersected with token scopes, retain account disablement and current source-grant enforcement, and use a separate allowlist of machine-readable read endpoints. Tokens cannot add a capability absent from the account or invoke browser self-service routes.

## Verification and deployment

Run `bash scripts/check.sh`. The suite verifies all four roles, credential ceilings, unsupported methods, unknown/reserved routes, metrics Basic authorization, browser administration, and source isolation. Role, team membership, and grant changes take effect on the next request. Schema v7 adds administrative audit facts while preserving existing roles and teams; no source refresh or index rebuild is required. Restart after deploying.
