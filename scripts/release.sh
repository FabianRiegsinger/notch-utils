#!/bin/bash
# Release: scripts/release.sh 0.2.0
# Sets the version in Cargo.toml, commits, tags v0.2.0 and pushes. CI then builds macOS,
# Windows and Linux and publishes a GitHub Release with the packages attached.
# A suffix (0.2.0-beta.1) makes a pre-release.
set -euo pipefail
cd "$(dirname "$0")/.."

v=${1:?usage: scripts/release.sh <version, e.g. 0.2.0>}
v=${v#v}
[[ $v =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] || { echo "not a version: $v" >&2; exit 1; }
[ "$(git branch --show-current)" = main ] || { echo "release from main" >&2; exit 1; }
[ -z "$(git status --porcelain --untracked-files=no)" ] || { echo "commit or stash your changes first" >&2; exit 1; }
git fetch -q origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] || { echo "main is not in sync with origin/main" >&2; exit 1; }
git rev-parse -q --verify "refs/tags/v$v" >/dev/null && { echo "tag v$v exists" >&2; exit 1; }

# First `version = ` line is the package's.
sed -i.bak "1,/^version = /s/^version = \".*\"/version = \"$v\"/" Cargo.toml && rm Cargo.toml.bak
cargo check -q  # updates the version in Cargo.lock
git add Cargo.toml Cargo.lock
git commit -q -m "release: v$v"
git tag -a "v$v" -m "notch-utils $v"
git push -q origin main "v$v"
echo "Pushed v$v. Release appears at: $(gh repo view --json url -q .url 2>/dev/null || echo '<repo>')/releases"
