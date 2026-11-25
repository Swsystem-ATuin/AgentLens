#!/usr/bin/env bash
# Personal launcher; compatible with the Bash 3.2 shipped by macOS.
set -euo pipefail
umask 077

usage() {
	cat <<'HELP'
Usage: bash scripts/local.sh [--port PORT] [--index-only]

Find this user's Codex, Gemini CLI and Hermes homes, rebuild a personal index,
and serve http://127.0.0.1:3001/usage without an account. Ctrl-C stops the server.
Use --index-only to rebuild without starting the server.

Optional environment overrides (absolute paths):
  AGENT_LENS_LOCAL_DIR    Personal index/state directory
  AGENT_LENS_BIN          Existing executable (otherwise build with Cargo)
  AGENT_LENS_CODEX_HOME   Codex home (defaults to CODEX_HOME or ~/.codex)
  AGENT_LENS_GEMINI_HOME  Gemini CLI home (defaults to ~/.gemini)
  AGENT_LENS_HERMES_HOME  Hermes home (defaults to HERMES_HOME or ~/.hermes)
HELP
}
fail() {
	echo "AgentLens: $*" >&2
	exit 1
}
port=3001
index_only=false
while [[ $# -gt 0 ]]; do
	case "$1" in
	--help | -h)
		usage
		exit 0
		;;
	--index-only)
		index_only=true
		shift
		;;
	--port)
		[[ $# -ge 2 ]] || fail '--port needs a value'
		port=$2
		shift 2
		;;
	*) fail "unknown option: $1 (use --help)" ;;
	esac
done
case "${port}" in '' | *[!0-9]*) fail 'port must be an integer from 1 to 65535' ;; *) ;; esac
[[ ${#port} -le 5 ]] || fail 'port must be an integer from 1 to 65535'
port=$((10#${port}))
[[ ${#port} -le 5 ]] && [[ ${port} -ge 1 ]] && [[ ${port} -le 65535 ]] || fail 'port must be an integer from 1 to 65535'
: "${HOME:?HOME must identify the current user home}"
case "$(uname -s)" in
Linux) default_storage="${XDG_DATA_HOME:-${HOME}/.local/share}/agent-lens" ;;
Darwin) default_storage="${HOME}/Library/Application Support/AgentLens" ;;
*) fail 'this launcher currently supports Linux and macOS' ;;
esac
storage=${AGENT_LENS_LOCAL_DIR:-${default_storage}}
case "${storage}" in /*) ;; *) fail 'AGENT_LENS_LOCAL_DIR must be an absolute path' ;; esac
case "${storage}/" in */../* | */./*) fail 'personal storage must not contain . or .. path components' ;; *) ;; esac

# Canonicalize the existing ancestor before mkdir, including symlinked parents.
ancestor=${storage}
suffix=
while [[ ! -e ${ancestor} ]]; do
	suffix="/$(basename -- "${ancestor}")${suffix}"
	ancestor=$(dirname -- "${ancestor}")
done
[[ -d ${ancestor} ]] || fail 'personal storage parent is not a directory'
storage="$(cd -- "${ancestor}" && pwd -P)${suffix}"

# Resolve sources before creating any application storage. Never write into a home.
source_names=()
source_paths=()
source_count=0
add_home() {
	local name=$1 candidate=$2 resolved
	case "${candidate}" in /*) ;; *) fail "${name} home must be an absolute path" ;; esac
	if [[ ! -e ${candidate} ]] && [[ ! -L ${candidate} ]]; then
		return
	fi
	[[ -d ${candidate} ]] && [[ -r ${candidate} ]] && [[ -x ${candidate} ]] || fail "${name} home is not a readable directory"
	resolved=$(cd -- "${candidate}" && pwd -P)
	case "${storage}/" in "${resolved}/"*) fail 'personal storage must be outside agent homes' ;; *) ;; esac
	source_names[source_count]=${name}
	source_paths[source_count]=${resolved}
	source_count=$((source_count + 1))
}
add_home .codex "${AGENT_LENS_CODEX_HOME:-${CODEX_HOME:-${HOME}/.codex}}"
add_home .gemini "${AGENT_LENS_GEMINI_HOME:-${HOME}/.gemini}"
add_home .hermes "${AGENT_LENS_HERMES_HOME:-${HERMES_HOME:-${HOME}/.hermes}}"
[[ ${source_count} -gt 0 ]] || fail 'no supported agent homes found; use --help for custom locations'

mkdir -p -- "${storage}"
storage=$(cd -- "${storage}" && pwd -P)
for source_path in "${source_paths[@]}"; do
	case "${storage}/" in "${source_path}/"*) fail 'personal storage must be outside agent homes' ;; *) ;; esac
done
# A directory lock works on Linux and macOS without requiring flock.
lock="${storage}/launcher.lock"
mkdir -- "${lock}" 2>/dev/null || fail "another launcher may be running; if it exited unexpectedly, remove the empty ${lock} directory before retrying"
server_pid=
# Called indirectly by the EXIT trap.
# shellcheck disable=SC2329
cleanup() {
	if [[ -n ${server_pid} ]]; then
		kill "${server_pid}" 2>/dev/null || true
		wait "${server_pid}" 2>/dev/null || true
	fi
	rmdir -- "${lock}"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)
if [[ -n ${AGENT_LENS_BIN:-} ]]; then
	case "${AGENT_LENS_BIN}" in /*) ;; *) fail 'AGENT_LENS_BIN must be an absolute path' ;; esac
	binary=${AGENT_LENS_BIN}
	[[ -x ${binary} ]] || fail 'AGENT_LENS_BIN is not executable'
else
	command -v cargo >/dev/null 2>&1 || fail 'install Rust/Cargo and native build dependencies listed in README.md, then retry'
	# Use a predictable target directory even if CARGO_TARGET_DIR is configured.
	cargo build --locked --manifest-path "${repo}/Cargo.toml" --target-dir "${repo}/target"
	binary="${repo}/target/debug/agent-lens"
fi

sources="${storage}/sources"
links="${sources}/local/current"
for directory in "${sources}" "${sources}/local" "${links}"; do
	[[ ! -L ${directory} ]] || fail 'launcher source directories must not be symlinks'
	mkdir -p -- "${directory}"
done
for name in .codex .gemini .hermes; do
	link="${links}/${name}"
	if [[ -e ${link} ]] && [[ ! -L ${link} ]]; then
		fail "expected a launcher-owned symlink at ${link}"
	fi
done
for name in .codex .gemini .hermes; do
	link="${links}/${name}"
	if [[ -L ${link} ]]; then
		unlink "${link}"
	fi
done
for ((i = 0; i < source_count; i++)); do
	ln -s -- "${source_paths[${i}]}" "${links}/${source_names[${i}]}"
	echo "Found ${source_names[${i}]}"
done

# Explicit paths and bind address keep this workflow separate from deployed services.
"${binary}" --database "${storage}/index.sqlite" --state-database "${storage}/state.sqlite" rebuild --data "${sources}"
if [[ ${index_only} == true ]]; then
	echo "Personal index rebuilt. Run this command without --index-only to view usage."
	exit 0
fi
echo "Open http://127.0.0.1:${port}/usage (Ctrl-C to stop; rerun to update usage)."
"${binary}" --database "${storage}/index.sqlite" --state-database "${storage}/state.sqlite" serve --local --bind "127.0.0.1:${port}" &
server_pid=$!
server_status=0
wait "${server_pid}" || server_status=$?
server_pid=
exit "${server_status}"
