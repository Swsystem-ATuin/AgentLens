#!/usr/bin/env bash
# Run from any directory; stop as soon as a required check fails.
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
cargo fmt --check
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
bash scripts/verify-packaging.sh
trunk check --all --no-fix --ci
git diff --check
