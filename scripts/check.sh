#!/usr/bin/env bash
# Quality gate for local runs and CI.
# Regenerate help fixtures with the command in AGENTS.md, then re-run this script.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

step() { printf '\n==> %s\n' "$*"; }

step "Formatting"
cargo fmt --all -- --check

step "Clippy"
cargo clippy --locked --all-targets --all-features -- -D warnings

step "Tests"
cargo test --locked

step "Documentation"
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps

step "Help fixtures"
cargo build --locked --bin jev
bin="${CARGO_TARGET_DIR:-$root/target}/debug/jev"
check_help() {
  local name=$1
  shift
  local expected="$root/tests/fixtures/help/${name}.txt"
  local actual
  actual=$(mktemp)
  if ! "$bin" "$@" >"$actual"; then
    rm -f "$actual"
    printf 'jev %s failed\n' "$*" >&2
    exit 1
  fi
  if ! diff -u "$expected" "$actual"; then
    rm -f "$actual"
    exit 1
  fi
  rm -f "$actual"
}
check_help jev --help
check_help ask ask --help
check_help noul noul --help
check_help choice choice --help
check_help score score --help
check_help example example --help
check_help completions completions --help

printf '\ncheck: ok\n'
