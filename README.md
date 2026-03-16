# PrisM — AI Workspace

An AI workspace where agents do software and humans supervise. Code, debugging, review, and testing are panes inside an agent workflow — not the other way around.

## Vision

Current AI coding tools bolt a chat sidebar onto a text editor. The editor is still the product; the AI is a helper. This makes sense when AI writes one function at a time. It breaks down when an agent is running a full feature branch, managing dependencies, writing tests, and debugging failures across hours of work.

PrisM inverts the model. The **agent thread** is the primary unit of work. Code is one pane. The editor exists to let you supervise what agents are doing — inspect their reasoning, unblock them, approve decisions, and redirect when they're wrong.

**Seven pillars:**

1. **Task Graph** — work decomposed into nodes with dependencies, blockers, confidence, and cost estimates. Agents own nodes; humans approve gates.
2. **Agent State** — persistent memory and decisions that survive session boundaries. Agents pick up where they left off.
3. **Execution** — unified timeline of commands, test runs, file changes, and tool calls. Every action is recorded and replayable.
4. **Review** — code review as a first-class gate in the task graph, not an afterthought. Diff, comment, approve — embedded in the workflow.
5. **Economics** — every model call has a cost. Routing, budgets, and spend tracking are built in, not bolted on.
6. **Memory** — rules, decisions, failures, and current truths stored as structured knowledge, not chat logs.
7. **Automation** — hooks, personas, and sandboxing so agents operate within defined boundaries without needing human approval for every action.

**What's built today:** LLM gateway (smart routing, cost tracking, observability), agent coordination (threads, memory, decisions), full IDE integration (225 editor crates), and an agent CLI with personas and sandboxing.

---

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

**Prism IDE → Language Models → OpenAI Compatible:**
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
- Rust 1.93+ (via `rustup`)

### Basic Commands

```bash
make check              # cargo check (workspace)
make ci                 # fmt + lint + test (full quality gate)
make run-prism          # cargo run -p prism --bin prism-server
make install-prism-cli  # install prism CLI to ~/.cargo/bin
make run-prism-ide      # build and launch PrisM IDE
make dogfood            # workspace overview (prism context context)
make health             # curl health endpoint
make models             # curl models endpoint
```

## Architecture

### Workspace Structure

```
PrisM/
├── crates/
│   ├── prism/              # LLM gateway (main service)
│   ├── prism-types/        # Shared OpenAI-compatible types
│   ├── prism-client/       # HTTP client for IDE integration
│   ├── prism-cli/          # CLI agent powered by PrisM (personas, sandboxing, tools)
│   ├── prism-context/      # Agent coordination library (threads, memory, decisions)
│   └── prism-hq/           # All PrisM IDE panels (agent roster, task board, dashboard)
├── config/
│   ├── prism.min.toml      # Minimal dogfood config (no Docker)
│   └── prism.dev.toml      # Full dev config template
├── ide/                    # Editor crates (225 crates, fully integrated)
├── Cargo.toml              # Workspace root
├── docker-compose.yml      # Postgres, ClickHouse, Grafana, Loki, Jaeger
└── CLAUDE.md               # Development guide
```

### PrisM — LLM Gateway

**Purpose:** Smart routing, cost optimization, observability for LLM requests

**Key features:**
- OpenAI-compatible REST API (`/v1/chat/completions`, `/v1/models`)
- Multi-provider support (OpenAI, Anthropic, custom) with health-aware fallback chains
- Virtual API key system with rate limiting & budget tracking
- Request classification (chat, summarization, code generation, etc.)
- Intelligent model routing (fitness scoring, policy-based)
- Event logging to ClickHouse (batched, async)
- SSE streaming for long-running completions
- Model alias system and automatic context window management

**Architecture:**
- `main.rs` — startup, database init, background tasks
- `config.rs` — Figment-based config (TOML + `PRISM_` env vars)
- `types.rs` — OpenAI-compatible request/response structures
- `models/` — static model catalog with pricing + alias system
- `api/` — REST endpoint handlers
- `providers/` — LLM provider trait + implementations
- `proxy/` — request forwarding, cost computation, context window management
- `routing/` — model selection & fitness scoring
- `keys/` — virtual key management, rate limiting, budgets, audit log
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

### prism-context — Agent Coordination

**Purpose:** Persistent context management for AI agents across sessions

**Key features:**
- SQLite-based store (no external dependencies)
- Threads: named context buckets grouping related work
- Memory: key-value facts that survive session boundaries (upsert on key)
- Decisions: recorded rationale for choices made
- Agent check-in/check-out with activity tracking
- `prism context` CLI subcommand

**CLI Usage:**
```bash
# Initialize workspace
prism context init "Project Name"

# Threads (named context buckets)
prism context thread create auth-refactor --desc "JWT migration" --tags auth
prism context thread list
prism context thread archive auth-refactor

# Memory (persistent facts)
prism context remember "auth_approach" "Using JWT with refresh tokens" --thread auth-refactor
prism context forget "auth_approach"
prism context memories --thread auth-refactor

# Decisions
prism context decide "Use JWT" --content "Chose JWT over sessions because..." --thread auth-refactor

# Context retrieval
prism context recall auth-refactor          # full thread context
prism context recall --tags auth,security   # by tag
prism context recall --since 2h             # recent activity

# Agent coordination
prism context checkin --name claude --capabilities rust,api
prism context checkout --name claude --summary "completed auth refactor"
prism context agents                        # who's doing what
```

### prism-types

**Purpose:** Lightweight, zero-dependency types shared between PrisM, clients, and integrations

- OpenAI API request/response structures
- InferenceEvent for observability
- Provider configuration types

### prism-client

**Purpose:** HTTP client for IDE integration (e.g., PrisM language models)

- SSE streaming support for long-running requests
- Transparent authentication (API key injection)
- Error handling and retry logic

## What We're Building

The gateway and coordination layer are the foundation. The IDE panels are the interface. Here's what each component does:

**Agent Inbox** — work items tied to graph nodes, not chat messages. An agent thread is a unit of work with state, history, and cost — not a conversation. Items route to the right agent automatically based on capabilities and current load.

**Task Board / Graph** — kanban meets dependency graph. Nodes have agent ownership, blockers, confidence estimates, cost budgets, and approval gates. Humans see the graph; agents see their queue.

**Execution Console** — unified timeline of commands, test runs, file changes, tool calls, and debugger sessions. Every action is recorded. You can pause an agent mid-execution, inspect state, and resume or redirect.

**Review Pane** — code review as a first-class gate in the task graph. Diff, comment, and approve without leaving the workspace. Review completion unblocks the next graph node automatically.

**Debugger Pane** — hypothesis-driven debugging loop. An agent surfaces a symptom, proposes hypotheses ranked by probability, runs experiments, and updates its belief model. You see the reasoning, not just the output.

**Routing / Telemetry** — flight recorder for every model call: which model was chosen, why, what it cost, how long it took, and what permissions were required. Cost budgets enforced per-key and per-session.

**Memory / State Browser** — a living runbook. Rules, decisions, failures, and current truths stored as structured knowledge. Agents read from it; humans edit it. Not a chat log — a structured belief store.

## Development Workflow

### Start Work

```bash
# Check what's active
prism context agents

# Create a thread for the work
prism context thread create <feature-name> --desc "what you're building"

# Check in
prism context checkin --name claude --capabilities rust,api --thread <feature-name>

# Create a worktree for isolated work
git worktree add .worktrees/<feature-name> -b <feature-name>

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

# Save knowledge mid-session
prism context remember "key" "value" --thread <feature-name>
prism context decide "Decision title" --content "rationale" --thread <feature-name>
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

### Complete Work

```bash
prism context checkout --name claude --summary "what was accomplished" --thread <feature-name>
prism context thread archive <feature-name>
```

## Configuration & Deployment

### Environment Variables

**Agent identity:**
- `PRISM_AGENT_NAME` — agent name for context checkin/checkout (default: `claude`)

**Provider keys** (referenced in `config/prism.toml` via `${VAR}` interpolation):
- `LITELLM_API_KEY` + `LITELLM_BASE_URL` — any OpenAI-compatible proxy (e.g. your company's LiteLLM)
- `GROQ_API_KEY` — Groq (free tier at console.groq.com)
- `ANTHROPIC_API_KEY` — Anthropic direct
- `OPENAI_API_KEY` — OpenAI direct

**Infrastructure** (Option B / full setup only):
- `PRISM_POSTGRES__URL` — Postgres connection string (overrides config)
- `PRISM_CLICKHOUSE__URL` — ClickHouse HTTP endpoint (overrides config)

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

**SQLite** (prism-context store)
- Auto-initialized on first `prism context` command
- File: `.prism/context.db`

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
- Applied on startup (idempotent, versioned migrations)
- Events written asynchronously in batches

### Database Patterns (prism-context)

**SQLite (via sqlx):**
- All TEXT columns for UUIDs, timestamps, JSON arrays
- Store trait with ~19 async methods
- Transactions: `pool.begin().await?` → execute on `&mut *tx` → `tx.commit().await?`
- Config discovered by walking up from `$CWD`: `.prism/context.json`

## Testing

```bash
# Unit tests
cargo test -p prism --features full
cargo test -p prism-context

# Integration tests (requires docker-up)
cargo test --test '*' -- --nocapture

# Known flaky test
cargo test classify_summarization -- --nocapture --test-threads=1
# Issue: keyword-based classifier doesn't match LLM phrasing. Pre-existing issue.
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
RUST_LOG=debug cargo run -p prism --bin prism-server 2>&1 | grep -i clickhouse
```

### prism CLI Not Found
```bash
# Install the CLI
make install-prism-cli

# Verify it's in PATH
prism --version
prism context context   # workspace overview
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
- **.gitignore:** only excludes `/target` — **do not commit `.env`, `.prism/context.json`, or credentials**

## Project Links

- **PrisM Architecture:** see `CLAUDE.md` for full design details
- **IDE crates:** `ide/` (PrisM IDE, 225 crates)

## Download

Pre-built binaries are available on the [GitHub Releases](https://github.com/harrisonju123/PrisM/releases) page.

| Platform | Download |
|----------|----------|
| macOS (Apple Silicon) | `Prism-aarch64.dmg` |
| macOS (Intel) | `Prism-x86_64.dmg` |
| Linux (x86_64) | `prism-server-linux-x86_64` |
| Linux (aarch64) | `prism-server-linux-aarch64` |

## Getting Started

### End Users (Gateway Only)

1. Download `prism-server` for your platform from [Releases](https://github.com/harrisonju123/PrisM/releases)
2. Create a minimal config file:

```toml
# config/prism.toml
[gateway]
address = "0.0.0.0:9100"

[keys]
enabled = false

[providers.openai]
api_key = "${OPENAI_API_KEY}"

[models.default]
provider = "openai"
model = "gpt-4o-mini"
tier = 3
```

3. Run: `OPENAI_API_KEY=sk-... ./prism-server`
4. Point any OpenAI-compatible client to `http://localhost:9100/v1`

### IDE

1. Download the `.dmg` (macOS) from [Releases](https://github.com/harrisonju123/PrisM/releases)
2. Open PrisM IDE
3. Go to Language Models settings and add your API key

## Supported Providers

PrisM supports any OpenAI-compatible endpoint plus native Anthropic:

| Provider | Config key | Notes |
|----------|------------|-------|
| OpenAI | `providers.openai` | `OPENAI_API_KEY` |
| Anthropic | `providers.anthropic` | `ANTHROPIC_API_KEY` |
| LiteLLM | `providers.litellm` | Any LiteLLM proxy |
| Groq | `providers.groq` | `GROQ_API_KEY` (free tier) |
| Azure OpenAI | `providers.azure` | `AZURE_OPENAI_API_KEY` |
| Custom | `providers.<name>` | Any OpenAI-compatible endpoint |

## License

PrisM uses a dual license:

- **crates/** (gateway, CLI, context): [Apache License 2.0](LICENSE-APACHE)
- **ide/** (editor, derived from Zed): [GNU General Public License v3.0](LICENSE-GPL)

See [LICENSE](LICENSE) for the full dual-license explainer.
