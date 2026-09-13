# Historical usage dashboard

The authenticated `/usage` route aggregates the checked cumulative-counter deltas described in [token accounting semantics](token-accounting.md). It never sums raw cumulative snapshots or assigns a session's lifetime summary to its last-modified date.

## Time and grouping

Each delta is attributed to the timestamp of the source observation that reported it. The configured fixed offset from the project configuration is applied before inclusive `from` and `to` dates and before period boundaries. Days start at local midnight, weeks start on Monday, and months use the local calendar month. An observation without a SQLite-readable timestamp cannot be placed in a period; the dashboard reports its count separately and excludes it from period totals.

This is observation-time attribution, not billing-time attribution. In particular, the first usable cumulative snapshot can include tokens consumed before its timestamp, and a value following a missing snapshot can include usage from that gap. The page states this limitation next to the totals.

The breakdown can group by session, model, configured project, host, operating-system user, or agent. Missing models and unmatched projects have explicit `Unknown model` and `Unassigned project` labels. Direct-home indexes omit host/user choices and ignore a supplied host/user grouping, consistent with the sessions browser. Codex hidden-model policy applies to overview totals, breakdowns, and recent observations alike.

## Values and availability

Input, cached input, output, reasoning output, and source-reported total remain separate metrics. Cached input is not added to input, and reasoning output is not added to output. Cache ratio is `cached input / input`; it is `Unavailable` for a zero or unavailable denominator. Reasoning remains `Unavailable` when the source does not provide it.

When only some observations in an aggregate provide a metric, the displayed known subtotal and ratio are labeled `partial`. This avoids turning unavailable evidence into zero. Reset, duplicate, missing, and inconsistent counts remain visible beside the overview, while grouped rows count observations carrying reset, missing, or inconsistent evidence.

## Bounds and drill-down

The breakdown returns 100 rows per page and preserves period, grouping, and date range in navigation. Recent activity is limited to the latest 20 timestamped observations within the selected range. Session-group rows and recent observations link to the corresponding transcript using its stable public ID. Overview totals always cover the complete selected range, even when breakdown rows span multiple pages.

Run one `refresh` after upgrading from parser version 4 or earlier so legacy order-unknown usage rows are replaced with calculated deltas. No additional rebuild or schema migration is required for the dashboard itself.

## Verification

Run `bash scripts/check.sh`. Synthetic route/query regressions cover fixed-offset day and month boundaries, Monday-based weeks, all six grouping dimensions, unknown models, zero denominators, partially available reasoning, hidden-model scope, unattributed timestamps, recent transcript links, invalid ranges, direct-home origin suppression, bounded pagination, and reconciliation of a complete grouped result with its overview totals. Tests do not read the private development fixture.
