#!/usr/bin/env bash
# reconcile-cargo.sh — Compare root Cargo.toml against zed-upstream/Cargo.toml
#
# Reports:
#   - Workspace members in upstream missing from root (needs zed-upstream/ prefix)
#   - Root Zed entries absent from upstream (stale)
#   - External dependency version mismatches (excluding intentional overrides)
#
# Exit code: 0 = clean, 1 = discrepancies found

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT_CARGO="$REPO_ROOT/Cargo.toml"
ZED_CARGO="$REPO_ROOT/zed-upstream/Cargo.toml"

# Intentional overrides — excluded from mismatch reporting.
# Format: exact dep name as it appears in [workspace.dependencies].
SKIP_DEPS=(
    "async-tungstenite"   # 0.33.0 vs 0.31.0 — jupyter-websocket-client compat
    "tokio"               # PrisM uses features=["full"]; Zed uses a subset
    "reqwest"             # PrisM uses crates.io 0.12; Zed uses zed-reqwest fork
)

# PrisM-only deps that have no upstream counterpart (suppress "missing in upstream" noise)
PRISM_ONLY_DEPS=(
    "prism"
    "prism-types"
    "prism-client"
    "prism-cli"
    "uglyhat-panel"
    "prism-dashboard"
    "axum"
    "axum-extra"
    "sqlx"
    "tracing-subscriber"
    "figment"
    "sha2"
    "hex"
    "dashmap"
    "moka"
    "clickhouse"
    "bb8"
    "bb8-postgres"
    "tokio-postgres"
    "qdrant-client"
    "fastembed"
)

RED='\033[0;31m'
YELLOW='\033[1;33m'
GREEN='\033[0;32m'
NC='\033[0m'

ISSUES=0

err() { echo -e "${RED}[MISMATCH]${NC} $*"; ISSUES=$((ISSUES + 1)); }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
ok() { echo -e "${GREEN}[OK]${NC} $*"; }

if [ ! -f "$ROOT_CARGO" ]; then
    echo "ERROR: $ROOT_CARGO not found" >&2; exit 2
fi
if [ ! -f "$ZED_CARGO" ]; then
    echo "ERROR: $ZED_CARGO not found" >&2; exit 2
fi

echo "==> Reconciling Cargo.toml workspace members..."
echo ""

# --- 1. Workspace members ---
# Extract members from upstream (strip quotes, leading/trailing whitespace)
upstream_members=$(grep -E '^\s+"crates/' "$ZED_CARGO" | \
    sed 's/.*"\(crates\/[^"]*\)".*/\1/' | sort)

# Extract Zed crate paths from root members list only (leading whitespace = member line, not dep)
root_zed_members=$(grep -E '^\s+"zed-upstream/crates/' "$ROOT_CARGO" | \
    sed 's/.*"zed-upstream\/\(crates\/[^"]*\)".*/\1/' | sort)

# Also check extensions and tooling members
upstream_ext=$(grep -E '^\s+"(extensions|tooling)/' "$ZED_CARGO" | \
    sed 's/.*"\([^"]*\)".*/\1/' | sort)
root_zed_ext=$(grep -E '^\s+"zed-upstream/(extensions|tooling)/' "$ROOT_CARGO" | \
    sed 's/.*"zed-upstream\/\([^"]*\)".*/\1/' | sort)

# Members in upstream but missing from root
missing_from_root=$(comm -23 \
    <(echo "$upstream_members"; echo "$upstream_ext") \
    <(echo "$root_zed_members"; echo "$root_zed_ext") 2>/dev/null || true)

if [ -n "$missing_from_root" ]; then
    echo "Workspace members in zed-upstream/Cargo.toml missing from root (add with zed-upstream/ prefix):"
    while IFS= read -r m; do
        err "  Missing: \"zed-upstream/$m\""
    done <<< "$missing_from_root"
    echo ""
else
    ok "All upstream workspace members present in root."
    echo ""
fi

# Zed members in root that are absent from upstream (stale)
stale_in_root=$(comm -23 \
    <(echo "$root_zed_members"; echo "$root_zed_ext") \
    <(echo "$upstream_members"; echo "$upstream_ext") 2>/dev/null || true)

if [ -n "$stale_in_root" ]; then
    echo "Zed members in root missing from zed-upstream (possibly stale — remove or check):"
    while IFS= read -r m; do
        warn "  Stale?: \"zed-upstream/$m\""
    done <<< "$stale_in_root"
    echo ""
fi

echo "==> Reconciling [workspace.dependencies]..."
echo ""

# --- 2. External dependency version comparison ---
# Extract external deps from a Cargo.toml [workspace.dependencies] section.
# Output format: "name version" (one per line, path/workspace deps excluded).
extract_external_deps() {
    local file="$1"
    # Use sed/grep pipeline — avoids gawk-only features.
    awk '
        /^\[/ { in_deps = ($0 ~ /\[workspace\.dependencies\]/) }
        in_deps && /^[a-zA-Z_-]+ *=/ {
            name = $1
            line = $0
            # Skip path/git deps
            if (line ~ /path *=/ || line ~ /git *=/) next
            # Extract version: either "x.y.z" (simple) or version = "x.y.z" (table)
            ver = ""
            # Table form: version = "..."
            if (sub(/.*version *= *"/, "", line)) {
                sub(/".*/, "", line)
                ver = line
            }
            if (ver != "") print name " " ver
        }
    ' "$file" | sort
}

# Build skip set for comparison
is_skipped() {
    local name="$1"
    for skip in "${SKIP_DEPS[@]}" "${PRISM_ONLY_DEPS[@]}"; do
        [ "$name" = "$skip" ] && return 0
    done
    return 1
}

root_deps=$(extract_external_deps "$ROOT_CARGO")
upstream_deps=$(extract_external_deps "$ZED_CARGO")

# Check for version mismatches (deps present in both but with different versions)
while IFS=' ' read -r name version; do
    is_skipped "$name" && continue
    upstream_ver=$(echo "$upstream_deps" | awk -v n="$name" '$1 == n {print $2; exit}')
    if [ -n "$upstream_ver" ] && [ "$upstream_ver" != "$version" ]; then
        err "Version mismatch: $name — root=$version upstream=$upstream_ver"
    fi
done <<< "$root_deps"

# Deps in upstream but absent from root (may need adding)
while IFS=' ' read -r name version; do
    is_skipped "$name" && continue
    root_ver=$(echo "$root_deps" | awk -v n="$name" '$1 == n {print $2; exit}')
    if [ -z "$root_ver" ]; then
        warn "Dep in upstream not in root: $name = \"$version\" (may need workspace dep)"
    fi
done <<< "$upstream_deps"

echo ""
echo "==> Checking intentional override docs..."
OVERRIDES_DOC="$REPO_ROOT/patches/zed/CARGO_OVERRIDES.md"
if [ -f "$OVERRIDES_DOC" ]; then
    ok "CARGO_OVERRIDES.md present."
else
    warn "patches/zed/CARGO_OVERRIDES.md not found — intentional overrides undocumented."
fi

echo ""
if [ "$ISSUES" -eq 0 ]; then
    ok "No unexpected Cargo.toml discrepancies found."
    exit 0
else
    echo -e "${RED}==> $ISSUES discrepancy(ies) found. Review and update root Cargo.toml or add to SKIP list.${NC}"
    exit 1
fi
