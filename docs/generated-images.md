# Generated images

For Codex sessions, the transcript header links to a generated-images page. AgentLens reads supported image files directly from:

```text
<copied .codex home>/generated_images/<native-session-id>/
```

This works with the normal platform defaults (`%USERPROFILE%\.codex` on Windows and `$HOME/.codex` on macOS or Linux) as long as the entire agent home is included in the published snapshot. No refresh or rebuild is required when the image directory is already present; restart the server after upgrading AgentLens.

The list and individual image endpoints require application authentication and apply the same hidden-session and source-grant rules as the transcript. Requests are limited to regular `.png`, `.jpg`, `.jpeg`, or `.webp` files with single-component names, at most 64 MiB each. Symbolic-link files and directories, traversal components, other file types, and images outside the session-specific generated directory are not served. Responses retain the global `Cache-Control: no-store` policy.

Generated images remain immutable raw-source artifacts. They are not indexed in SQLite, renamed, moved, or written into the copied home. A missing directory produces an empty list. Files added to an already mounted snapshot become visible on the next request, although operators should continue publishing coherent immutable snapshots rather than modifying a live copy.

Workspace-relative image links recorded in Codex assistant Markdown, such as `outputs/example.png`, describe aliases in the original working directory and cannot safely identify an `exec-*.png` file after only the agent home is copied. AgentLens therefore redirects those relative image links to the session's **Generated images** gallery, where the canonical files retained in the copied Codex home can be opened. Absolute, remote, non-image, and non-Codex links are unchanged.
