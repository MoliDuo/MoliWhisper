#!/bin/bash
# Build and run in one step, with logs streaming
set -e
cd "$(dirname "$0")/.."

echo "🔨 Building..."
xcodebuild -project MoliWhisper.xcodeproj \
  -scheme MoliWhisper \
  -configuration Debug \
  -derivedDataPath build \
  CODE_SIGN_ALLOW_ENTITLEMENTS_MODIFICATION=YES \
  build \
  2>&1 | grep -E '(error:|warning:|BUILD SUCCEEDED|BUILD FAILED)'

APP="build/Build/Products/Debug/MoliWhisper.app"

# Kill existing instance
pkill -x "MoliWhisper" 2>/dev/null || true
sleep 0.5

echo ""
echo "🚀 Starting MoliWhisper..."
echo "---"
"$APP/Contents/MacOS/MoliWhisper" 2>&1
