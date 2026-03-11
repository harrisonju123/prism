CARGO := $(HOME)/.cargo/bin/cargo
export CARGO_TARGET_DIR ?= $(HOME)/.cache/prism-build

.PHONY: build run check test lint dev fmt ci clean \
        run-prism run-uh run-acp install-uh install-prism-cli dev-all \
        docker-up docker-down docker-build docker-logs docker-deps \
        health models uh-health \
        dev-setup dev dev-min \
        dashboard-install dashboard-build dashboard-dev \
        sync-zed sync-zed-dry check-prism check-zed check-all reconcile-cargo verify-patches regenerate-patches check-upstream-drift \
        disk-usage prune-build-cache sweep \
        build-zed run-zed dogfood

# Development
build:
	$(CARGO) build --workspace

check:
	$(CARGO) check --workspace

test:
	$(CARGO) test --workspace

lint:
	$(CARGO) clippy --workspace -- -W clippy::all

fmt:
	$(CARGO) fmt --all

# Full CI
ci: fmt lint test

# Clean everything: shared cache, legacy ./target, worktree targets
clean:
	$(CARGO) clean
	@[ ! -d ./target ] || (echo "Removing legacy ./target ..." && rm -rf ./target)
	@for wt in .worktrees/*/; do \
		[ -d "$$wt/target" ] && echo "Removing $$wt/target ..." && rm -rf "$$wt/target"; \
	done; true

# Run individual crates
run-prism:
	$(CARGO) run -p prism

run-uh:
	$(CARGO) run -p uglyhat --bin uh

# Run prism-cli in ACP agent server mode (stdio JSON-RPC for Zed)
run-acp:
	cargo run -p prism-cli -- acp

# Install the uh CLI
install-uh:
	$(CARGO) install --path crates/uglyhat --bin uh

# Install prism-cli to ~/.cargo/bin
install-prism-cli:
	$(CARGO) install --path crates/prism-cli

# Docker
docker-up:
	docker compose up -d

# Start only postgres + clickhouse (deps for local dev)
docker-deps:
	docker compose up -d postgres clickhouse

docker-down:
	docker compose down

docker-build:
	docker compose build

docker-logs:
	docker compose logs -f prism

# One-time setup: create config/prism.toml, start deps, create virtual key
dev-setup:
	@bash scripts/dev-setup.sh

# Daily driver: start deps then run prism locally
dev: docker-deps
	$(CARGO) run -p prism

# Minimal local dev: no Docker, no virtual keys — routes to a LiteLLM proxy.
# Requires:
#   export LITELLM_BASE_URL=https://your-litellm.internal/v1
#   export LITELLM_API_KEY=sk-...
dev-min:
	@[ -n "$$LITELLM_BASE_URL" ] || (echo "Error: LITELLM_BASE_URL is not set"; exit 1)
	@[ -n "$$LITELLM_API_KEY" ] || (echo "Error: LITELLM_API_KEY is not set"; exit 1)
	@[ -f config/prism.toml ] || cp config/prism.min.toml config/prism.toml
	$(CARGO) run -p prism

# Quick health check against running server
health:
	curl -s http://localhost:9100/health | jq .

models:
	curl -s http://localhost:9100/v1/models | jq .

uh-health:
	~/.cargo/bin/uh context

# Dashboard
dashboard-install:
	cd dashboard && pnpm install

dashboard-build:
	cd dashboard && pnpm run build

dashboard-dev:
	cd dashboard && pnpm run dev

# Zed upstream sync
sync-zed:
	./scripts/sync-zed-upstream.sh

sync-zed-dry:
	./scripts/sync-zed-upstream.sh --dry-run

check-prism:
	$(CARGO) check -p prism

check-zed:
	$(CARGO) check -p zed

check-all: check-prism check-zed

# Build Zed from source (release build)
build-zed:
	$(CARGO) build -p zed --release

# Run Zed from source (debug build — faster compile, slower runtime)
run-zed:
	$(CARGO) run -p zed

# Remove build artifacts not accessed in the last 7 days, cap cache at 20GB
# Requires: cargo install cargo-sweep (auto-installed on first run)
sweep:
	@command -v cargo-sweep >/dev/null 2>&1 || (echo "Installing cargo-sweep..." && $(CARGO) install cargo-sweep)
	$(CARGO) sweep --time 7
	$(CARGO) sweep --maxsize 20

# Prune incremental cache when it exceeds threshold, sweep stale artifacts, clean worktrees
CACHE_PRUNE_THRESHOLD_GB ?= 5
prune-build-cache:
	@cache_dir="$(CARGO_TARGET_DIR)/debug/incremental"; \
	if [ -d "$$cache_dir" ]; then \
		size_kb=$$(du -sk "$$cache_dir" 2>/dev/null | cut -f1); \
		size_gb=$$((size_kb / 1048576)); \
		if [ "$$size_gb" -ge "$(CACHE_PRUNE_THRESHOLD_GB)" ]; then \
			echo "  Build cache incremental dir is $${size_gb}GB (threshold $(CACHE_PRUNE_THRESHOLD_GB)GB) — pruning..."; \
			rm -rf "$$cache_dir"; \
			echo "  ✓ Pruned incremental cache"; \
		fi; \
	fi
	@[ ! -d ./target ] || (echo "  Removing stale ./target ..." && rm -rf ./target && echo "  ✓ Removed legacy ./target")
	@for wt in .worktrees/*/; do \
		[ -d "$$wt/target" ] && echo "  Removing $$wt/target ..." && rm -rf "$$wt/target"; \
	done; true
	@echo "  Pruning stale worktrees..."
	@git worktree prune -v
	@if command -v cargo-sweep >/dev/null 2>&1; then \
		echo "  Running cargo-sweep..."; \
		$(MAKE) --no-print-directory sweep; \
	fi

# Run Prism in background + Zed in foreground; kills Prism on exit
dev-all:
	@echo "Starting Prism in background..."
	$(CARGO) run -p prism &
	@PRISM_PID=$$!; \
	trap "kill $$PRISM_PID 2>/dev/null" EXIT INT TERM; \
	$(CARGO) run -p zed

# Dogfood: validate env then launch Zed with PrisM embedded gateway
dogfood: prune-build-cache
	@echo "=== PrisM Dogfood ==="
	@echo "Checking prerequisites..."
	@( [ -n "$$ANTHROPIC_API_KEY" ] || [ -n "$$OPENAI_API_KEY" ] || [ -n "$$GOOGLE_AI_STUDIO_API_KEY" ] ) \
		|| (echo "Error: Set at least one of ANTHROPIC_API_KEY, OPENAI_API_KEY, or GOOGLE_AI_STUDIO_API_KEY"; exit 1)
	@echo "  ✓ Provider key found"
	@which prism >/dev/null 2>&1 || (echo "Warning: prism-cli not found. Run 'make install-prism-cli' for agent spawning."; echo "")
	@echo ""
	@echo "Launching Zed with embedded PrisM gateway..."
	@echo "  - Assistant panel: select a PrisM model to chat"
	@echo "  - Agent roster: spawn prism-cli agents in worktrees"
	@echo ""
	$(CARGO) run -p zed

check-upstream-drift:
	bash scripts/check-upstream-drift.sh

reconcile-cargo:
	./scripts/reconcile-cargo.sh

verify-patches:
	@echo "==> Verifying PrisM patches are present in the working tree..."
	@for p in patches/zed/0*.patch; do \
		printf "  %s ... " "$$p"; \
		if git apply --check --reverse "$$p" 2>/dev/null; then \
			echo "applied (OK)"; \
		else \
			echo "NOT APPLIED (stale or missing)"; \
		fi; \
	done

regenerate-patches:
	rm -f patches/zed/0*.patch
	git format-patch $$(cat patches/zed/BASELINE)..HEAD \
		--output-directory patches/zed/ -- zed-upstream/

# Disk management
disk-usage:
	@echo "=== Shared build cache ==="
	@du -sh ~/.cache/prism-build 2>/dev/null || echo "  (empty — not yet built)"
	@echo ""
	@echo "=== Per-worktree target dirs (legacy) ==="
	@for wt in .worktrees/*/; do \
		size=$$(du -sh "$$wt/target" 2>/dev/null | cut -f1); \
		[ -n "$$size" ] && printf "  %-30s %s\n" "$$(basename $$wt)" "$$size"; \
	done
	@echo ""
	@echo "=== Active worktrees ==="
	@git worktree list

