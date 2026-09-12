# Safe rich transcript rendering

Agent Lens renders CommonMark with selected GitHub-flavored extensions in transcript sections and individual message-part pages. Supported extensions are tables, fenced code blocks with language classes, strikethrough, task lists, and GFM blockquote alerts. Math, heading attributes, metadata blocks, footnotes, superscript/subscript, and wiki links are not enabled.

Ordinary text that has no rich Markdown structure keeps the existing whitespace-preserving plain-text presentation. Consecutive assistant records still render as separate contents inside one grouped reply. Source-specific setup labels, collapsed state, interruption notices, user actions, transcript section navigation, and lazy continuation links are determined before Markdown rendering and are unchanged.

## Safety boundary

Stored transcript content is untrusted. Rendering applies these rules before generated HTML is marked safe for the server-side template:

- Raw block and inline HTML are emitted as escaped visible text, never interpreted as elements.
- Markdown image elements are replaced with readable `Image: alt text`; no `<img>` tag or source URL is emitted.
- Relative links and `http`, `https`, and `mailto` links are retained. Other explicit schemes, including `javascript`, `data`, and `file`, lose the anchor while retaining their readable label.
- Heading attributes and raw metadata extensions are disabled, so transcript text cannot supply arbitrary HTML attributes.
- Only the Markdown renderer's fixed element set reaches the template. All raw text and attribute values are escaped by the renderer.

Every authenticated and unauthenticated response also receives this Content Security Policy:

```text
default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'
```

This blocks scripts, images, media, frames, object loads, network connections, and other subresources even if a future rendering regression emits a loading element. The app has no client-side JavaScript. External links require an explicit user navigation and are not loaded while viewing a transcript.

Responses also use `Referrer-Policy: no-referrer`, preventing an explicit external-link navigation from disclosing the private local transcript URL in the HTTP referrer.

## Resource bounds and chunk behavior

Rich rendering operates only on content already bounded by the reader:

- A transcript section fetches at most 51 rows, renders 50, and supplies at most 2,048 SQLite characters from each message.
- An individual message page supplies at most 16,384 characters for one lazy part.
- Plain-text escaping expands output linearly, with a tested worst case of at most five output bytes per ASCII input byte plus a small fixed wrapper.

The Markdown parser streams borrowed events from each bounded input into a string. It does not fetch URLs or retain state across messages. A truncated preview or a later lazy message part is parsed as an independent fragment, so a Markdown construct split across a 2,048/16,384-character boundary may display as plain text or as separately balanced markup. The continuation link always exposes the next bounded part; the renderer never loads the complete oversized message to repair cross-part syntax.

Rendered regressions cover GFM tables, fenced code, setup messages before and after resume, ordinary whitespace-preserving prose, grouped replies, section boundaries, malicious raw HTML, unsafe link schemes, remote image syntax, and the existing 20,000-character lazy-message case. No SQLite schema or raw-source migration is required; restart the server after updating.
