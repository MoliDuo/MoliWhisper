# Shared helpers for the scripts in this directory. Source it, don't run it.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

REPO="MoliDuo/MoliWhisper"
APP_NAME="MoliWhisper"
BUNDLE_ID="com.moliduo.moliwhisper"
BINARY_NAME="moliwhisper"

# Sign with the shared Moli self-signed certificate (installed in the login
# keychain) so macOS keeps the Accessibility and microphone grants across
# rebuilds. Self-signed, so find-identity runs without -v. Falls back to the
# ad-hoc signature from tauri.conf.json when no certificate is installed.
signing_config() {
  local sha
  sha="$(security find-identity -p codesigning 2>/dev/null | awk '/"Moli Self-Signed Code Signing"/ {print $2; exit}')"
  if [[ -n "$sha" ]]; then
    printf '{"bundle":{"macOS":{"signingIdentity":"%s"}}}' "$sha"
  else
    echo "⚠️  No Moli Self-Signed Code Signing certificate; using an ad-hoc signature (permissions reset on every build)." >&2
    printf '{}'
  fi
}

# The app version: `version` in package.json, the one place it is written
# (MoliSpec 006 §6.2.2); tauri.conf.json reads it from there.
app_version() {
  node -p 'require("./package.json").version'
}

kill_app() {
  pkill -x "$BINARY_NAME" 2>/dev/null && sleep 0.5 || true
}
