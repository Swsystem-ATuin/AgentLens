# Web query and presentation boundaries

The server-rendered web application is split into three focused layers under `src/web/`:

- `web.rs` owns Axum routing, request extraction and validation, authentication middleware composition, sanitized HTTP error mapping, and final Askama rendering. Handlers coordinate the query and presentation layers but contain no SQL or transcript classification rules.
- `queries.rs` owns SQLite row types and reads for session pages, filter values, source-layout display state, transcript sections, and message chunks. Query limits remain next-page-aware: 51 session rows for a 50-row page and 51 message rows for a 50-message section. Message projections and chunks retain the documented character limits.
- `presentation.rs` converts query results into filter options and transcript segments, groups consecutive assistant replies, applies source-dispatched message classifications, and constructs safely encoded navigation links. It has no database or HTTP dependencies.

Source-specific interpretation remains in `src/sources/`. The query layer applies the generic source policy returned by that dispatcher, while presentation asks it how a stored message should be labeled and opened. Neither web submodule inspects raw Codex JSON.

The existing database-backed route tests exercise the boundaries together, including filtering, stable pagination, direct-home origin suppression, source-specific hidden models, bounded transcript sections, large-message parts, initial and resumed setup classification, ordinary conversation, authentication, and sanitized errors. Unit tests cover grouping and URL encoding directly. This refactor changes no routes, query parameters, schema, or parser semantics and requires no refresh or rebuild.
