# Project navigation and session filters

Project grouping is an optional, read-only web-server configuration. It does not annotate the SQLite index or write into copied agent homes. Start from [`projects.example.json`](../projects.example.json):

```json
{
  "timezone": "+00:00",
  "projects": [
    {
      "name": "Example project",
      "host": "example-host",
      "root": "/srv/example-project",
      "subprojects": [
        { "name": "Web", "path": "apps/web" }
      ]
    }
  ]
}
```

Run the server with `--projects-file projects.local.json` or set `AGENT_LENS_PROJECTS_FILE`. Without a file, sessions remain unassigned and all other filters continue to work. Configuration is loaded at startup, so edits require a server restart but no index refresh or rebuild.

## Path matching

- `host` must exactly match the indexed host. Use an empty string for a direct agent-home index, whose host is intentionally empty.
- `root` must be an absolute POSIX path or an absolute drive-letter Windows path. Windows drive paths accept either slash style and compare ASCII case-insensitively; POSIX paths are case-sensitive.
- A subproject path is relative to its parent root. Its displayed and filtered label is `Project / Subproject`.
- Matching uses directory-component boundaries, so `/work/repo` does not match `/work/repository`. When configured roots overlap, the longest matching path wins.
- Configuration permits at most 64 combined project and subproject rules, 120 characters per name, and 4,096 characters per path. Ambiguous labels for one host/path are rejected at startup.
- Missing, relative, or unmatched working directories remain visible with no project assignment.

Project names are exact filter values. Selecting a subproject selects that most-specific group; selecting its parent selects sessions assigned to the parent rather than automatically including every subproject.

## Activity, dates, and archive state

“Last activity” is the last timestamp observed in source-record order, falling back to the session start timestamp. It is evidence from the copied rollout, not a live-process or freshness claim. Sessions sort by this timestamp descending and then by the stable public session ID descending, so ties remain deterministic across rebuilds. Missing timestamps sort last.

The `From` and `Through` controls apply to the last-activity calendar date and are inclusive. The signed-in account's timezone preference controls the boundary: UTC, browser-local, or a validated IANA timezone with historical daylight-saving transitions. Stored timestamps without an offset are interpreted by SQLite as UTC. Missing or unparseable activity timestamps remain visible without date filters and are excluded when either date boundary is active. Browser-local mode falls back to UTC when scripting or timezone discovery is unavailable; see [user preferences](user-preferences.md).

The top-level project-file `timezone` remains accepted for configuration compatibility and must be `UTC`, `Z`, or a fixed offset from `-14:00` through `+14:00`. Account preferences supersede it in web views.

Archive state comes from the rollout source area:

- `active`: a source under `sessions/` (and any future non-archive source kind)
- `archived`: a source under `archived_sessions/`
- `unknown`: a session without source-area metadata

Agent filters use the indexed generic agent kind. Every filter composes with host, user, model, status, project, archive state, and date boundaries. Pagination links retain all active filters. The page reads at most the account's selected 25, 50, or 100 rows plus one to determine whether a next page exists; a synthetic regression verifies the 25-row case and a 256 KiB response ceiling. Project configuration is bounded, and an automatically created expression index supports stable activity/public-ID ordering.

The ordering query was also checked against a generated 100,000-session SQLite table with no private data. `EXPLAIN QUERY PLAN` selected the covering activity/public-ID index. A warm-cache 51-row first page took 0.5 ms, while an inclusive date-filtered 51-row page took 30.4 ms in that local run. These query-core timings exclude joins, filter-option queries, template rendering, network latency, and machine-to-machine variation; the automated row and response ceilings are the resource guarantees.
