# Pull request

## Summary

<!-- Describe the concrete problem and resulting behavior. Link the issue if there is one.
Add the smallest useful example, diff sketch, or diagram when it clarifies the change. -->

## Evidence

- **Before:** <!-- Previous behavior; for a bug fix, show the regression test failing without the fix. -->
- **After:** <!-- Resulting behavior and regression verification. -->
- **Checks:** <!-- Results of bash scripts/check.sh. For docs, include command/link validation. Note any checks not run. -->

<!-- For UI/transcript changes, cover rendered HTML, initial/resumed sessions, and ordinary conversation.
For accessibility work, record manual keyboard/zoom/assistive-technology checks and environment.
For ingest changes, include before/after full-rebuild measurements using safe data.
All examples, screenshots, logs, and fixtures must be synthetic or fully redacted. -->

## Merge Danger

**Door:** <!-- Two-way (reversible) or one-way; explain any hard-to-reverse change. -->

**Blast Radius:** <!-- Affected users, agents, platforms, or data. -->

<!-- State migration, refresh, or rebuild requirements and any remaining risks or validation gaps. -->

- [ ] Targets `develop` and follows CONTRIBUTING.md.
- [ ] Adds appropriate regression verification; README and Unreleased changelog updated where needed.
- [ ] Contains no private agent artifacts, credentials, identifying data, local indexes, or build output.
