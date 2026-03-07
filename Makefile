.PHONY: build run check test lint dev fmt ci clean \
        run-prism run-uh install-uh \
        docker-up docker-down docker-build docker-logs \
        health models \
        dashboard-install dashboard-build dashboard-dev

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

# Dashboard
dashboard-install:
	cd dashboard && pnpm install

dashboard-build:
	cd dashboard && pnpm run build

dashboard-dev:
	cd dashboard && pnpm run dev
