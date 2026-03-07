# PrisM — LLM Gateway + uglyhat Monorepo

## Workflow: Plan → Worktree → Execute

**Before executing any non-trivial plan, always create a worktree first.**

```bash
# Start a new worktree for isolated work
claude worktree <feature-name>
# or manually:
git worktree add .worktrees/<feature-name> -b <feature-name>
```

This allows parallel execution in isolation and clean merges back to main. Each worktree gets its own branch. Merge when work is stable and reviewed.

## Quick Reference

```bash
make check          # cargo check
make test           # cargo test
make lint           # cargo clippy -- -W clippy::all
make fmt            # cargo fmt
make ci             # fmt + lint + test (full quality gate)
make run-prism      # cargo run -p prism
make run-uh         # cargo run -p uglyhat --bin uglyhat
make install-uh     # install uh CLI to ~/.cargo/bin
make docker-up      # docker compose up -d
make docker-down    # docker compose down
make health         # curl health endpoint
make models         # curl models endpoint
```

Cargo is at `~/.cargo/bin/cargo` — may not be in shell PATH. Use `export PATH="$HOME/.cargo/bin:$PATH"` if needed.

## Architecture

Cargo workspace with two crates:

```
crates/
├── prism/               # LLM gateway binary
│   ├── Cargo.toml
│   ├── migrations/postgres/
│   └── src/
│       ├── main.rs              # Startup, Postgres/ClickHouse init, background tasks
│       ├── config.rs            # Figment-based config (TOML + PRISM_ env vars)
│       ├── error.rs             # PrismError enum → HTTP status codes
│       ├── types.rs             # OpenAI-compatible request/response types, InferenceEvent
│       ├── models.rs            # Static model catalog with pricing
│       ├── api/                 # REST endpoints (health, models, key management)
│       ├── classifier/          # Rules-based task type classification
│       ├── keys/                # Virtual API keys, rate limiting, budgets
│       │   ├── mod.rs           # VirtualKey, AuthContext, MaybeAuth/MasterAuth extractors, KeyService
│       │   ├── virtual_key.rs   # LRU KeyCache + Postgres KeyRepository
│       │   ├── rate_limit.rs    # Sliding-window RPM/TPM limiter (DashMap)
│       │   └── budget.rs        # Daily/monthly spend tracker with reconciliation
│       ├── observability/       # ClickHouse event writer (batched, async)
│       ├── providers/           # LLM provider trait + implementations (OpenAI, Anthropic)
│       ├── proxy/               # Request forwarding, cost computation, SSE streaming
│       ├── routing/             # Smart model routing (fitness scoring, policies)
│       └── server/              # Axum router, CORS middleware
└── uglyhat/             # AI-agent PM — HTTP API + CLI
    ├── Cargo.toml
    └── src/
        ├── main.rs              # Server binary (port 3001, UGLYHAT_ADDR/UGLYHAT_DB_PATH env)
        ├── lib.rs               # Library root
        ├── error.rs             # Error enum → HTTP status codes
        ├── model/               # Domain types (Workspace, Initiative, Epic, Task, …)
        ├── store/               # Store trait + SQLite implementation (45 async methods)
        │   └── sqlite/          # sqlx SQLite, WAL mode, schema.sql embedded via include_str!
        ├── api/                 # Axum handlers (one file per entity)
        ├── middleware/auth.rs   # X-API-Key / Bearer → workspace_id injection
        ├── server/router.rs     # Public + protected route table
        └── bin/uh.rs            # CLI binary (clap derive, direct SQLite, JSON stdout)
```

Workspace root: `Cargo.toml` with `[workspace] resolver = "2"`.
Dockerfile builds only prism: `COPY crates ./crates && cargo build --release -p prism`.

## Key Patterns

### Dependency injection
All shared state flows through `AppState` (in `proxy/handler.rs`), passed as `Arc<AppState>` via axum's `State` extractor. New services get added as fields on `AppState`.

### Error handling
`PrismError` variants map directly to HTTP status codes. Use `Result<T>` (alias for `Result<T, PrismError>`). Propagate with `?`, map external errors with `.map_err(|e| PrismError::Internal(...))`.

### Axum extractors
Custom extractors (`MaybeAuth`, `MasterAuth`) implement `FromRequestParts<S>`. They read from request extensions, which are injected via `from_fn` middleware layers in `router.rs`.

### Module organization
Each module has `mod.rs` for public API + re-exports, with implementation details in sibling files. Traits define interfaces (`Provider`), implementations are private.

### Observability
Use `tracing` macros everywhere (not println). Structured fields: `tracing::info!(model = %name, tokens = count, "message")`.

### Configuration
Figment layers: TOML file → `PRISM_` env var overrides. Env vars like `${VAR}` in TOML are resolved at provider init. Every config struct needs `Default` impl with sensible values.

### Database patterns
- ClickHouse: schema-as-code in `observability/schema.rs`, applied on startup
- Postgres: raw SQL migrations in `migrations/postgres/`, applied on startup via `sqlx::raw_sql`
- Both are best-effort on startup (warn on failure, don't crash)

### Background tasks
Pattern: `tokio::spawn` + `tokio::select!` with `cancel.cancelled()` for graceful shutdown. See rate limiter pruning and budget reconciliation in `main.rs`.

## uglyhat Patterns (SQLite / sqlx)

uglyhat uses SQLite with raw sqlx — no ORM. All TEXT columns for UUIDs, timestamps, and arrays.

```rust
// UUIDs: always stringify
.bind(id.to_string())
row.try_get::<String, _>("col")?.parse::<Uuid>()?

// Timestamps: RFC3339 strings
.bind(ts.to_rfc3339())

// Vec<String>: JSON TEXT column
.bind(serde_json::to_string(&tags)?)
let tags: Vec<String> = serde_json::from_str(&raw)?;

// Transactions
let mut tx = pool.begin().await?;
// execute on &mut *tx
tx.commit().await?;

// Single row fetch
fetch_optional(...).await?.ok_or_else(|| Error::NotFound(id))
```

Store trait has ~45 async methods — requires `#[async_trait]`. `RETURNING` clauses work with SQLite 3.35+.

## Conventions

- Structs: PascalCase. Functions: snake_case.
- Keep `pub` surface minimal — only expose what other modules need.
- Tests go in `#[cfg(test)] mod tests` at the bottom of each file.
- Don't add comments/docs to code you didn't change.
- Virtual keys: `prism_<32 hex>` format, SHA-256 hashed, plaintext never stored.
- Module files: singular (`workspace.rs` not `workspaces.rs`).
- Concurrent context fetches: use `tokio::join!` not sequential awaits.

## uglyhat CLI (uh)

The `uh` binary operates in **local mode** (direct SQLite, no server) by default.

```bash
uh init "Project Name"                    # bootstrap workspace → .uglyhat.json + .uglyhat.db
uh context                                # workspace overview JSON
uh next [--limit 5]                       # prioritized unblocked tasks
uh report <title> [--desc --severity --source --tags]
uh initiative create <name> [--desc]
uh initiative list
uh epic create <name> --initiative <id> [--desc]
uh epic list --initiative <id>
uh task create <name> --epic <id> [--desc --priority --assignee --tags --status]
uh task get <id>
uh task update <id> [--status --assignee --priority --name --desc]
uh task claim <id> --name <agent>
uh task deps <id>
uh task block <blocking-id> <blocked-id>
uh task context <id>                      # rich briefing with decisions, deps, activity
uh tasks [--status --domain --assignee --unassigned]
uh decision create <title> [--content --initiative --epic]
uh decision list
uh note <title> [--content --task-id]
uh activity [--since --actor --limit]
uh checkin --name <agent> [--capabilities rust,api]
uh checkout --name <agent> [--summary "..."]
uh handoff <task-id> --summary "..." [--findings --blockers --next-steps]
```

Config: `.uglyhat.json` discovered by walking up from `$CWD`. `UH_AGENT_NAME` env sets agent name for handoffs.

## Known Issues

- `classify_summarization` test is flaky (classifies as Documentation instead of Summarization) — pre-existing Phase 2 issue. Keyword-based classifier is brittle; agents don't use textbook phrasing.
- `.gitignore` only excludes `/target` — **do not commit `.env`, `.uglyhat.json`, or any credentials**.
- Fitness cache runs on hardcoded synthetic data — feedback loop (LLM-judge → fitness) not yet wired to live traffic.
