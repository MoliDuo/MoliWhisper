# Shared helpers for the scripts in this directory. Source it, don't run it.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

REPO="MoliDuo/MoliWhisper"
APP_NAME="MoliWhisper"
BUNDLE_ID="com.moliduo.moliwhisper"
# Lowercase on purpose: it never matches the legacy Swift app's "MoliWhisper" process.
BINARY_NAME="moliwhisper"

# Sign with the local Apple Development certificate so macOS keeps the
# Accessibility and microphone grants across rebuilds. Falls back to the
# ad-hoc signature from tauri.conf.json when no certificate is installed.
signing_config() {
  local sha
  sha="$(security find-identity -v -p codesigning 2>/dev/null | awk '/"Apple Development/ {print $2; exit}')"
  if [[ -n "$sha" ]]; then
    printf '{"bundle":{"macOS":{"signingIdentity":"%s"}}}' "$sha"
  else
    echo "⚠️  No Apple Development certificate; using an ad-hoc signature (permissions reset on every build)." >&2
    printf '{}'
  fi
}

kill_app() {
  pkill -x "$BINARY_NAME" 2>/dev/null && sleep 0.5 || true
}
