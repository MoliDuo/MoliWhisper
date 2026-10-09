#!/bin/bash
# Install the newest release from GitHub to
# /Applications and launch it. Downloading with gh leaves no quarantine flag,
# so Gatekeeper does not stop the first launch.
#   scripts/update.sh              # the newest release
#   scripts/update.sh v2.1.0       # a given one
source "$(dirname "$0")/lib.sh"

TAG="${1:-$(gh release list -R "$REPO" --exclude-drafts --limit 1 --json tagName --jq '.[0].tagName')}"
if [[ -z "$TAG" ]]; then
  echo "❌ No release found." >&2
  exit 1
fi

TMP="$(mktemp -d)"
trap 'hdiutil detach -quiet "$TMP/mnt" 2>/dev/null || true; rm -rf "$TMP"' EXIT
echo "⬇️  $TAG"
gh release download "$TAG" -R "$REPO" --pattern '*_macos_arm64.dmg' --dir "$TMP"
hdiutil attach -quiet -nobrowse -readonly -mountpoint "$TMP/mnt" "$TMP"/*.dmg

DEST="/Applications/$APP_NAME.app"
kill_app
rm -rf "$DEST"
ditto "$TMP/mnt/$APP_NAME.app" "$DEST"
open "$DEST"
echo "✅ Installed $TAG to $DEST"
