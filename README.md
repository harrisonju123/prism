# PrisM — LLM Gateway + uglyhat Monorepo

A production-grade LLM gateway with integrated task management, built with Rust + Cargo workspace. Includes PrisM (intelligent routing, cost optimization, observability) and uglyhat (AI-agent project management).

## Quick Start

### Option A — Minimal (no Docker, no infra)

Route directly to an existing LiteLLM proxy or Groq — useful for dogfooding at work or getting
started in under 5 minutes.

```bash
export PATH="$HOME/.cargo/bin:$PATH"

# LiteLLM (recommended if your company uses it)
export LITELLM_BASE_URL=https://your-litellm.internal/v1
export LITELLM_API_KEY=sk-...
make dev-min
# → copies config/prism.min.toml → config/prism.toml on first run
# → PrisM at http://localhost:9100, no virtual keys, no Postgres

# Groq (free tier, no card required — console.groq.com)
export GROQ_API_KEY=gsk_...
# edit config/prism.min.toml to use [providers.groq] instead, then:
make dev-min
```

**Test it:**
```bash
make health   # → {"status":"ok"}

curl http://localhost:9100/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"cheap","messages":[{"role":"user","content":"hello"}]}'
```

**Zed → Language Models → OpenAI Compatible:**
- API URL: `http://localhost:9100/v1`
- API key: (anything, or blank)
- Model: `cheap` or `smart` (defined in `config/prism.toml`)

**Edit `config/prism.toml`** to add/rename models matching whatever your LiteLLM has configured.

---

### Option B — Full (Docker + virtual keys + observability)

```bash
export PATH="$HOME/.cargo/bin:$PATH"

# One-time setup: creates config/prism.toml, starts Postgres+ClickHouse, creates master key
make dev-setup

# Daily: start deps then run prism
make dev
```

### Prerequisites (Option B only)
- Docker + Docker Compose

### Prerequisites (both)
- Rust 1.70+ (via `rustup`)

### Basic Commands

```bash
make check          # cargo check
make ci             # fmt + lint + test (full quality gate)
make run-prism      # cargo run -p prism
make run-uh         # cargo run -p uglyhat
make install-uh     # install uh CLI to ~/.cargo/bin
make health         # curl health endpoint
make models         # curl models endpoint
make request-replay # generate request replay artifacts from OpenAPI
```

### Request Replay (agent-first endpoint testing)

Request Replay turns “test an endpoint” into a repeatable, AI-friendly workflow. It generates
versioned request templates (happy path + edge case), expected response schema references, and
a replayable runner that can target `local`, `dev`, or `staging`.

**Generate artifacts:**
- `make request-replay` (auto-discovers OpenAPI or generates via swaggo)
- Output goes to `request-replay/` (committed to the repo)

**Agent skill (IDE + agents):**
- Run `/request-replay` to execute OpenAPI discovery + replay in one flow.
- The skill runs: `prism request-replay openapi`, `prism request-replay generate`, then a happy-path replay.

**Agent hook (automatic):**
- Configure a repo hook to run `prism request-replay openapi` + `prism request-replay generate` after endpoint changes.
- This keeps `request-replay/` artifacts current without manual steps.

**OpenAPI discovery (Rust/Go-friendly):**
- `prism request-replay openapi --output-dir request-replay`
- Priority order:
  1. `PRISM_OPENAPI_PATH` (file)
  2. `PRISM_OPENAPI_URL` (URL)
  3. repo scan for `openapi.json|yaml` or `swagger.json|yaml`
  4. scrape a running server (`/openapi.json`, `/swagger.json`, etc.)
  5. Go: run `swag init` (uses `PRISM_SWAG_INIT_CMD` if provided)

**Replay a request:**
- `prism request-replay run <request-id> --env local`
- Uses `PRISM_API_KEY` for auth (or the scheme declared in OpenAPI)

**Configure environments:**
- `PRISM_LOCAL_URL` (default: `http://localhost:9100`)
- `PRISM_DEV_URL`
- `PRISM_STAGING_URL`

## Architecture

### Workspace Structure

```
PrisM/
├── crates/
│   ├── prism/              # LLM gateway (main service)
│   ├── prism-types/        # Shared OpenAI-compatible types
│   ├── prism-client/       # HTTP client for IDE integration
│   ├── prism-cli/          # CLI agent powered by PrisM
│   ├── prism-dashboard/    # Dashboard backend
│   ├── uglyhat/            # Agent PM (HTTP API + CLI)
│   └── uglyhat-panel/      # uglyhat web UI
├── config/
│   ├── prism.min.toml      # Minimal dogfood config (no Docker)
│   └── prism.dev.toml      # Full dev config template
├── zed-upstream/           # Zed editor fork (shallow clone)
├── Cargo.toml              # Workspace root
├── docker-compose.yml      # Postgres, ClickHouse, Grafana, Loki, Jaeger
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

**Configuration** (Figment — TOML file + `PRISM_` env var overrides):
```toml
# config/prism.toml
[gateway]
address = "0.0.0.0:9100"

[keys]
enabled = true   # false = no auth, no Postgres needed

[providers.litellm]
api_key = "${LITELLM_API_KEY}"
api_base = "${LITELLM_BASE_URL}"   # any OpenAI-compatible endpoint

[models.cheap]
provider = "litellm"
model = "gpt-4o-mini"
tier = 3
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

**Provider keys** (referenced in `config/prism.toml` via `${VAR}` interpolation):
- `LITELLM_API_KEY` + `LITELLM_BASE_URL` — any OpenAI-compatible proxy (e.g. your company's LiteLLM)
- `GROQ_API_KEY` — Groq (free tier at console.groq.com)
- `ANTHROPIC_API_KEY` — Anthropic direct
- `OPENAI_API_KEY` — OpenAI direct

**Infrastructure** (Option B / full setup only):
- `PRISM_POSTGRES__URL` — Postgres connection string (overrides config)
- `PRISM_CLICKHOUSE__URL` — ClickHouse HTTP endpoint (overrides config)

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
