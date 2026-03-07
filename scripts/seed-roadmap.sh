#!/usr/bin/env bash
# seed-roadmap.sh — Bootstrap PrisM development roadmap in uglyhat
# Usage: bash scripts/seed-roadmap.sh [--rebuild]
#
# Creates the full initiative → epic → task hierarchy for the PrisM platform.
# Idempotent by default (skips init if .uglyhat.json exists). Pass --rebuild
# to remove existing db and re-seed from scratch.

set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# ── Helpers ──────────────────────────────────────────────────────────────────

id_of() {
  # Extract the id field from a JSON object printed to stdout
  echo "$1" | grep -o '"id":"[^"]*"' | head -1 | cut -d'"' -f4
}

# ── Rebuild flag ──────────────────────────────────────────────────────────────

if [[ "${1:-}" == "--rebuild" ]]; then
  echo "→ --rebuild: removing existing uglyhat db..."
  rm -f .uglyhat.json .uglyhat.db .uglyhat.db-wal .uglyhat.db-shm
fi

# ── 1. Build uh ───────────────────────────────────────────────────────────────

echo "→ Building uh CLI..."
cargo build -p uglyhat --bin uh -q

UH="$REPO_ROOT/target/debug/uh"

# ── 2. Init workspace ─────────────────────────────────────────────────────────

if [[ ! -f .uglyhat.json ]]; then
  echo "→ Initialising workspace 'PrisM Platform'..."
  $UH init "PrisM Platform" > /dev/null
else
  echo "→ Workspace already initialised (skip init)"
fi

# ── 3. Create Initiatives ─────────────────────────────────────────────────────

echo "→ Creating initiatives..."

P1=$($UH initiative create "Phase 1 — Foundation" \
  --desc "Zed fork integration, PrisM library refactor, uglyhat Rust rewrite")
INIT1=$(id_of "$P1")
echo "   Phase 1: $INIT1"

P2=$($UH initiative create "Phase 2 — Deep Integration" \
  --desc "PrisM provider for Zed, agent UI panels, CLI agent integration, feedback loop")
INIT2=$(id_of "$P2")
echo "   Phase 2: $INIT2"

P3=$($UH initiative create "Phase 3 — Polish" \
  --desc "In-process PrisM, observability UI, upstream maintenance")
INIT3=$(id_of "$P3")
echo "   Phase 3: $INIT3"

# ── 4. Phase 1 epics & tasks ──────────────────────────────────────────────────

echo "→ Phase 1 epics..."

# Epic: Zed Fork Integration
E_ZED=$($UH epic create "Zed Fork Integration" --initiative "$INIT1" \
  --desc "Bring Zed into the PrisM monorepo and wire up the Cargo workspace")
EID_ZED=$(id_of "$E_ZED")

T=$($UH task create "Fork Zed on GitHub" --epic "$EID_ZED" --status todo)
$UH task update "$(id_of "$T")" --status done > /dev/null

T=$($UH task create "Bring Zed into PrisM via git subtree" --epic "$EID_ZED" --status todo)
$UH task update "$(id_of "$T")" --status done > /dev/null

T=$($UH task create "Merge workspace Cargo.toml" --epic "$EID_ZED" --status todo)
$UH task update "$(id_of "$T")" --status done > /dev/null

$UH task create "Strip Zed telemetry and account features" \
  --epic "$EID_ZED" --status todo --tags "zed" > /dev/null

$UH task create "Define Zed upstream rebase strategy" \
  --epic "$EID_ZED" --status backlog --tags "zed,maintenance" > /dev/null

# Epic: PrisM Library Refactor
E_LIB=$($UH epic create "PrisM Library Refactor" --initiative "$INIT1" \
  --desc "Split prism binary into reusable library crates")
EID_LIB=$(id_of "$E_LIB")

T=$($UH task create "Create prism lib.rs" --epic "$EID_LIB" --status todo)
$UH task update "$(id_of "$T")" --status done > /dev/null

T=$($UH task create "Create prism-types crate" --epic "$EID_LIB" --status todo)
$UH task update "$(id_of "$T")" --status done > /dev/null

T=$($UH task create "Create prism-client crate" --epic "$EID_LIB" --status todo)
$UH task update "$(id_of "$T")" --status done > /dev/null

T_BUILDER=$($UH task create "Create AppStateBuilder" \
  --epic "$EID_LIB" --status todo --priority high --tags "prism")
TID_BUILDER=$(id_of "$T_BUILDER")

$UH task create "Add Cargo feature gates" \
  --epic "$EID_LIB" --status todo --tags "prism" > /dev/null

# Epic: uglyhat Rust Rewrite
E_UH=$($UH epic create "uglyhat Rust Rewrite" --initiative "$INIT1" \
  --desc "Port uglyhat from Go to Rust with SQLite store, axum API, and CLI")
EID_UH=$(id_of "$E_UH")

for name in \
  "Port domain model types" \
  "Implement Store trait" \
  "SQLite store implementation" \
  "API handlers and router" \
  "Server binary and main.rs" \
  "CLI tool (uh)"; do
  T=$($UH task create "$name" --epic "$EID_UH" --status todo)
  $UH task update "$(id_of "$T")" --status done > /dev/null
done

# ── 5. Phase 2 epics & tasks ──────────────────────────────────────────────────

echo "→ Phase 2 epics..."

# Epic: PrisM Provider for Zed
E_PROV=$($UH epic create "PrisM Provider for Zed" --initiative "$INIT2" \
  --desc "Implement and register PrismLanguageModelProvider inside the Zed codebase")
EID_PROV=$(id_of "$E_PROV")

$UH task create "Implement PrismLanguageModelProvider" \
  --epic "$EID_PROV" --status todo --priority high --tags "zed,prism" > /dev/null

$UH task create "Register provider in language_models crate" \
  --epic "$EID_PROV" --status todo --priority high --tags "zed" > /dev/null

$UH task create "Wire PrisM into Zed model picker" \
  --epic "$EID_PROV" --status todo --tags "zed" > /dev/null

$UH task create "PrisM sidecar launch from Zed" \
  --epic "$EID_PROV" --status todo --tags "zed,prism" > /dev/null

$UH task create "Replace cloud_llm_client references" \
  --epic "$EID_PROV" --status backlog --tags "zed" > /dev/null

# Epic: Agent UI Panels
E_UI=$($UH epic create "Agent UI Panels" --initiative "$INIT2" \
  --desc "GPUI panels for uglyhat task board, agent check-in/out, and cost display")
EID_UI=$(id_of "$E_UI")

$UH task create "uglyhat task board panel (GPUI)" \
  --epic "$EID_UI" --status backlog --priority high --tags "zed,uglyhat" > /dev/null

$UH task create "Agent check-in and checkout UI" \
  --epic "$EID_UI" --status backlog --tags "zed,uglyhat" > /dev/null

$UH task create "Task context display in agent panel" \
  --epic "$EID_UI" --status backlog --tags "zed,uglyhat" > /dev/null

$UH task create "Cost display panel" \
  --epic "$EID_UI" --status backlog --tags "zed,prism" > /dev/null

# Epic: CLI Agent Integration
E_CLI=$($UH epic create "CLI Agent Integration" --initiative "$INIT2" \
  --desc "Complete uh CLI agent workflow commands and HTTP client mode")
EID_CLI=$(id_of "$E_CLI")

$UH task create "uh checkin and checkout commands" \
  --epic "$EID_CLI" --status todo --tags "uglyhat,cli" > /dev/null

$UH task create "uh next, claim, and handoff commands" \
  --epic "$EID_CLI" --status todo --tags "uglyhat,cli" > /dev/null

$UH task create "HTTP client mode for remote uglyhat server" \
  --epic "$EID_CLI" --status backlog --tags "uglyhat,cli" > /dev/null

# Epic: Feedback Loop
E_FB=$($UH epic create "Feedback Loop" --initiative "$INIT2" \
  --desc "Connect LLM-judge benchmark to production traffic and improve routing")
EID_FB=$(id_of "$E_FB")

$UH task create "Wire LLM-judge benchmark to production" \
  --epic "$EID_FB" --status backlog --tags "prism" > /dev/null

$UH task create "Populate fitness cache with real traffic data" \
  --epic "$EID_FB" --status backlog --tags "prism" > /dev/null

$UH task create "Upgrade classifier to embedding-based approach" \
  --epic "$EID_FB" --status backlog --tags "prism" > /dev/null

# ── 6. Phase 3 epics & tasks ──────────────────────────────────────────────────

echo "→ Phase 3 epics..."

# Epic: In-Process PrisM
E_INPROC=$($UH epic create "In-Process PrisM" --initiative "$INIT3" \
  --desc "Embed the PrisM gateway directly in Zed, eliminating the sidecar process")
EID_INPROC=$(id_of "$E_INPROC")

T_EMBED=$($UH task create "Embed gateway in Zed (no sidecar)" \
  --epic "$EID_INPROC" --status backlog --priority high --tags "prism,zed")
TID_EMBED=$(id_of "$T_EMBED")

$UH task create "Use AppStateBuilder from Zed" \
  --epic "$EID_INPROC" --status backlog --tags "prism,zed" > /dev/null

# Epic: Observability UI
E_OBS=$($UH epic create "Observability UI" --initiative "$INIT3" \
  --desc "GPUI panels for cost, routing, and edit prediction metrics")
EID_OBS=$(id_of "$E_OBS")

$UH task create "GPUI cost and routing dashboard panels" \
  --epic "$EID_OBS" --status backlog --tags "zed,prism" > /dev/null

$UH task create "Route edit predictions through PrisM" \
  --epic "$EID_OBS" --status backlog --tags "zed,prism" > /dev/null

# Epic: Upstream Maintenance
E_UP=$($UH epic create "Upstream Maintenance" --initiative "$INIT3" \
  --desc "Keep the Zed subtree up to date with upstream")
EID_UP=$(id_of "$E_UP")

$UH task create "Zed rebase strategy and automation scripts" \
  --epic "$EID_UP" --status backlog --tags "zed,maintenance" > /dev/null

$UH task create "Automated upstream merge testing" \
  --epic "$EID_UP" --status backlog --tags "zed,maintenance" > /dev/null

# ── 7. Dependencies ───────────────────────────────────────────────────────────

echo "→ Wiring dependencies..."
# AppStateBuilder must land before In-Process PrisM
$UH task block "$TID_BUILDER" "$TID_EMBED"

# ── 8. Summary ────────────────────────────────────────────────────────────────

echo ""
echo "✓ Roadmap seeded. Quick checks:"
echo ""
echo "  uh context          — workspace overview"
echo "  uh next --limit 10  — highest-priority unblocked tasks"
echo "  uh tasks --status done   — completed Phase 1 work"
echo "  uh tasks --status todo   — active todo items"
