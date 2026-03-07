.PHONY: build run check test lint dev fmt ci clean \
        run-prism run-uh install-uh \
        docker-up docker-down docker-build docker-logs \
        health models uh-health \
        dashboard-install dashboard-build dashboard-dev \
        sync-zed sync-zed-dry check-prism check-zed check-all reconcile-cargo verify-patches regenerate-patches check-upstream-drift

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

# Install the uh CLI
install-uh:
	cargo install --path crates/uglyhat --bin uh

# Docker
docker-up:
	docker compose up -d

docker-down:
	docker compose down

docker-build:
	docker compose build

docker-logs:
	docker compose logs -f prism

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
