#!/usr/bin/env bash
# Fast checks before a commit: format, clippy, tests. The full gate is scripts/check.sh.
set -euo pipefail

if [[ ${JEV_SKIP_HOOKS:-0} == 1 || ${SKIP:-0} == 1 ]]; then
  printf 'pre-commit: skipped\n'
  exit 0
fi

root=$(git rev-parse --show-toplevel)
cd "$root"

step() { printf '==> %s\n' "$*"; }

step "Formatting"
cargo fmt --all -- --check

step "Clippy"
cargo clippy --locked --all-targets --all-features -- -D warnings

step "Tests"
cargo test --locked

printf 'pre-commit: ok. Full gate: scripts/check.sh\n'
