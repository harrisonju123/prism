# PrisM — LLM Gateway + uglyhat Monorepo

A production-grade LLM gateway with integrated task management, built with Rust + Cargo workspace. Includes PrisM (intelligent routing, cost optimization, observability) and uglyhat (AI-agent project management).

## Quick Start

### Prerequisites
- Rust 1.70+ (via `rustup`)
- Docker + Docker Compose (for Postgres, ClickHouse)
- SQLite 3.35+ (bundled with uglyhat)

### Setup

```bash
# Clone and enter the repo
git clone <repo-url>
cd PrisM
export PATH="$HOME/.cargo/bin:$PATH"

# Start databases
make docker-up

# Run tests to verify setup
make ci
```

### Basic Commands

```bash
# Check compilation
make check

# Run full quality gate (fmt + lint + test)
make ci

# Start PrisM gateway
make run-prism

# Start uglyhat server
make run-uh

# Install uh CLI
make install-uh

# View health/models endpoints
make health
make models
```

## Architecture

### Workspace Structure

```
PrisM/
├── crates/
│   ├── prism/              # LLM gateway (main service)
│   ├── prism-types/        # Shared OpenAI-compatible types
│   ├── prism-client/       # HTTP client for IDE integration
│   └── uglyhat/            # Agent PM (HTTP API + CLI)
├── zed-upstream/           # Zed editor fork (shallow clone)
├── Cargo.toml              # Workspace root
├── docker-compose.yml      # Postgres, ClickHouse
└── CLAUDE.md               # Development guide
```

### PrisM — LLM Gateway

**Purpose:** Smart routing, cost optimization, observability for LLM requests

**Key features:**
- OpenAI-compatible REST API (`/v1/chat/completions`, `/v1/models`)
- Multi-provider support (OpenAI, Anthropic, custom)
- Virtual API key system with rate limiting & budget tracking
- Request classification (chat, summarization, code generation, etc.)
- Intelligent model routing (fitness scoring, policy-based)
- Event logging to ClickHouse (batched, async)
- SSE streaming for long-running completions

**Architecture:**
- `main.rs` — startup, database init, background tasks
- `config.rs` — Figment-based config (TOML + `PRISM_` env vars)
- `types.rs` — OpenAI-compatible request/response structures
- `models.rs` — static model catalog with pricing
- `api/` — REST endpoint handlers
- `providers/` — LLM provider trait + implementations
- `proxy/` — request forwarding, cost computation
- `routing/` — model selection & fitness scoring
- `keys/` — virtual key management, rate limiting, budgets
- `observability/` — ClickHouse event writer
- `classifier/` — rules-based task classification

**Configuration:**
```bash
# Via environment variables
export PRISM_LOG_LEVEL=info
export PRISM_POSTGRES_URL="postgresql://..."
export PRISM_CLICKHOUSE_URL="http://localhost:8123"
export PRISM_OPENAI_API_KEY="sk-..."

# Or via config.toml (TOML + env var interpolation)
[prism]
log_level = "info"
postgres_url = "${PRISM_POSTGRES_URL}"
```

### uglyhat — Agent PM

**Purpose:** Task tracking, initiative planning, and agent coordination

**Key features:**
- SQLite-based task store (no external dependencies)
- HTTP API for programmatic access
- CLI tool (`uh`) for local task management
- Rich task context with dependencies & decisions
- Agent check-in/check-out with activity tracking
- Workspace-based multi-tenancy

**Architecture:**
- `main.rs` — HTTP server (Axum, port 3001)
- `lib.rs` — library root
- `model/` — domain types (Workspace, Epic, Task, etc.)
- `store/` — Store trait + SQLite impl (45+ async methods)
- `api/` — REST handlers (one file per entity)
- `middleware/auth.rs` — X-API-Key / Bearer auth
- `bin/uh.rs` — CLI tool (clap derive, direct SQLite)

**CLI Usage:**
```bash
# Initialize workspace
uh init "Project Name"

# Task workflow
uh next                              # find next priority task
uh task claim <id> --name claude
uh task update <id> --status in_progress
uh task update <id> --status done

# Agent check-ins
uh checkin --name claude --capabilities rust,api
uh checkout --name claude --summary "completed X feature"

# Planning
uh initiative create "Q1 Roadmap"
uh epic create "Auth System" --initiative <id>
uh task create "Add JWT support" --epic <id>

# Reporting
uh report "API timeout issue" --severity high
```

### prism-types

**Purpose:** Lightweight, zero-dependency types shared between PrisM, clients, and integrations

- OpenAI API request/response structures
- InferenceEvent for observability
- Provider configuration types

### prism-client

**Purpose:** HTTP client for IDE integration (e.g., Zed language models)

- SSE streaming support for long-running requests
- Transparent authentication (API key injection)
- Error handling and retry logic

## Development Workflow

### Start Work

```bash
# Find next task
uh next

# Claim it
uh task claim <task-id> --name claude

# Create a worktree for isolated work
git worktree add .worktrees/<feature-name> -b <feature-name>
cd .worktrees/<feature-name>

# or use shorthand:
claude worktree <feature-name>
```

### While Working

```bash
# Run checks frequently
make check

# Run tests when adding/modifying features
make test

# Full quality gate before commits
make ci
```

### Merge Back

```bash
# Commit your work
git add -A
git commit -m "feat: description of work"

# Return to main worktree
cd ../..
git worktree prune

# Fast-forward merge
git merge .worktrees/<feature-name>
```

### Complete Task

```bash
uh task update <task-id> --status done
uh checkout --name claude --summary "what was accomplished"
```

## Configuration & Deployment

### Environment Variables

**PrisM:**
- `PRISM_LOG_LEVEL` — debug, info, warn, error
- `PRISM_POSTGRES_URL` — Postgres connection string
- `PRISM_CLICKHOUSE_URL` — ClickHouse HTTP endpoint
- `PRISM_OPENAI_API_KEY` — OpenAI provider key
- `PRISM_ANTHROPIC_API_KEY` — Anthropic provider key

**uglyhat:**
- `UGLYHAT_ADDR` — listen address (default 0.0.0.0:3001)
- `UGLYHAT_DB_PATH` — SQLite database path (default `.uglyhat.db`)

### Databases

**Postgres** (PrisM key store, request logs)
```bash
docker-compose up -d postgres
# Auto-migrations on PrisM startup
```

**ClickHouse** (observability)
```bash
docker-compose up -d clickhouse
# Schema applied on PrisM startup
```

**SQLite** (uglyhat task store)
- Auto-initialized on first `uh` command
- File: `.uglyhat.db`

### Docker

**Development:**
```bash
make docker-up      # Start Postgres + ClickHouse
make docker-down    # Stop containers
```

**Production:**
```bash
# Build PrisM image (crates/prism only)
docker build -t prism:latest .

# Run with env vars
docker run -e PRISM_LOG_LEVEL=info \
           -e PRISM_POSTGRES_URL=... \
           prism:latest
```

## Key Patterns

### Dependency Injection
All shared state flows through `AppState` (Axum), passed via `State` extractor:
```rust
struct AppState {
    key_service: Arc<KeyService>,
    provider: Arc<dyn Provider>,
    // ...
}

async fn handler(State(state): State<Arc<AppState>>) -> Result<Json<Response>> {
    // use state.key_service, etc.
}
```

### Error Handling
Centralized error enum maps to HTTP status:
```rust
pub enum PrismError {
    NotFound(String),
    Unauthorized,
    RateLimited,
    Internal(String),
    // ...
}

impl IntoResponse for PrismError {
    fn into_response(self) -> Response {
        // status code + JSON error body
    }
}

// Propagate with ?
fn handler() -> Result<Json<T>> {
    let key = key_service.get(&id).await?;  // auto-converts to PrismError
    Ok(Json(key))
}
```

### Observability
Use `tracing` everywhere (not println):
```rust
tracing::info!(model = %model_name, tokens = count, "request processed");
tracing::warn!(error = %err, "retry attempt");
```

### Database Patterns (PrisM)

**Postgres (via sqlx):**
- Raw SQL migrations in `crates/prism/migrations/postgres/`
- Auto-applied on startup
- Virtual keys stored hashed (SHA-256)

**ClickHouse (via clickhouse-rs):**
- Schema defined as code in `observability/schema.rs`
- Applied on startup (idempotent ALTER TABLE)
- Events written asynchronously in batches

### Database Patterns (uglyhat)

**SQLite (via sqlx):**
- All TEXT columns for UUIDs, timestamps, JSON arrays
- Store trait with ~45 async methods
- Transactions: `pool.begin().await?` → execute on `&mut *tx` → `tx.commit().await?`
- CLI talks directly to SQLite (no HTTP layer)

## Testing

```bash
# Unit tests
cargo test -p prism
cargo test -p uglyhat

# Integration tests (requires docker-up)
cargo test --test '*' -- --nocapture

# Known flaky test
cargo test classify_summarization -- --nocapture --test-threads=1
# Issue: keyword-based classifier doesn't match LLM phrasing. Pre-existing Phase 2 issue.
```

## Troubleshooting

### Cargo Path Issue
```bash
# If cargo commands fail, ensure PATH is set
export PATH="$HOME/.cargo/bin:$PATH"
which cargo  # verify
```

### Postgres Connection
```bash
# Verify connection
psql -U prism -d prism -h localhost -p 5432

# Check env var
echo $PRISM_POSTGRES_URL
```

### ClickHouse Events Not Showing
```bash
# Verify connection to ClickHouse
curl http://localhost:8123/ping

# Check logs for warnings
RUST_LOG=debug cargo run -p prism 2>&1 | grep -i clickhouse
```

### uglyhat CLI Not Found
```bash
# After install-uh, verify ~/.cargo/bin/uh exists
ls -la ~/.cargo/bin/uh

# And that it's in PATH
uh --version
```

## Code Style & Conventions

- **Modules:** singular names (`workspace.rs` not `workspaces.rs`)
- **Visibility:** keep `pub` surface minimal
- **Tests:** `#[cfg(test)] mod tests` at end of file
- **Comments:** only for non-obvious logic; don't document unchanged code
- **Naming:** structs PascalCase, functions snake_case
- **Errors:** use `?` operator with centralized error enum

## Known Issues

- **Flaky test:** `classify_summarization` — classifier is keyword-based, brittle with LLM phrasing
- **Fitness cache:** uses synthetic data; feedback loop (LLM-judge → live traffic) not yet wired
- **.gitignore:** only excludes `/target` — **do not commit `.env`, `.uglyhat.json`, or credentials**

## Project Links

- **PrisM Architecture:** see `CLAUDE.md` for full design details
- **uglyhat Go original:** `/Users/harrisonju/Documents/Projects/uglyhat/`
- **Zed fork:** `zed-upstream/` (shallow clone of harrisonju123/zed)

## License

[License info to be added]
