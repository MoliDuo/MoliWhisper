#!/bin/bash
# Set the version everywhere, commit, tag and push; CI (release.yml) builds
# and publishes the release from the tag.
#   scripts/release.sh 2.0.1        # release
#   scripts/release.sh 2.1.0-rc.1   # prerelease (any version with a "-")
#   scripts/release.sh 2.0.1 --no-push
source "$(dirname "$0")/lib.sh"

VERSION="${1:-}"
PUSH=1
[[ "${2:-}" == "--no-push" ]] && PUSH=0
if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; then
  echo "usage: $0 X.Y.Z[-pre] [--no-push]" >&2
  exit 1
fi
TAG="v$VERSION"

if [[ -n "$(git status --porcelain)" ]]; then
  echo "❌ The working tree has changes; commit or stash them first." >&2
  exit 1
fi
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  echo "❌ Tag $TAG already exists." >&2
  exit 1
fi

"$ROOT/scripts/set-version.sh" "$VERSION"

git add Cargo.toml Cargo.lock src-tauri/tauri.conf.json package.json
if git diff --cached --quiet; then
  echo "Version is already $VERSION."
else
  git commit -q -m "chore: release $VERSION"
fi
git tag -a "$TAG" -m "MoliWhisper $VERSION"
echo "✅ Tagged $TAG"

if [[ "$PUSH" == 1 ]]; then
  git push -q origin HEAD "$TAG"
  echo "🚀 Pushed; follow the build with: gh run watch \$(gh run list -w Release -L1 --json databaseId -q '.[0].databaseId')"
else
  echo "Push with: git push origin HEAD $TAG"
fi
