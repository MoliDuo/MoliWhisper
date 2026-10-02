#!/bin/bash
# Set the app version in Cargo.toml, tauri.conf.json and package.json.
# CI sets the build version with it; bump X.Y here by hand, e.g.
#   scripts/set-version.sh 2.1.0
source "$(dirname "$0")/lib.sh"

VERSION="${1:-}"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; then
  echo "usage: $0 X.Y.Z[-pre]" >&2
  exit 1
fi
export VERSION

# Cargo.toml: only the version under [workspace.package].
perl -pi -e 'if (/^\[workspace\.package\]/ ... /^\[/) { s/^version = ".*"/version = "$ENV{VERSION}"/ }' Cargo.toml
# The top-level "version" is the first one in both files.
perl -0pi -e 's/"version": "[^"]*"/"version": "$ENV{VERSION}"/' src-tauri/tauri.conf.json package.json
cargo update --workspace --quiet
