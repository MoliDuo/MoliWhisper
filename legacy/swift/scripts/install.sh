#!/bin/bash
# Build a Release copy and install it to /Applications
set -eo pipefail
cd "$(dirname "$0")/.."

APP_NAME="MoliWhisper"
DEST="/Applications/$APP_NAME.app"

# Regenerate so project.yml changes (version, new files) are picked up
xcodegen generate --quiet

echo "🔨 Building Release..."
xcodebuild -project MoliWhisper.xcodeproj \
  -scheme MoliWhisper \
  -configuration Release \
  -derivedDataPath build \
  CODE_SIGN_ALLOW_ENTITLEMENTS_MODIFICATION=YES \
  build \
  2>&1 | grep -E '(error:|BUILD SUCCEEDED|BUILD FAILED)'

pkill -x "$APP_NAME" 2>/dev/null || true
sleep 0.5

echo "📦 Installing to $DEST"
rm -rf "$DEST"
ditto "build/Build/Products/Release/$APP_NAME.app" "$DEST"

open "$DEST"
echo "✅ Installed and launched"
