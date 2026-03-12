# PrisM — LLM Gateway + Agent Monorepo

## Agent Workflow (multi-agent coordination)

Each Claude Code session is an agent. Sessions auto checkin/checkout via hooks — no manual action needed at startup/shutdown.

**Set your agent name (once per shell):**
```bash
export PRISM_AGENT_NAME=claude-zed-surface   # matches your track/worktree branch
```
Default name if unset: `claude`. The old `UH_AGENT_NAME` env var is still read as a fallback.

**Checkin happens automatically on first prompt. To do it manually:**
```bash
~/.cargo/bin/prism context checkin --name $PRISM_AGENT_NAME --capabilities rust,api,zed
# → returns active threads, global memories, recent sessions, other agents
```

**Organize work into threads (named context buckets):**
```bash
~/.cargo/bin/prism context thread create auth-refactor --desc "JWT auth migration" --tags auth,security
~/.cargo/bin/prism context thread list                         # active threads
~/.cargo/bin/prism context thread archive auth-refactor        # mark thread done
```

**Save knowledge that persists across sessions:**
```bash
~/.cargo/bin/prism context remember "auth_approach" "Using JWT with refresh tokens" --thread auth-refactor
~/.cargo/bin/prism context forget "auth_approach"              # delete a memory
~/.cargo/bin/prism context memories --thread auth-refactor     # list memories for a thread
```

**Record decisions with rationale:**
```bash
~/.cargo/bin/prism context decide "Use JWT" --content "Chose JWT over sessions because..." --thread auth-refactor
```

**Recall context mid-session:**
```bash
~/.cargo/bin/prism context recall auth-refactor               # full thread context (memories, decisions, sessions)
~/.cargo/bin/prism context recall --tags auth,security        # memories + decisions by tag
~/.cargo/bin/prism context recall --since 2h                  # everything from last 2 hours
```

**Checkout when done:**
```bash
~/.cargo/bin/prism context checkout --name $PRISM_AGENT_NAME --summary "what was done"
```

**See what all agents are doing:**
```bash
~/.cargo/bin/prism context agents          # roster: name, session open/idle, current thread
~/.cargo/bin/prism context context         # full workspace overview
```

---

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
make run-prism      # cargo run -p prism --bin prism-server
make install-prism-cli  # install prism CLI to ~/.cargo/bin (prism + prism context subcommands)
make docker-up      # docker compose up -d
make docker-deps    # start only postgres + clickhouse
make dev-setup      # one-time: create config, start deps, create virtual key
make dev            # daily: start deps + run prism-server locally
make dev-min        # minimal: no Docker, no virtual keys, just cargo run -p prism
make docker-down    # docker compose down
make health         # curl health endpoint
make models         # curl models endpoint
```

Cargo is at `~/.cargo/bin/cargo` — may not be in shell PATH. Use `export PATH="$HOME/.cargo/bin:$PATH"` if needed.

## Architecture

Cargo workspace with 6 crates:

```
crates/
├── prism-types/         # Shared types (leaf, no workspace deps)
├── prism-context/       # Context management library for AI agents
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs               # Library root (error, model, store)
│       ├── error.rs             # Error enum (NotFound, BadRequest, Conflict, Internal, Sqlx)
│       ├── model/mod.rs         # Domain types (Workspace, Thread, Memory, Decision, Agent, Session, Activity)
│       ├── config.rs            # Config discovery (.prism/context.json with .uglyhat.json fallback)
│       └── store/               # Store trait (~19 async methods) + SQLite implementation
│           └── sqlite/          # sqlx SQLite, WAL mode, schema.sql embedded via include_str!
├── prism-client/        # HTTP client for gateway API
├── prism/               # LLM gateway lib + server
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
├── prism-cli/           # Unified agent CLI + context subcommands
│   └── src/
│       ├── main.rs              # Commands: run, personas, models, health, sessions, context, ...
│       └── context.rs           # prism context subcommands (ported from prism-context bin)
└── prism-hq/            # All Zed IDE panels (agent roster, task board, dashboard, etc.)
```

Workspace root: `Cargo.toml` with `[workspace] resolver = "2"`.
Dockerfile builds `prism-server`: `cargo build --release -p prism`.
Gateway binary is `prism-server`; agent CLI binary is `prism`.

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

## prism-context Patterns (SQLite / sqlx)

prism-context uses SQLite with raw sqlx — no ORM. All TEXT columns for UUIDs, timestamps, and arrays.

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

Store trait has ~19 async methods — requires `#[async_trait]`. `RETURNING` clauses work with SQLite 3.35+.

## Conventions

- Structs: PascalCase. Functions: snake_case.
- Keep `pub` surface minimal — only expose what other modules need.
- Tests go in `#[cfg(test)] mod tests` at the bottom of each file.
- Don't add comments/docs to code you didn't change.
- Virtual keys: `prism_<32 hex>` format, SHA-256 hashed, plaintext never stored.
- Module files: singular (`workspace.rs` not `workspaces.rs`).
- Concurrent context fetches: use `tokio::join!` not sequential awaits.

## prism context CLI

Context management is now a subcommand of the main `prism` CLI binary (`~/.cargo/bin/prism`).
Config file: `.prism/context.json` (discovered by walking up from `$CWD`; falls back to `.uglyhat.json` for backward compat).

```bash
# Setup
prism context init <name>                                    # bootstrap workspace → .prism/context.json + .prism/context.db
prism context context                                        # workspace overview JSON

# Threads (named context buckets — the core organizing primitive)
prism context thread create <name> [--desc --tags]           # start a new context thread
prism context thread list [--active --archived]              # list threads
prism context thread archive <name>                          # mark thread done

# Memory (persistent facts + knowledge)
prism context remember <key> <value> [--thread --tags]       # save/upsert a memory (UNIQUE on workspace+key)
prism context forget <key>                                   # delete a memory
prism context memories [--thread --tags]                     # list memories

# Decisions
prism context decide <title> [--content --thread --tags]     # record a decision with rationale
prism context decisions [--thread --tags]                    # list decisions

# Context retrieval (the main agent interface)
prism context recall <thread-name>                           # full thread context (memories, decisions, sessions)
prism context recall --tags <tag1,tag2>                      # memories + decisions by tag
prism context recall --since 2h                              # everything recent (supports m/h/d)

# Agent coordination
prism context checkin --name <agent> [--capabilities --thread]
prism context checkout --name <agent> [--summary --findings --files --next-steps]
prism context agents                                         # who's doing what

# History
prism context activity [--since --actor --limit]
prism context snapshot [--label]                             # capture point-in-time state
```

`PRISM_AGENT_NAME` env sets agent name. `UH_AGENT_NAME` is still read as fallback.

### Core entities (6)

| Entity | Purpose |
|---|---|
| **Workspace** | Project-level scope. One per repo. |
| **Thread** | Named context bucket. Groups related memories, decisions, sessions. |
| **Memory** | Atomic fact/knowledge unit. Key-value with upsert on (workspace_id, key). |
| **Decision** | Why a choice was made. Optionally attached to a thread. |
| **Agent + Session** | Agent identity + session records (summary, findings, files_touched, next_steps). |
| **Activity** | Event log. Mutations auto-log activity. |

## Syncing Zed Upstream

`zed-upstream/` tracks `harrisonju123/zed` (our Zed fork). It was imported as a flat directory snapshot at baseline `5c481c6` — **not** a git submodule or subtree. PrisM patches are stored in `patches/zed/` and documented in `patches/zed/PATCHES.md`.

To pull in new Zed upstream changes:

```bash
# 1. Preview what changed (no write)
./scripts/sync-zed-upstream.sh --dry-run

# 2. Apply upstream delta + list patches to re-verify
./scripts/sync-zed-upstream.sh

# 3. Resolve any conflict markers in zed-upstream/
# 4. Verify compilation
cargo check -p zed

# 5. Update baseline
echo '<new-sha>' > patches/zed/BASELINE

# 6. Regenerate patch files
git format-patch <old-baseline>..HEAD \
  --output-directory patches/zed/ -- zed-upstream/

# 7. Commit
git add zed-upstream/ patches/zed/ && git commit -m "chore(zed): sync upstream to <sha>"
```

See `patches/zed/PATCHES.md` for per-file risk notes and what to watch for during conflict resolution.

## Rust Memory & Stack Safety

Rust does not auto-grow thread stacks. Each thread gets a fixed stack (typically 2-8MB), and overflows crash the process. Apply these rules when writing or reviewing Rust code:

### Stack overflow prevention
- **Recursive code / deep call chains:** Wrap with `stacker::maybe_grow(512 * 1024, 8 * 1024 * 1024, || { ... })` when calling into recursive third-party code (cranelift, tree-sitter, etc.) or writing recursive functions without bounded depth. `stacker` is already a workspace dependency.
- **Large stack allocations:** Never put large arrays or structs on the stack. Use `Box::new(...)` or `Vec` for anything over a few KB.
- **Spawned threads:** Use `std::thread::Builder::new().stack_size(N)` when the workload needs more than the default 8MB. The Zed codebase already does this for grammar loading (64MB thread in `language_registry.rs`).
- **Async futures:** Each `.await` point adds to the future's in-memory size. Break very deep async chains into `tokio::spawn` to reset the stack.

### Heap memory leaks
- **`Arc` cycles:** Use `Weak` to break reference cycles between `Arc` pointers. If two structs hold `Arc` to each other, neither will ever drop.
- **Unbounded collections:** `Vec`, `HashMap`, and channels that grow without bound are the most common Rust leak. Add eviction, capacity limits, or periodic cleanup.
- **Detached tasks:** `task.detach()` means no one will ever cancel or collect that work. Prefer storing `Task` handles and dropping them when the owner is dropped.
- **Static caches:** Mutex-guarded `Vec` pools (like `PARSERS` in `language.rs`) grow but never shrink. Be aware of this when adding similar patterns.

### Zed-specific stack pressure areas
- **Cranelift/WASM compilation:** Deep recursive IR lowering in `WasmStore::new()`. Mitigated with `stacker::maybe_grow` in `with_parser()`.
- **Tree-sitter parsing:** Recursive descent on deeply nested syntax. Grammar loading uses a dedicated 64MB thread.
- **GCD dispatch threads (macOS):** Have ~8MB stacks and are not under our control — always guard recursive work that may run on GCD threads.

## Known Issues

- `classify_summarization` test is flaky (classifies as Documentation instead of Summarization) — pre-existing Phase 2 issue. Keyword-based classifier is brittle; agents don't use textbook phrasing.
- `.gitignore` only excludes `/target` — **do not commit `.env`, `.prism/context.json`, or any credentials**.
- Fitness cache runs on hardcoded synthetic data — feedback loop (LLM-judge → fitness) not yet wired to live traffic.
