# Release procedure

Agent Lens releases are cut from `develop` only after the intended roadmap scope is complete. `main` contains released commits, and annotated `vMAJOR.MINOR.PATCH` tags identify releases. Source and binary distributions are licensed under GPL-3.0-or-later and must include the license, corresponding source, preserved notices, and identification of modifications required by those terms. The project does not currently publish to crates.io.

## Prepare a release

1. Confirm `develop` is clean, synchronized with `origin/develop`, and contains only the intended release changes.
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

After reviewing the prepared commit, fast-forward `main`, create an annotated tag on that exact commit, and publish both references atomically:

```bash
git switch main
git merge --ff-only develop
git tag -a v0.1.0 -m "Agent Lens 0.1.0"
git push --atomic origin main v0.1.0
git switch develop
```

Replace `0.1.0` with the version being released. Verify that `main`, the tag, and the reviewed commit resolve to the same object. Do not move or reuse a published release tag. Release artifacts, if added later, must be built from that tag and accompanied by checksums.

## Upgrade to 0.1.0

This is the first tagged release, but existing development deployments may have older indexes or the retired shared-credential configuration.

1. Stop the Agent Lens server. Back up the persistent state database using the stopped-service procedure in [application authentication](application-authentication.md). The disposable index can always be rebuilt from raw sources.
2. Build or install the tagged binary with the lockfile (`cargo build --release --locked`).
3. Provision the first application account if the state database has no enabled account. The old `auth.local.json` and `--auth-file` interface are not supported.
4. Run one `refresh` against the existing copied sources. Schema upgrades are automatic, and parser version 7 reimports older Codex rollouts to populate current titles, ordered tool results, token summaries, turns, and pressure evidence. A full rebuild is optional unless the source layout or adapter changes.
5. Restart with the intended `--state-database`, project configuration, bind address, and `--secure-cookies` setting. Keep direct backend access restricted when using an HTTPS reverse proxy.
6. Confirm login, source grants, Source health, and a representative transcript before returning the service to users. Administrators must explicitly grant stable source IDs to viewer accounts.

Raw source homes remain immutable throughout the upgrade. Never place the persistent state database inside a copied agent home or discard it as part of an index rebuild.
