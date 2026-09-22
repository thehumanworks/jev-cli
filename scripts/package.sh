#!/usr/bin/env bash
# Package a release build of jev into dist/jev-<version>-<target>.tar.gz with a .sha256 beside it.
#
# Usage: scripts/package.sh <target> <version>
# Expects `cargo build --release --target <target>` to have run. Prints the archive path.
# Kept to bash 3.2 so it runs on the stock macOS shell.
set -euo pipefail
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

if [[ $# -ne 2 ]]; then
  echo "usage: $0 <target> <version>" >&2
  exit 2
fi
target=$1
version=$2
name="jev-${version}-${target}"
bin="${CARGO_TARGET_DIR:-$root/target}/${target}/release/jev"

if [[ ! -x $bin ]]; then
  echo "no release binary at $bin" >&2
  exit 1
fi

stage="$root/dist/$name"
rm -rf "$stage"
mkdir -p "$stage"
cp "$bin" README.md CHANGELOG.md LICENSE-MIT LICENSE-APACHE "$stage/"
tar -C "$root/dist" -czf "$root/dist/$name.tar.gz" "$name"
rm -rf "$stage"

cd "$root/dist"
if command -v sha256sum >/dev/null; then
  sha256sum "$name.tar.gz" >"$name.tar.gz.sha256"
else
  shasum -a 256 "$name.tar.gz" >"$name.tar.gz.sha256"
fi
echo "dist/$name.tar.gz"
