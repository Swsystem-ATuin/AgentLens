# Generated images

For Codex sessions, the transcript header links to a generated-images page. AgentLens reads supported image files directly from:

```text
<copied .codex home>/generated_images/<native-session-id>/
```

Directly resolved transcript images appear in a separate preview grid after the complete assistant message. Each card places a bounded, lazy-loaded thumbnail first and its named full-size link underneath. The generated-images page provides the same thumbnail treatment for every retained file. Thumbnail display uses the authenticated image endpoint and permits only same-origin images through the Content Security Policy; it does not contact an external image host.

This works with the normal platform defaults (`%USERPROFILE%\.codex` on Windows and `$HOME/.codex` on macOS or Linux) as long as the entire agent home is included in the published snapshot. No refresh or rebuild is required when the image directory is already present; restart the server after upgrading AgentLens.

The list and individual image endpoints require application authentication and apply the same hidden-session and source-grant rules as the transcript. Requests are limited to regular `.png`, `.jpg`, `.jpeg`, or `.webp` files with single-component names, at most 64 MiB each. Symbolic-link files and directories, traversal components, other file types, and images outside the session-specific generated directory are not served. Responses retain the global `Cache-Control: no-store` policy.

Generated images remain immutable raw-source artifacts. They are not indexed in SQLite, renamed, moved, or written into the copied home. A missing directory produces an empty list. Files added to an already mounted snapshot become visible on the next request, although operators should continue publishing coherent immutable snapshots rather than modifying a live copy.

Some Codex desktop sessions display generated media natively and record no Markdown link or workspace-copy command in the assistant message. AgentLens shows those otherwise unlinked files in a **Generated images from this session** section after the visible transcript. Files already resolved to a transcript preview are excluded from that fallback section to avoid duplicates.

Workspace-relative image links recorded in Codex assistant Markdown, such as `outputs/example.png`, describe aliases in the original working directory. When the session contains an observed PowerShell `Copy-Item` or POSIX `cp` command from its own generated-image directory to that relative alias, AgentLens validates both paths and rewrites the link directly to the retained generated file. An image alias without such evidence falls back to the session's **Generated images** gallery. Absolute, remote, non-image, cross-session, escaping, and non-Codex links are unchanged.
