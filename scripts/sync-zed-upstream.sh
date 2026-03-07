#!/usr/bin/env bash
# sync-zed-upstream.sh — Pull new Zed upstream changes into zed-upstream/
#
# Usage: ./scripts/sync-zed-upstream.sh [--dry-run] [--resume] [--abort] [--auto-baseline]
#
# Flags:
#   --dry-run         Show what would change without applying
#   --resume          Continue an interrupted sync (skip fetch+apply, run verification)
#   --abort           Roll back an interrupted sync and remove the lock file
#   --auto-baseline   Automatically update patches/zed/BASELINE after a clean apply
#
# What it does:
#   1. Checks for an interrupted sync (lock file)
#   2. Fetches the zed-upstream remote (harrisonju123/zed)
#   3. Stashes zed-upstream/ for rollback safety
#   4. Applies the upstream delta via 3-way merge
#   5. Reports conflict markers, Cargo drift, and per-patch risk
#
# After running:
#   - Resolve any conflict markers in zed-upstream/
#   - cargo check -p zed
#   - Update patches/zed/BASELINE to the new SHA (or use --auto-baseline)
#   - Regenerate patches: git format-patch <old-baseline>..HEAD \
#       --output-directory patches/zed/ -- zed-upstream/
#   - git add zed-upstream/ patches/zed/ && git commit -m "chore(zed): sync upstream to <sha>"

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BASELINE_FILE="$REPO_ROOT/patches/zed/BASELINE"
LOCKFILE="$REPO_ROOT/patches/zed/.sync-in-progress"
RECONCILE="$SCRIPT_DIR/reconcile-cargo.sh"

DRY_RUN=false
RESUME=false
ABORT=false
AUTO_BASELINE=false

for arg in "$@"; do
  case "$arg" in
    --dry-run)       DRY_RUN=true ;;
    --resume)        RESUME=true ;;
    --abort)         ABORT=true ;;
    --auto-baseline) AUTO_BASELINE=true ;;
    *) echo "Unknown argument: $arg"; exit 1 ;;
  esac
done

cd "$REPO_ROOT"

# --- Idempotency guard ---
if $ABORT; then
  if [ ! -f "$LOCKFILE" ]; then
    echo "No sync in progress (lockfile not found)."
    exit 0
  fi
  LOCKED_SHA=$(cat "$LOCKFILE")
  echo "==> Aborting interrupted sync (target was $LOCKED_SHA)..."
  git checkout -- zed-upstream/ 2>/dev/null && echo "  Restored zed-upstream/ from git." || \
    echo "  NOTE: git checkout had nothing to restore."
  rm -f "$LOCKFILE"
  echo "Lockfile removed. Sync aborted."
  exit 0
fi

if [ -f "$LOCKFILE" ] && ! $RESUME; then
  LOCKED_SHA=$(cat "$LOCKFILE")
  echo ""
  echo "WARNING: A sync was already started (target SHA: $LOCKED_SHA)."
  echo "The sync did not complete cleanly. Options:"
  echo "  --resume  Skip fetch+apply, run verification on current zed-upstream/ state"
  echo "  --abort   Roll back zed-upstream/ to git HEAD and remove lockfile"
  echo ""
  exit 1
fi

if [ ! -f "$BASELINE_FILE" ]; then
  echo "ERROR: patches/zed/BASELINE not found. Cannot determine upstream baseline."
  exit 1
fi

BASELINE=$(git rev-parse "$(cat "$BASELINE_FILE" | tr -d '[:space:]')")

# --- Resume path: skip fetch+apply, go straight to verification ---
if $RESUME; then
  if [ ! -f "$LOCKFILE" ]; then
    echo "No interrupted sync found (lockfile absent). Nothing to resume."
    exit 0
  fi
  NEW_SHA=$(cat "$LOCKFILE")
  echo "==> Resuming sync (target: $NEW_SHA). Running post-apply verification..."
  echo ""
  run_post_apply_verification() { :; }  # defined below
else
  # --- Normal path: fetch + apply ---
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

  # Write lockfile before modifying anything
  echo "$NEW_SHA" > "$LOCKFILE"

  # Stash zed-upstream/ for rollback
  STASH_MSG="pre-zed-sync-$(date +%s)"
  if git stash push -m "$STASH_MSG" -- zed-upstream/ 2>/dev/null; then
    STASHED=true
    echo "==> Stashed zed-upstream/ as '$STASH_MSG' (rollback available)."
  else
    STASHED=false
    echo "==> No changes to stash (zed-upstream/ is clean)."
  fi

  echo ""
  echo "==> Applying upstream delta to zed-upstream/ (3-way merge)..."
  APPLY_OK=true
  git diff "$BASELINE" "$NEW_SHA" | git apply --directory=zed-upstream/ --3way 2>&1 || APPLY_OK=false

  if ! $APPLY_OK; then
    # Check if there are actual conflict markers vs a catastrophic failure
    CONFLICT_FILES=$(grep -rln '<<<<<<< ' zed-upstream/ 2>/dev/null | sort || true)
    if [ -n "$CONFLICT_FILES" ]; then
      echo ""
      echo "WARNING: git apply completed with conflict markers in:"
      while IFS= read -r f; do
        echo "  $f"
      done <<< "$CONFLICT_FILES"
      echo ""
      echo "Resolve conflicts in the above files, then re-run with --resume."
    else
      echo ""
      echo "ERROR: git apply failed catastrophically (no conflict markers found)."
      if $STASHED; then
        echo "==> Rolling back zed-upstream/ via git stash pop..."
        git stash pop
      fi
      rm -f "$LOCKFILE"
      echo "Lockfile removed. Sync aborted."
      exit 1
    fi
  fi
fi

# --- Post-apply verification ---
echo ""
echo "==> Post-apply verification..."
echo ""

# 1. Conflict markers
echo "--- Conflict markers ---"
CONFLICT_FILES=$(grep -rln '<<<<<<< ' zed-upstream/ 2>/dev/null | sort || true)
if [ -n "$CONFLICT_FILES" ]; then
  echo "Files with unresolved conflict markers:"
  while IFS= read -r f; do
    echo "  $f"
  done <<< "$CONFLICT_FILES"
else
  echo "No conflict markers found."
fi

# 2. Cargo reconciliation
echo ""
echo "--- Cargo.toml reconciliation ---"
if [ -x "$RECONCILE" ]; then
  "$RECONCILE" || true
else
  echo "  (reconcile-cargo.sh not found or not executable — skipping)"
fi

# 3. Per-patch risk report
echo ""
echo "--- Per-patch risk report ---"
# Build set of files changed upstream
if $RESUME; then
  # We don't have BASELINE/NEW_SHA context on resume — use lockfile SHA
  RESUME_SHA=$(cat "$LOCKFILE" 2>/dev/null || echo "")
  if [ -n "$RESUME_SHA" ] && [ "$BASELINE" != "$RESUME_SHA" ]; then
    UPSTREAM_CHANGED=$(git diff --name-only "$BASELINE" "$RESUME_SHA" 2>/dev/null | sed 's|^|zed-upstream/|' | sort || true)
  else
    UPSTREAM_CHANGED=""
  fi
else
  UPSTREAM_CHANGED=$(git diff --name-only "$BASELINE" "$NEW_SHA" | sed 's|^|zed-upstream/|' | sort)
fi

for p in "$REPO_ROOT/patches/zed/0"*.patch; do
  [ -f "$p" ] || continue
  patch_name=$(basename "$p")
  # Extract file paths from the patch header (--- a/ lines)
  patch_files=$(grep '^--- a/' "$p" | sed 's|^--- a/||' | sort || true)
  hit=false
  while IFS= read -r pf; do
    if echo "$UPSTREAM_CHANGED" | grep -qF "$pf"; then
      hit=true
      break
    fi
  done <<< "$patch_files"

  if $hit; then
    echo "  $patch_name — ALSO changed upstream (REVIEW REQUIRED)"
  else
    echo "  $patch_name — not changed upstream (safe)"
  fi
done

# 4. Auto-baseline
if $AUTO_BASELINE && [ -z "$CONFLICT_FILES" ]; then
  echo ""
  echo "==> --auto-baseline: updating patches/zed/BASELINE to $NEW_SHA"
  echo "$NEW_SHA" > "$BASELINE_FILE"
  echo "BASELINE updated."
fi

# Clean completion
rm -f "$LOCKFILE"

echo ""
echo "==> Next steps:"
if [ -n "$CONFLICT_FILES" ]; then
  echo "  1. Resolve conflict markers listed above"
  echo "  2. cargo check -p zed"
  echo "  3. echo '${NEW_SHA:-<new-sha>}' > patches/zed/BASELINE"
else
  echo "  1. cargo check -p zed"
  if ! $AUTO_BASELINE; then
    echo "  2. echo '${NEW_SHA:-<new-sha>}' > patches/zed/BASELINE"
    echo "  3. Regenerate patches:"
    echo "       git format-patch $BASELINE..HEAD \\"
    echo "         --output-directory patches/zed/ -- zed-upstream/"
    echo "  4. git add zed-upstream/ patches/zed/ && git commit -m 'chore(zed): sync upstream to ${NEW_SHA:0:7}'"
  else
    echo "  2. Regenerate patches:"
    echo "       git format-patch $BASELINE..HEAD \\"
    echo "         --output-directory patches/zed/ -- zed-upstream/"
    echo "  3. git add zed-upstream/ patches/zed/ && git commit -m 'chore(zed): sync upstream to ${NEW_SHA:0:7}'"
  fi
fi
