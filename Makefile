.PHONY: build run check test lint dev fmt ci clean \
        run-prism run-uh run-acp install-uh install-prism-cli \
        docker-up docker-down docker-build docker-logs docker-deps \
        health models uh-health \
        dev-setup dev dev-min \
        dashboard-install dashboard-build dashboard-dev \
        sync-zed sync-zed-dry check-prism check-zed check-all reconcile-cargo verify-patches regenerate-patches check-upstream-drift \
        disk-usage clean-worktree-targets prune-worktrees \
        build-zed run-zed dogfood

# Development
build:
	cargo build --workspace

check:
	cargo check --workspace

test:
	cargo test --workspace

lint:
	cargo clippy --workspace -- -W clippy::all

fmt:
	cargo fmt --all

# Full CI
ci: fmt lint test

# Clean
clean:
	cargo clean

# Run individual crates
run-prism:
	cargo run -p prism

run-uh:
	cargo run -p uglyhat --bin uglyhat

# Run prism-cli in ACP agent server mode (stdio JSON-RPC for Zed)
run-acp:
	cargo run -p prism-cli -- acp

# Install the uh CLI
install-uh:
	cargo install --path crates/uglyhat --bin uh

# Install prism-cli to ~/.cargo/bin
install-prism-cli:
	cargo install --path crates/prism-cli

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
	cargo run -p prism

# Minimal local dev: no Docker, no virtual keys — routes to a LiteLLM proxy.
# Requires:
#   export LITELLM_BASE_URL=https://your-litellm.internal/v1
#   export LITELLM_API_KEY=sk-...
dev-min:
	@[ -n "$$LITELLM_BASE_URL" ] || (echo "Error: LITELLM_BASE_URL is not set"; exit 1)
	@[ -n "$$LITELLM_API_KEY" ] || (echo "Error: LITELLM_API_KEY is not set"; exit 1)
	@[ -f config/prism.toml ] || cp config/prism.min.toml config/prism.toml
	cargo run -p prism

# Quick health check against running server
health:
	curl -s http://localhost:9100/health | jq .

models:
	curl -s http://localhost:9100/v1/models | jq .

uh-health:
	curl -s http://localhost:3001/health | jq .

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
	cargo check -p prism

check-zed:
	cargo check -p zed

check-all: check-prism check-zed

# Build Zed from source (release build)
build-zed:
	cargo build -p zed --release

# Run Zed from source (debug build — faster compile, slower runtime)
run-zed:
	cargo run -p zed

# Dogfood: validate env then launch Zed with PrisM embedded gateway
dogfood:
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
	cargo run -p zed

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

clean-worktree-targets:
	@echo "Removing per-worktree target/ directories..."
	@for wt in .worktrees/*/; do \
		if [ -d "$$wt/target" ]; then \
			printf "  Cleaning %s ... " "$$(basename $$wt)"; \
			rm -rf "$$wt/target"; \
			echo "done"; \
		fi; \
	done
	@echo "Done. Next build will use shared cache at ~/.cache/prism-build"

prune-worktrees:
	@echo "=== Worktree status ==="
	@git worktree list
	@echo ""
	@echo "To remove a stale worktree: git worktree remove .worktrees/<name>"
	@echo "To remove all sprint1 worktrees (if merged):"
	@echo "  for wt in sprint1-track-{a,b,c,d}; do git worktree remove .worktrees/\$$wt --force; done"
