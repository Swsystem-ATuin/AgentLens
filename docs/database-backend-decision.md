# Database backend reassessment

Status: retain SQLite for the current deployment model; reassessed on 2026-09-14.

## Decision

Agent Lens will continue using two local SQLite databases: a disposable, rebuildable observability index and a small persistent application-state store. PostgreSQL would add operational and migration cost without solving a current requirement. The decision is conditional, not permanent: multiple application instances, remote database storage, or sustained concurrent writers trigger a new assessment.

This follows SQLite's own selection guidance: SQLite is appropriate when application and data coexist on one machine, while a client/server database is preferred when many processes must write concurrently or the data is separated from the application by a network. [SQLite WAL](https://www.sqlite.org/wal.html) allows readers to continue while a writer appends, but still permits only one writer at a time. [SQLite's appropriate-use checklist](https://www.sqlite.org/whentouse.html) and [network-filesystem guidance](https://www.sqlite.org/useovernet.html) define the boundary used here.

PostgreSQL remains the preferred candidate if that boundary changes. Its [MVCC model](https://www.postgresql.org/docs/current/mvcc-intro.html) is designed so ordinary readers and writers do not conflict, making it a better fit for multiple application instances and materially concurrent mutation.

## Current workload

| Concern | Current evidence | Consequence |
| --- | --- | --- |
| Application topology | One Agent Lens process and local CLI processes on the same host | No network database or multi-instance coordination is required |
| Disposable index writes | Refresh/rebuild is the only writer; route handlers and Prometheus are read-only | WAL supports concurrent readers during a refresh; full rebuild publication deliberately requires the server to stop |
| Persistent-state writes | Account commands are rare; authenticated requests perform short server-side session updates | One serialized SQLite writer is acceptable at the current user count; the state database now explicitly uses WAL |
| Read consistency | Ingest uses transactions/savepoints; metrics use one read transaction; publication replaces only a validated staged index | Existing snapshot and rollback contracts already match the product's source-of-truth model |
| Scale evidence | Synthetic tests cover concurrent live reads during refresh, 100,000-session metric aggregation, bounded searches, and large streamed rebuilds | The observed pressure is aggregate scan/import cost, not competing writers or database-server availability |
| Operations | One binary, two owner-controlled files, no database service | PostgreSQL would add service provisioning, credentials, patching, backup, and restore procedures without a current availability requirement |

Both file-backed databases use WAL. The index enables it in its schema; the application state selects it when connecting so account/grant reads can coexist with short session writes. The pinned SQLx 0.8 connection options already use a five-second busy timeout, as documented by [`SqliteConnectOptions`](https://docs.rs/sqlx/0.8.6/sqlx/sqlite/struct.SqliteConnectOptions.html#method.busy_timeout). Database files and their WAL/shared-memory companions must remain on a local filesystem and be backed up only through the documented stopped-service procedure.

## Migration cost

The application is not currently database-portable by changing a connection URL. The index deliberately uses SQLite FTS5, `json_each`, `julianday`/`unixepoch`, `GLOB`, `instr`, `INSERT OR …`, PRAGMAs, row IDs, and file-based staged publication. PostgreSQL would require explicit replacements for search, date expressions, JSON grant expansion, conflict handling, migrations, integrity validation, and rebuild publication. The persistent session-store backend and its migration/backup path would also change.

Those changes are feasible, but implementing and maintaining two backends before a deployment needs PostgreSQL would expand every parser, query, authorization, rebuild, and recovery regression without user-visible benefit. A future migration should target PostgreSQL directly rather than introducing a lowest-common-denominator abstraction that weakens current SQLite behavior.

## Reconsideration triggers

Re-open this decision when any of the following is committed or measured:

- two or more Agent Lens application instances must share live state or an index;
- the database must reside on another host or a network filesystem;
- collectors, browser-owned annotations, or other features create sustained independent writers that cannot take short turns;
- representative load produces recurring `SQLITE_BUSY`, session-write latency, WAL checkpoint starvation, or unacceptable read latency after query/index tuning;
- high availability, managed replication, point-in-time recovery, or centralized database policy becomes a deployment requirement;
- the single-host database size or maintenance window exceeds measured operational limits and PostgreSQL testing shows a material improvement.

User count alone is not a trigger: read-heavy authenticated users do not create independent index writers. Conversely, even a small user count is enough to trigger reassessment if the topology requires multiple instances or remote storage.

## Required migration evidence

A PostgreSQL proposal must include a representative concurrent workload, measured SQLite failure or operational requirement, a mapping for every SQLite-specific feature, persistent-state migration and rollback, disposable-index rebuild semantics, authorization parity, search/result parity, backup/restore drills, and the full normal regression suite. Raw agent homes remain immutable sources of truth throughout; a backend migration must never turn the normalized index into the only copy of source data.

## Operational effect and verification

No index schema, import transaction, query result, command, or rebuild step changes. Restarting the application upgrades the persistent state database to WAL mode; this can create `-wal` and `-shm` companion files during operation. The normal stopped-service backup contract already accounts for them.

The automated state regression verifies WAL mode alongside account hashing and permissions. Existing tests continue to exercise concurrent committed refresh/read behavior, metrics snapshot consistency, staged rebuild publication, state migration, and rebuild-stable permission mappings. `bash scripts/check.sh` remains the acceptance command.
