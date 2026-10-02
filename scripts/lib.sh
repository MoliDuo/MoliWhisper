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

# The version of a build of HEAD: X.Y from Cargo.toml, the patch number is the
# commit count, so every push to master is newer than the one before. CI
# (release.yml) and install.sh both use it, so a local build of a pushed
# commit is not offered its own release as an update.
build_version() {
  local base
  base="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml")"
  base="${base%%-*}"
  echo "${base%.*}.$(git -C "$ROOT" rev-list --count HEAD)"
}

kill_app() {
  pkill -x "$BINARY_NAME" 2>/dev/null && sleep 0.5 || true
}
