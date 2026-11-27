# Release procedure

Agent Lens releases are cut from `develop` only after the intended roadmap scope is complete. `main` contains released commits, and annotated `vMAJOR.MINOR.PATCH` tags identify releases. Source and binary distributions are licensed under GPL-3.0-or-later and must include the license, corresponding source, preserved notices, and identification of modifications required by those terms. The project does not currently publish to crates.io.

## Prepare a release

1. Confirm `develop` is clean, synchronized with both `origin/develop` and `s4w/develop`, and contains only the intended release changes.
2. Confirm the version in `Cargo.toml` and `Cargo.lock` matches the intended tag without the leading `v`.
3. Move the accumulated changelog entries from `Unreleased` beneath a dated version heading, leaving a new empty `Unreleased` section for later work.
4. Review the upgrade notes below and update them whenever schema, parser, authentication, or deployment behavior changes.
5. Run the acceptance commands from the repository root:

   ```bash
   bash scripts/check.sh
   cargo package --locked
   cargo build --release --locked
   target/release/agent-lens --help
   ```

   `cargo package` verifies that a clean source archive builds without relying on untracked repository files. Build and test on every supported target before describing that target as supported.

## Publish a release

After reviewing the prepared commit, fast-forward `main`. A commit with the
subject `Release AgentLens vMAJOR.MINOR.PATCH` explicitly requests publication
when pushed to `main` on GitHub. The version must match the manifest. Once the
release checks pass, the workflow creates the annotated tag at that commit
and publishes the Windows ZIP and checksum. Fetch the tag from GitHub before
publishing the same refs to the private mirror.

For a manually tagged release, create the annotated tag locally and push the
refs together:

```bash
git switch main
git merge --ff-only develop
git tag -a v0.2.0 -m "Agent Lens 0.2.0"
git push --atomic origin develop main v0.2.0
git push --atomic s4w develop main v0.2.0
git switch develop
```

Replace `0.2.0` with the version being released. Verify that `develop`, `main`, the peeled tag, and the reviewed commit resolve to the same commit on both remotes. Do not move or reuse a published release tag.

The **Windows package** workflow runs on the standard GitHub-hosted Windows x64
runner. Runs on `develop` and manual runs produce a preview ZIP and checksum
under Actions. Download the ZIP from a successful preview and smoke-test it on a
Windows machine without developer tools before creating the first public Windows
release. Check that the double-click launcher opens the dashboard and Ctrl-C
stops it.

Pushing a `vMAJOR.MINOR.PATCH` tag to the public GitHub repository starts the
release build. The workflow checks that the executable version matches the tag,
runs the native Windows regression tests, and rebuilds a synthetic index with
the executable extracted from the ZIP. After those checks pass, it creates the
GitHub Release with `AgentLens-windows-x64.zip` and its SHA-256 checksum. Leave
release creation to the workflow for those tags. Retain the matching tag as the
source for the package. The private mirror does not participate in this workflow.

## Upgrade to 0.3.0

- On Windows x64, extract the release ZIP and run **Start AgentLens.cmd**. The
  launcher rebuilds the personal index and opens the local Usage dashboard.
  Stop any previous launcher before updating. Personal storage is retained.
- Rerun the personal launcher or run a deployment refresh to discover compressed
  Codex rollouts. Existing deployment indexes can be kept.

## Upgrade to 0.2.0

1. Stop the server and back up the persistent state database as described in [application authentication](application-authentication.md). Keep copied raw sources unchanged.
2. Build the tagged source with `cargo build --release --locked` and retain the existing state database and deployment configuration.
3. Run one `refresh` to apply automatic schema upgrades and reimport sources affected by parser-version changes, including Hermes cron output classification and Gemini context handling. Routine upgrades do not require a full index rebuild.
4. If enabling or changing pre-persistence masking policies, follow the staged rebuild requirements in [import masking](import-masking.md). Switching source layouts or adapters also requires a full rebuild.
5. Restart the server and verify login, source access, Sessions dropdown filtering, and representative transcripts. OIDC production routes remain disabled in this release; existing local account authentication continues to apply.

## Upgrade to 0.1.0

This is the first tagged release, but existing development deployments may have older indexes or the retired shared-credential configuration.

1. Stop the Agent Lens server. Back up the persistent state database using the stopped-service procedure in [application authentication](application-authentication.md). The disposable index can always be rebuilt from raw sources.
2. Build or install the tagged binary with the lockfile (`cargo build --release --locked`).
3. Provision the first application account if the state database has no enabled account. The old `auth.local.json` and `--auth-file` interface are not supported.
4. Run one `refresh` against the existing copied sources. Schema upgrades are automatic, and parser version 7 reimports older Codex rollouts to populate current titles, ordered tool results, token summaries, turns, and pressure evidence. A full rebuild is optional unless the source layout or adapter changes.
5. Restart with the intended `--state-database`, project configuration, bind address, and `--secure-cookies` setting. Keep direct backend access restricted when using an HTTPS reverse proxy.
6. Confirm login, source grants, Source health, and a representative transcript before returning the service to users. Administrators must explicitly grant stable source IDs to viewer accounts.

Raw source homes remain immutable throughout the upgrade. Never place the persistent state database inside a copied agent home or discard it as part of an index rebuild.
