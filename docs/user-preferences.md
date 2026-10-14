# User preferences

Each authenticated account can update its own display and browsing preferences from `/account`. Preferences are application-owned state: they live in the owner-only `agent-lens-state.sqlite` database, remain independent of the disposable agent index, and never modify copied source homes.

## Stored settings

State schema version 3 adds one typed `account_preferences` row per account. Columns have explicit types and defaults rather than storing an unchecked JSON document:

- timezone mode: UTC, browser local, or a validated IANA name such as `Europe/London`;
- ISO-style, long 24-hour, or long 12-hour timestamps;
- system, light, dark, or high-contrast appearance;
- 90%, 100%, 110%, or 125% font scale;
- 25, 50, or 100 rows/events per page;
- a 5, 10, 30, or 60 second live-reader interval and its initial paused state;
- compact, standard, or expanded transcript detail and code wrapping;
- all, active, or archived as the default sessions view.

The update endpoint is authenticated, accepts `POST` only, and verifies the account session's CSRF token. The account ID comes from the authenticated session rather than form input. SQL parameters are bound, and generated CSS and JavaScript contain only values selected from validated enums and numeric allowlists. Unknown or stale stored values fall back by field to safe defaults and do not prevent sign-in, allowing an older binary to tolerate settings introduced by a newer one.

Local mode has no application identity to own preferences. It therefore uses the defaults and keeps the preference form unavailable.

## Time semantics

Raw timestamps remain unchanged in the rebuildable index. UTC is the default preference and the no-script fallback. A named IANA timezone is validated against the bundled timezone database and applied to session date filters, usage ranges and daily/weekly/monthly grouping as well as rendered session, transcript, turn, usage, diagnostic, health, message, and tool timestamps. Cached transition tables generate SQLite modifier expressions so historical daylight-saving boundaries use the offset at each observation, not today's offset.

Browser-local mode uses a same-origin script to discover the browser's IANA timezone. On the sessions and usage pages it adds the validated name to the URL before rendering timezone-dependent results; pagination retains it. Other timestamps are converted with the browser's `Intl.DateTimeFormat`. If scripting or browser timezone discovery is unavailable, the same content and forms remain usable and timestamps and date boundaries fall back explicitly to UTC.

## Bounded behavior

The selected page size applies consistently to sessions, usage groups, transcript events, turn history, source health, and evidence links. List queries fetch at most the selected size plus one row for next-page detection; the allowed maximum is 100. Existing fixed diagnostic and recent-usage caps remain in force. Message and tool bodies retain their independent character limits. The synthetic 30-session regression verifies that a 25-row selection renders exactly 25 links, exposes continuation, and stays below the 256 KiB response ceiling.

Appearance is served as an authenticated same-origin stylesheet, so theme, contrast, font scale, and code wrapping work without JavaScript. The small authenticated preference script only supplies browser-local conversion and live-reader defaults. All responses retain `Cache-Control: no-store` and the existing restrictive content security policy.

## Migration and operation

Schema version 3 introduced `account_preferences`; the current binary migrates older state databases through that change and the later [session-security schema](session-security.md) automatically. Existing accounts receive preference defaults lazily; no bulk row creation is required. Back up the stopped state database before deploying as described in [application authentication](application-authentication.md). Restart the web service to run migrations and load the assets. No source refresh or agent-index rebuild is required.

Synthetic regressions cover schema migration, two-account isolation, unknown-value fallback, invalid IANA names and allowlist values, CSRF rejection, persistence, rendered form selection, authenticated CSS/JavaScript, daylight-saving boundaries, the no-script UTC explanation, default session filtering, and 25-row bounded output.
