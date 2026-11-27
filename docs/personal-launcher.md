# Personal usage launcher

On Linux and macOS, run these commands from a directory you can write to:

```bash
git clone --branch develop https://github.com/Swsystem-ATuin/AgentLens.git
cd AgentLens
bash scripts/local.sh
```

This workflow is intended for individuals reviewing their own agent activity,
including usage reporting for an employer-provided subscription. It finds the
current user's supported agent homes, builds AgentLens with Cargo, rebuilds an
isolated personal index, then serves the Usage page without account provisioning:
<http://127.0.0.1:3001/usage>. The initial build can take several minutes and
requires network access to download Rust dependencies. Later runs reuse compiled
artifacts. The launcher works from any working directory when invoked by its path.

## Windows

### Run the release ZIP

1. Download `AgentLens-windows-x64.zip` from a
   [GitHub release](https://github.com/Swsystem-ATuin/AgentLens/releases) that
   includes it. Extract the entire ZIP into a folder you can write to.
2. Double-click **Start AgentLens.cmd**. The window shows discovered agents and
   indexing progress, then opens <http://127.0.0.1:3001/usage> in your default
   browser. If your browser does not open, copy the URL from the window.
3. Press Ctrl-C in the window to stop the server. Start AgentLens again to include
   new activity. To update the application, stop it and extract the new ZIP into
   a fresh folder. The index and state stay under `%LOCALAPPDATA%\AgentLens`.

The ZIP runs as a standard user without developer tools. The same home and storage
overrides described below apply. For another port or a rebuild without a server,
open a terminal in the extracted folder:

```powershell
.\agent-lens.exe personal --port 3002
.\agent-lens.exe personal --index-only
```

Add `--no-browser` to print the address and serve without opening the browser.
Use `personal --refresh --index-only` to update an existing personal index.
On the first run this builds the index. A refresh retains previous records
when a source is missing or a newer recording cannot be read. For a fresh
inventory of the recordings currently available, run without `--refresh`.
On a managed PC, IT may need to approve the downloaded executable or launcher.

### Build from source

Use Windows PowerShell 5.1 or PowerShell 7. The source launcher runs as a standard
user. On a managed PC, ask IT about approved installers and script execution.

1. Install [Git for Windows](https://gitforwindows.org/).
2. Install [Visual Studio Build Tools](https://learn.microsoft.com/en-us/cpp/overview/acquire-msvc)
   with **Desktop development with C++**, the MSVC toolset, and a Windows SDK.
   The installer may require administrator approval.
3. Install the current stable MSVC toolchain using [rustup](https://rustup.rs/).
   Open a new PowerShell window and confirm `git --version`, `rustc -Vv`, and
   `cargo --version` work. The Rust host should end in `-pc-windows-msvc`.
4. Install OpenSSL for that toolchain with [Microsoft vcpkg](https://learn.microsoft.com/en-us/vcpkg/get_started/get-started).
   On x64 Windows, run:

   ```powershell
   Set-Location $env:USERPROFILE
   git clone https://github.com/microsoft/vcpkg.git
   Set-Location vcpkg
   .\bootstrap-vcpkg.bat -disableMetrics
   .\vcpkg.exe install openssl:x64-windows-static-md
   $env:VCPKG_ROOT = (Get-Location).Path
   ```

   Set `VCPKG_ROOT` again in any new PowerShell window used for builds. On other
   architectures, use the OpenSSL build and vcpkg triplet appropriate to the
   installed Rust target. For a separately provisioned OpenSSL install, set
   `OPENSSL_DIR` to its matching header and library prefix.

### Clone and launch on Windows

```powershell
Set-Location $env:USERPROFILE
git clone --branch develop https://github.com/Swsystem-ATuin/AgentLens.git
Set-Location AgentLens
.\scripts\local.ps1
```

The launcher prints a Cargo status message, the names of the sources it found,
the rebuild totals, and the URL. Open <http://127.0.0.1:3001/usage> in your
browser. Press Ctrl-C in the launcher terminal to stop it. Rerun the same command
to include new activity. To update AgentLens itself, stop it, run
`git pull --ff-only origin develop`, then rerun the launcher.

```powershell
.\scripts\local.ps1 -Port 3002
.\scripts\local.ps1 -IndexOnly
```

The discovery settings below apply on Windows using `%USERPROFILE%` as the
default home. Set overrides in the same PowerShell session before launching:

```powershell
$env:AGENT_LENS_CODEX_HOME = "$env:USERPROFILE\custom-codex"
.\scripts\local.ps1 -Port 3002
```

Codex discovery reads plain `.jsonl` and compressed `.jsonl.zst` recordings from
`sessions` and `archived_sessions`. The native launcher checks the user profile,
environment-provided app-data paths, and both Local and Roaming app data under
the host profile. It checks installed package directory names beginning with
`OpenAI.Codex_` or the ChatGPT desktop package identities. An explicit Codex
home override selects only that Codex root.

On Windows, the importer probes versioned `state_*.sqlite` and
`thread_history_*.sqlite` databases and the optional Codex SQLite catalogs.
It checks for supported `threads`, `thread_items`, and `thread_turns` schemas and
imports the available fields, including originator and archive metadata.
Rollouts supply the transcript where available. SQLite supplies missing
transcripts and can enrich rollout metadata for the same thread. An old
catalog reference ending in `.jsonl` still matches a discovered compressed
rollout by thread ID. Paths from catalogs do not extend discovery outside the
configured history roots. SQLite-only history supports ordinary text and
recognized turn lifecycle records. Tool and reasoning content in those
projections, token records without verified accounting, and unrecognized
schemas are skipped.

An active SQLite database is read through a consistent SQLite backup with
read-only source access and existing WAL/SHM files. An inactive database
can be copied with its journal into private temporary storage and recovered
there. Each database is snapshotted separately. If a coherent view is unavailable,
close Codex cleanly and rerun. No database sidecars are created in the source.

ChatGPT IndexedDB, local storage, and browser session storage are detected by
directory markers only. Browser session storage is site state. The launcher
does not open those databases or inspect browser credentials, and it does not
import ordinary ChatGPT chats or cloud-only Work conversations.

Windows stores its disposable index and local state under
`%LOCALAPPDATA%\AgentLens`. `AGENT_LENS_LOCAL_DIR` can select another dedicated
directory. The launcher passes the source directories to the rebuild command
directly. It creates no source links or source metadata. Use ordinary absolute
drive paths for source roots, storage, and build paths. This workflow rejects
junctions, symbolic links and UNC paths in those locations. Keep personal storage
and build output outside your agent homes.

The source launcher also accepts a compatible binary through
`$env:AGENT_LENS_BIN = 'C:\path\to\agent-lens.exe'`. This skips Cargo.

### Windows troubleshooting

| Symptom | What to check |
| --- | --- |
| No supported agent homes found | Record activity in a supported agent first, or configure the appropriate home override below. Explicit overrides must point to existing directories. |
| Cargo, a linker, or OpenSSL is missing | Check the toolchain, MSVC workload and SDK, matching OpenSSL architecture, and `VCPKG_ROOT` in the current shell. The first Cargo build also needs access to the configured Rust package registry. |
| The script or executable is blocked | Check your organisation's PowerShell signing rules, application allowlist, and network policy. Ask for an approved checkout, script, or binary where required. The launcher uses the existing policies. |
| Loopback port unavailable | Stop the process using it or select another with `personal --port 3002` or `-Port 3002` for the PowerShell launcher. |
| Personal storage is locked | Stop the launcher that is using the same storage. Windows releases the file lock when the launcher exits. The empty `launcher.lock` file can remain in place. |
| Hermes state import reports errors | Close Hermes cleanly and rerun. On Windows the state database must be available without a writer or uncheckpointed WAL so the importer can read it without creating or updating SQLite sidecars. Hermes request snapshots and cron output still use their ordinary read-only file imports. |

## Linux and macOS prerequisites

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

Missing default homes are skipped. An unreadable home or dangling home symlink stops the
launcher with an error. With no supported homes, it exits before building or
creating application storage. It only discovers these configured locations; it
does not recursively search your disks or other users' homes. An existing home
can contain no recorded sessions.

Default Linux/macOS personal storage:

- Linux: `${XDG_DATA_HOME:-$HOME/.local/share}/agent-lens`
- macOS: `~/Library/Application Support/AgentLens`

Linux/macOS storage contains `index.sqlite`, `state.sqlite`, and a `sources/local/current/`
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

On Linux or macOS, after a forced kill or power failure, a stale empty `launcher.lock` directory may
remain. Once you have confirmed the launcher and its server have stopped, remove
that empty directory with `rmdir /path/to/personal-agentlens/launcher.lock` and retry.

## Platform and adapter expansion

macOS storage selection is covered by a simulated-platform integration test;
native macOS execution still needs validation on a Mac. The regression suite
also runs the actual Linux launcher against synthetic homes, verifies repeated
rebuilds and source preservation, and starts/stops a real loopback usage server.

Additional adapters belong in the generic [source adapter layer](source-adapters.md).
As adapters are implemented and tested, add their home discovery to the launchers.
Both launchers use the same staged rebuild and local-server logic.
