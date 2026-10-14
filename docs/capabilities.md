# Capability authorization

The shared policy in `src/authorization.rs` maps live account roles to typed capabilities and classifies implemented HTTP routes by method and path. Browser sessions and metrics Basic authentication use this evaluator after checking the current account. Unknown routes and unsupported methods return `403` for both roles. Local loopback mode retains its explicit unrestricted development behavior.

| Capability | Admin | Viewer | Current surface |
| --- | --- | --- | --- |
| ContentRead | Yes | Yes, granted sources | Sessions, search, transcripts, turns, live status, artifacts, message/tool details |
| ReportsRead | Yes | Yes, granted sources | Usage, diagnostics, redacted postmortem exports |
| OperationsRead | Yes | No | Installation-wide health and metrics |
| AllSourcesRead | Yes | No | Bypass explicit source grants for authorized data reads |
| AccountSelfManage | Own account | Own account | Profile, preferences, password, logout, session revocation |
| AuditSelfRead | Own account | Own account | Session inventory and security history |
| SourceRefresh | No | No | Reserved for a future authenticated refresh surface |
| AnnotationsWrite | No | No | Reserved for future annotations |
| AccountsAdminister | No | No | Reserved for browser administration |
| AuditRead | No | No | Reserved for installation-wide audit review |
| TokensManage | No | No | Reserved for personal access token management |

Reserved capabilities have no HTTP routes or browser grants. Local administrator CLI commands continue to use filesystem access to the state database as their recovery boundary. Capability names establish the vocabulary for later roadmap work without enabling that work.

## Source and account scope

Capabilities authorize actions; source grants limit the records those actions can see. Parameterized query predicates use `AllSourcesRead` for installation-wide visibility and otherwise apply stable source grants before filtering, counts, aggregates, and pagination. A scope without content or report access supplies neither an unrestricted predicate nor source grants. Session and child-resource lookups retain `404` for ungranted identifiers. Self-service handlers derive the account ID from the authenticated session and retain CSRF validation for mutations.

The capability set supports intersection with an explicit credential ceiling. Future personal access tokens must use the live account's set intersected with token scopes, retain account disablement and current source-grant enforcement, and explicitly classify token endpoints. Tokens cannot add a capability absent from the account. Bearer authentication and token endpoints are not introduced by this change; they are M5.3.

## Verification and deployment

Run `bash scripts/check.sh`. The normal test suite verifies the complete role matrix, every credential-ceiling intersection, unsupported methods, unknown and reserved routes for both roles, metrics Basic authorization, and existing source-isolation query/render/export regressions. Role and grant changes still take effect on the next request. No state migration, source refresh, or index rebuild is required; restart the web service after deploying the new binary.
