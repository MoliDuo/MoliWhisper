#!/bin/bash
# Set the app version: `version` in package.json, the only place it is written
# (MoliSpec 006 §6.2.2). Then commit `chore(release): vX.Y.Z` and tag it; see
# AGENTS.md.
#   scripts/set-version.sh 2.1.0
source "$(dirname "$0")/lib.sh"

VERSION="${1:-}"
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "usage: $0 X.Y.Z" >&2
  exit 1
fi
export VERSION

node -e '
  const fs = require("node:fs");
  const pkg = JSON.parse(fs.readFileSync("package.json", "utf-8"));
  pkg.version = process.env.VERSION;
  fs.writeFileSync("package.json", JSON.stringify(pkg, null, 2) + "\n");
'
echo "package.json: $(app_version)"
