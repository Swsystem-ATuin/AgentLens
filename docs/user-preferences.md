# User preferences

Each authenticated account can update its own display and browsing preferences from `/account`. Account preferences live in the owner-only `agent-lens-state.sqlite` database. With `serve --local`, the same display/browsing settings are available without an account and are saved in a separate browser-local cookie. Neither mode modifies copied source homes or the disposable index.

## Local-mode preferences

With `serve --local`, `/account` exposes timezone/date formatting, appearance, font
scale, rows/events per page (including All), live-reader defaults, transcript detail,
code wrapping, and default session view. Profile/display-name changes, passwords,
MFA/passkeys, tokens, session/security history, and administrator controls are not
made available. Local mode remains loopback-only and unauthenticated.

Saving still requires the anonymous browser session's CSRF token and the same
allowlisted preference validation used for accounts. Settings are encoded in the
separate `agent_lens_local_preferences` cookie, scoped to `/`, HttpOnly, SameSite=Lax,
and retained for up to one year. It contains only non-sensitive display choices,
not credentials or an authenticated identity. It is intentionally not an
authentication token: editing it cannot grant permissions. Malformed, oversized,
ambiguous, or unsupported values fall back to safe defaults without logging contents.

The cookie follows normal host/browser cookie scope. Another browser profile starts
with defaults; clearing cookies resets these local choices. They survive service
restart and expiry of the separate short-lived anonymous CSRF session. No account,
account-preference row, authentication inventory, or security event is created when
saving local settings. Switching to authenticated serving ignores this cookie and
uses the signed-in account's database preferences instead; it does not bypass login.

## Stored settings

State schema version 3 adds one typed `account_preferences` row per account. Columns have explicit types and defaults rather than storing an unchecked JSON document:

- timezone mode: UTC, browser local, or a validated IANA name such as `Europe/London`;
- ISO-style, long 24-hour, or long 12-hour timestamps;
- system, light, dark, or high-contrast appearance;
- 90%, 100%, 110%, or 125% font scale;
- 25, 50, 100, or All rows/events per page;
- a 5, 10, 30, or 60 second live-reader interval and its initial paused state;
- compact, standard, or expanded transcript detail and code wrapping;
- all, active, or archived as the default sessions view.

The update endpoint is authenticated, accepts `POST` only, and verifies the account session's CSRF token. The account ID comes from the authenticated session rather than form input. SQL parameters are bound, and generated CSS and JavaScript contain only values selected from validated enums and numeric allowlists. Unknown or stale stored values fall back by field to safe defaults and do not prevent sign-in, allowing an older binary to tolerate settings introduced by a newer one.

Local mode has no application identity to own preferences. It therefore uses the defaults and keeps the preference form unavailable.

## Time semantics

Raw timestamps remain unchanged in the rebuildable index. UTC is the default preference and the no-script fallback. A named IANA timezone is validated against the bundled timezone database and applied to session date filters, usage ranges and daily/weekly/monthly grouping as well as rendered session, transcript, turn, usage, diagnostic, health, message, and tool timestamps. Cached transition tables generate SQLite modifier expressions so historical daylight-saving boundaries use the offset at each observation, not today's offset.

Browser-local mode uses a same-origin script to discover the browser's IANA timezone. On the sessions and usage pages it adds the validated name to the URL before rendering timezone-dependent results; pagination retains it. Other timestamps are converted with the browser's `Intl.DateTimeFormat`. If scripting or browser timezone discovery is unavailable, the same content and forms remain usable and timestamps and date boundaries fall back explicitly to UTC.

## Bounded behavior

The selected page size applies consistently to sessions, usage groups, transcript events, turn history, source health, account/security history, token lists, administrative lists, and evidence links. The numbered choices fetch the selected size plus one row for next-page detection. **All** loads every matching authorized row/event on one page and suppresses continuation links; old page/section URLs resolve to that single page. Filters and source/capability restrictions still apply. Existing fixed diagnostic and recent-usage caps remain in force, and message/tool bodies keep their separate preview/detail character limits. All is an explicit opt-in and may use substantially more memory and take longer for large histories; the default remains 50. The preference is persisted as numeric `page_size=0` without a schema change; invalid or malformed values still fall back to 50 rather than accidentally enabling All.

Synthetic rendered-HTML regressions cover more than 100 authorized rows/events, initial and resumed setup blocks staying collapsed, ordinary conversation preservation, filtering and forbidden sources, administrative rows, safe evidence/page arithmetic, and persistence after reopening state. The existing 30-session regression still verifies exactly 25 links and continuation for a bounded choice.

Appearance is served as a same-origin stylesheet (account-authenticated outside local mode), so theme, contrast, font scale, and code wrapping work without JavaScript. The preference script only supplies browser-local conversion and live-reader defaults. All responses retain `Cache-Control: no-store` and the existing restrictive content security policy.

## Migration and operation

Schema version 3 introduced `account_preferences`; the current binary migrates older state databases through that change and the later [session-security schema](session-security.md) automatically. Existing accounts receive preference defaults lazily; no bulk row creation is required. Back up the stopped state database before deploying as described in [application authentication](application-authentication.md). Restart the web service to run migrations and load the assets. No source refresh or agent-index rebuild is required.

Synthetic regressions cover schema migration, two-account isolation, unknown-value fallback, invalid IANA names and allowlist values, CSRF rejection, persistence, rendered form selection, authenticated CSS/JavaScript, daylight-saving boundaries, the no-script UTC explanation, default session filtering, and 25-row bounded output.

`cargo test local_preferences` additionally verifies local form rendering, security-
control exclusion, CSRF/invalid-value rejection, cookie bounds/flags, browser isolation,
restart/session-expiry persistence, CSS/JavaScript preferences, and authenticated-mode
separation. This local-mode change needs no schema migration, source refresh, or rebuild;
restart after deploying the new executable.
