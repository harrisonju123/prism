#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASELINE_FILE="$REPO_ROOT/patches/zed/BASELINE"

baseline=$(cat "$BASELINE_FILE" | tr -d '[:space:]')

if ! git remote get-url zed-upstream &>/dev/null; then
  git remote add zed-upstream "https://github.com/harrisonju123/zed.git"
fi
git fetch zed-upstream main --no-tags --depth=1 2>/dev/null

new_sha=$(git rev-parse zed-upstream/main)

if [ "$baseline" = "$new_sha" ]; then
  echo "UP_TO_DATE=true" >> "${GITHUB_OUTPUT:-/dev/null}"
  echo "Already up to date at $baseline"
  exit 0
fi

changed=$(git diff --name-only "$baseline" "$new_sha" 2>/dev/null | wc -l | tr -d ' ')

echo "UP_TO_DATE=false" >> "${GITHUB_OUTPUT:-/dev/null}"
echo "BASELINE=$baseline" >> "${GITHUB_OUTPUT:-/dev/null}"
echo "NEW_SHA=$new_sha" >> "${GITHUB_OUTPUT:-/dev/null}"
echo "CHANGED_FILES=$changed" >> "${GITHUB_OUTPUT:-/dev/null}"

echo "Upstream has moved: $baseline → $new_sha ($changed files changed)"
git diff --name-only "$baseline" "$new_sha" 2>/dev/null | sed 's|^|  zed-upstream/|'
