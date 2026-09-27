#!/bin/bash
# Build a signed release .app, install it to /Applications and launch it.
source "$(dirname "$0")/lib.sh"
pnpm install --frozen-lockfile
pnpm tauri build --bundles app --config "$(signing_config)"
SRC="$ROOT/target/release/bundle/macos/$APP_NAME.app"
DEST="/Applications/$APP_NAME.app"
kill_app
rm -rf "$DEST"
ditto "$SRC" "$DEST"
open "$DEST"
echo "✅ Installed $DEST"
