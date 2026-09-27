#!/bin/bash
# Build the app
set -e
cd "$(dirname "$0")/.."

echo "🔨 Building MoliWhisper..."
xcodebuild -project MoliWhisper.xcodeproj \
  -scheme MoliWhisper \
  -configuration Debug \
  -derivedDataPath build \
  CODE_SIGN_ALLOW_ENTITLEMENTS_MODIFICATION=YES \
  build \
  2>&1 | grep -E '(error:|warning:|BUILD SUCCEEDED|BUILD FAILED|CompileSwift|Linking)'

echo ""
echo "✅ Build complete"
