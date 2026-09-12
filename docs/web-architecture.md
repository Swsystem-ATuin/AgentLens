# Web query and presentation boundaries

The server-rendered web application is split into three focused layers under `src/web/`:

- `web.rs` owns Axum routing, request extraction and validation, authentication middleware composition, sanitized HTTP error mapping, and final Askama rendering. Handlers coordinate the query and presentation layers but contain no SQL or transcript classification rules.
- `queries.rs` owns SQLite row types and reads for session pages, filter values, runtime project classification, source-layout display state, transcript sections, and message chunks. Query limits remain next-page-aware: 51 session rows for a 50-row page and 51 message rows for a 50-message section. Project expressions and every filter value are parameterized. Message projections and chunks retain the documented character limits.
- `presentation.rs` converts query results into filter options and transcript segments, groups consecutive assistant replies, applies source-dispatched message classifications, and constructs safely encoded navigation links. It has no database or HTTP dependencies.

Source-specific interpretation remains in `src/sources/`. The query layer applies the generic source policy returned by that dispatcher, while presentation asks it how a stored message should be labeled and opened. Neither web submodule inspects raw Codex JSON.

The database-backed route tests exercise the boundaries together, including composable project/date/archive/agent filters, stable filter pagination, filtered-empty states, direct-home origin suppression, source-specific hidden models, bounded transcript sections, large-message parts, initial and resumed setup classification, ordinary conversation, authentication, and sanitized errors. Unit tests cover path grouping, date validation, transcript grouping, and URL encoding directly.

Project navigation adds query parameters but changes no routes or raw parsing. The disposable index automatically gains an activity/public-ID ordering index. No refresh or rebuild is required; restart the server to load code or project-configuration changes.
