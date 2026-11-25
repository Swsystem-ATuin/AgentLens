# Personal usage launcher

After cloning the repository, run:

```bash
bash scripts/local.sh
```

This workflow is intended for individuals reviewing their own agent activity,
including usage reporting for an employer-provided subscription. It finds the
current user's supported agent homes, builds AgentLens with Cargo, rebuilds an
isolated personal index, then serves the Usage page without account provisioning:
<http://127.0.0.1:3001/usage>. The initial build can take several minutes and
requires network access to download Rust dependencies. Later runs reuse compiled
artifacts. The launcher works from any working directory when invoked by its path.

## Prerequisites

Linux and macOS need Bash, a current stable Rust toolchain with Cargo, a native C
compiler, OpenSSL headers/libraries, and pkg-config. Bash 3.2 syntax is supported
for the version bundled with macOS. No administrator access is needed to run the
launcher after installing build dependencies.

For Debian/Ubuntu native dependencies:

```bash
sudo apt-get update
sudo apt-get install build-essential pkg-config libssl-dev
```

For macOS, install Xcode Command Line Tools and use Homebrew for native dependencies:

```bash
xcode-select --install
brew install pkgconf openssl@3
```

Install Rust/Cargo through your preferred toolchain manager before running the
launcher. If OpenSSL is not discovered on macOS, pass its Homebrew prefix:

```bash
OPENSSL_DIR="$(brew --prefix openssl@3)" bash scripts/local.sh
```

Alternatively, point `AGENT_LENS_BIN` at an existing compatible executable to skip
the Cargo build. The launcher does not install dependencies or change your shell
configuration.

## Discovery and storage

| Adapter | Default home | Overrides, in priority order |
| --- | --- | --- |
| Codex | `~/.codex` | `AGENT_LENS_CODEX_HOME`, `CODEX_HOME` |
| Gemini CLI | `~/.gemini` | `AGENT_LENS_GEMINI_HOME` |
| Hermes | `~/.hermes` | `AGENT_LENS_HERMES_HOME`, `HERMES_HOME` |

Missing homes are skipped. An unreadable home or dangling home symlink stops the
launcher with an error. With no supported homes, it exits before building or
creating application storage. It only discovers these configured locations; it
does not recursively search your disks or other users' homes. An existing home
can contain no recorded sessions.

Default personal storage:

- Linux: `${XDG_DATA_HOME:-$HOME/.local/share}/agent-lens`
- macOS: `~/Library/Application Support/AgentLens`

Storage contains `index.sqlite`, `state.sqlite`, and a `sources/local/current/`
layout with symlinks to discovered homes. These stable links let the existing
multi-agent importer combine homes while preserving session identities between
runs. Source files are read in place, never copied, renamed, or edited; all
application metadata stays outside agent homes. New files use an owner-only
umask. The launcher refuses storage inside a discovered agent home and refuses
to replace real directories where it expects its own source links.

Custom paths must be absolute. For example:

```bash
AGENT_LENS_CODEX_HOME=/path/to/custom-codex \
AGENT_LENS_LOCAL_DIR=/path/to/personal-agentlens \
bash scripts/local.sh --port 3002
```

The launcher explicitly selects its personal database, state database, source
root, and loopback bind address, keeping those defaults separate from ordinary
`cargo run` and service installations. Other application environment options,
such as `AGENT_LENS_REDACTION_FILE` and `AGENT_LENS_PROJECTS_FILE`, still apply.
Use `bash scripts/local.sh --help` for launcher options.

## Updating activity and reviewing usage

Stop with Ctrl-C, then rerun the command to rebuild from current source files.
The script holds a per-storage launcher lock and stops its child server when the
launcher exits. It does not stop other AgentLens instances. A rebuild failure
prevents server startup and retains the previous usable index. If another process
has the personal index open, stop it before retrying. Rebuilds use the existing
[safe staged publication](rebuild.md) behavior.

For a rebuild without a server:

```bash
bash scripts/local.sh --index-only
```

For reporting, use the Usage page to select the period and group by agent, then review
session, model, and token totals and their completeness indicators. Existing
[redacted per-session postmortem exports](redacted-postmortems.md) provide
shareable aggregate evidence. A subscription's invoice, paid seat assignment,
and work value are not inferred from recorded token counts. Review the selected
report before sharing; the local index also contains private transcripts. This
launcher does not submit reports or upload agent content.

The web server binds only to `127.0.0.1`; anyone on the machine able to reach its
port can read the personal index. Use [authenticated deployment](application-authentication.md)
for shared access. Source recordings may be updated while an agent is running;
the existing tolerant parser handles partial records, and another launcher run
picks up later changes. There is no automatic periodic refresh in this workflow.

After a forced kill or power failure, a stale empty `launcher.lock` directory may
remain. Once you have confirmed the launcher and its server have stopped, remove
that empty directory with `rmdir /path/to/personal-agentlens/launcher.lock` and retry.

## Platform and adapter expansion

The launcher currently supports Linux and macOS; Windows is a future addition.
macOS storage selection is covered by a simulated-platform integration test;
native macOS execution still needs validation on a Mac. The regression suite
also runs the actual Linux launcher against synthetic homes, verifies repeated
rebuilds and source preservation, and starts/stops a real loopback usage server.

Additional adapters belong in the generic [source adapter layer](source-adapters.md).
As adapters are implemented and tested, add their home discovery to the launcher.
A future Windows launcher can reuse the same rebuild and local-server CLI rather
than introducing separate parsing or reporting logic. Existing ingest behavior
and full-rebuild transaction boundaries are unchanged by this script.
