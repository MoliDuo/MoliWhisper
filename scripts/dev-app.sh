#!/bin/bash
# Build a signed debug .app and launch it. Use this for anything that needs
# real permissions: hotkeys, paste, microphone.
source "$(dirname "$0")/lib.sh"
pnpm install --frozen-lockfile
pnpm tauri build --debug --bundles app --config "$(signing_config)"
APP="$ROOT/target/debug/bundle/macos/$APP_NAME.app"
kill_app
open "$APP"
echo "✅ Launched $APP"
