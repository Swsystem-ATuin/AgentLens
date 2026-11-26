# Accessibility

AgentLens aims to make agent activity, usage, and evidence usable by people with
different abilities and assistive technologies. Accessibility applies to the web
interface, documentation, command-line workflows, and contribution process.

## Current status and goals

The web UI uses native HTML controls, headings, table headers, labelled navigation,
and expandable transcript sections. These provide a foundation for accessible
interaction, but are not proof of conformance. We aim toward WCAG 2.2 Level AA;
no formal accessibility audit or comprehensive assistive-technology test matrix
has been completed, and no conformance claim is made.

Areas needing validation include keyboard navigation and focus visibility,
screen-reader interpretation of long transcripts and tables, contrast, browser
zoom and narrow layouts, live-update announcements, and non-text alternatives
for generated images. Transcript content originates with agents and may itself
contain inaccessible tables, code, or image descriptions. Linux/macOS launcher
coverage does not establish browser or screen-reader accessibility.

## Report a barrier

Use the [accessibility issue form](https://github.com/Swsystem-ATuin/AgentLens/issues/new?template=accessibility.yml).
Describe the task you were trying to complete, where the barrier occurs, expected
behavior, and what happened. Browser/OS versions and assistive-technology details
are helpful when you are comfortable sharing them; disability or medical
information is never required. A synthetic example is enough. Do not include real
transcripts, credentials, identifying paths, or unredacted screenshots.

If an issue form itself is inaccessible, use a
[blank issue](https://github.com/Swsystem-ATuin/AgentLens/issues/new). For a concern
requiring confidential handling, request a private contact without posting
sensitive details, following the [code of conduct reporting process](CODE_OF_CONDUCT.md#reporting-concerns).
Use [SECURITY.md](SECURITY.md) for vulnerabilities.

Maintainers review reports as part of normal issue triage and can discuss
alternative ways to provide a reproduction. There is no dedicated accessibility
support team or guaranteed resolution timeline.

## Contribute and verify

Use semantic elements and native controls where possible. Give controls meaningful
accessible names, preserve keyboard operation and visible focus, and avoid
communicating important state by color alone. Keep messages readable when zoomed
and offer understandable alternatives for non-text content. Live updates should
not unexpectedly move focus or prevent reading.

For relevant UI changes, manually check keyboard-only interaction, zoom, and a
screen reader where available, alongside automated regressions. Record the tested
browser, operating system, assistive technology, and limitations in the pull
request. Rendered-HTML assertions alone do not establish accessibility. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the normal checks and privacy requirements.

Maintainers should update this page as barriers are verified or resolved and
record notable accessibility changes in the changelog.
