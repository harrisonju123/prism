#!/usr/bin/env bash
# sync-zed-upstream.sh — Pull new Zed upstream changes into zed-upstream/
#
# Usage: ./scripts/sync-zed-upstream.sh [--dry-run]
#
# What it does:
#   1. Fetches the zed-upstream remote (harrisonju123/zed)
#   2. Computes the diff between the stored baseline and the new remote tip
#   3. Applies that diff into zed-upstream/ using a 3-way merge
#   4. Lists PrisM patches that must be re-verified for conflicts
#
# After running:
#   - Resolve any conflict markers in zed-upstream/
#   - cargo check -p zed
#   - Update patches/zed/BASELINE to the new SHA
#   - Regenerate patches: git format-patch <old-baseline>..HEAD \
#       --output-directory patches/zed/ -- zed-upstream/
#   - git add zed-upstream/ patches/zed/ && git commit -m "chore(zed): sync upstream to <sha>"

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BASELINE_FILE="$REPO_ROOT/patches/zed/BASELINE"
DRY_RUN=false

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=true ;;
    *) echo "Unknown argument: $arg"; exit 1 ;;
  esac
done

cd "$REPO_ROOT"

if [ ! -f "$BASELINE_FILE" ]; then
  echo "ERROR: patches/zed/BASELINE not found. Cannot determine upstream baseline."
  exit 1
fi

BASELINE=$(git rev-parse "$(cat "$BASELINE_FILE" | tr -d '[:space:]')")

echo "==> Fetching zed-upstream remote..."
git fetch zed-upstream

NEW_SHA=$(git rev-parse zed-upstream/main)

if [ "$BASELINE" = "$NEW_SHA" ]; then
  echo "Already up to date ($BASELINE)."
  exit 0
fi

echo ""
echo "Upstream changed: $BASELINE → $NEW_SHA"
echo ""
echo "Files changed upstream (in zed-upstream/):"
git diff --name-only "$BASELINE" "$NEW_SHA" | sed 's|^|  zed-upstream/|'

CHANGED_COUNT=$(git diff --name-only "$BASELINE" "$NEW_SHA" | wc -l | tr -d ' ')
echo ""
echo "$CHANGED_COUNT file(s) changed upstream."

if $DRY_RUN; then
  echo ""
  echo "[dry-run] Skipping apply. Exiting."
  echo ""
  echo "PrisM patches to re-verify after a real sync:"
  for p in "$REPO_ROOT/patches/zed/0"*.patch; do
    echo "  $p"
  done
  exit 0
fi

echo ""
echo "==> Applying upstream delta to zed-upstream/ (3-way merge)..."
git diff "$BASELINE" "$NEW_SHA" | git apply --directory=zed-upstream/ --3way || {
  echo ""
  echo "WARNING: Some hunks did not apply cleanly. Conflict markers may be present."
  echo "Resolve conflicts in zed-upstream/, then continue."
}

echo ""
echo "==> PrisM patches to re-verify (check for conflicts or redundancy):"
for p in "$REPO_ROOT/patches/zed/0"*.patch; do
  echo "  $p"
done

echo ""
echo "==> Next steps:"
echo "  1. Resolve any conflicts in zed-upstream/"
echo "  2. cargo check -p zed"
echo "  3. echo '$NEW_SHA' > patches/zed/BASELINE"
echo "  4. Regenerate patches:"
echo "       git format-patch $BASELINE..HEAD \\"
echo "         --output-directory patches/zed/ -- zed-upstream/"
echo "  5. git add zed-upstream/ patches/zed/ && git commit -m 'chore(zed): sync upstream to ${NEW_SHA:0:7}'"
