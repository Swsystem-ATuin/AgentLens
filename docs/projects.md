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

The `From` and `Through` controls apply to the last-activity calendar date and are inclusive. `timezone` must be `UTC`, `Z`, or a fixed offset from `-14:00` through `+14:00`; the default is `+00:00`. RFC 3339 timestamps with offsets are converted to that fixed offset before their date is compared. Stored timestamps without an offset are interpreted by SQLite as UTC. Missing or unparseable activity timestamps remain visible without date filters and are excluded when either date boundary is active. Fixed offsets deliberately avoid claiming daylight-saving behavior; choose the offset whose calendar boundary you want for the viewed data.

Archive state comes from the rollout source area:

- `active`: a source under `sessions/` (and any future non-archive source kind)
- `archived`: a source under `archived_sessions/`
- `unknown`: a session without source-area metadata

Agent filters use the indexed generic agent kind. Every filter composes with host, user, model, status, project, archive state, and date boundaries. Pagination links retain all active filters. The page reads at most 51 session rows to render 50 and determine whether a next page exists; the rendered 105-session regression keeps each 50-row response below 64 KiB even with the added columns and controls. Project configuration is bounded, and an automatically created expression index supports stable activity/public-ID ordering.

The ordering query was also checked against a generated 100,000-session SQLite table with no private data. `EXPLAIN QUERY PLAN` selected the covering activity/public-ID index. A warm-cache 51-row first page took 0.5 ms, while an inclusive date-filtered 51-row page took 30.4 ms in that local run. These query-core timings exclude joins, filter-option queries, template rendering, network latency, and machine-to-machine variation; the automated row and response ceilings are the resource guarantees.
