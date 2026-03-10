# PrisM — LLM Gateway + uglyhat Monorepo

## Agent Workflow (multi-agent parallel execution)

Each Claude Code session is an agent. Sessions auto checkin/checkout via hooks — no manual action needed at startup/shutdown.

> **IMPORTANT — use the right `uh` binary.**
> `/opt/homebrew/bin/uh` is the old Go CLI that requires a running HTTP server and will return 401 errors.
> Always use `~/.cargo/bin/uh` (the Rust rewrite, talks directly to SQLite).
> Either prepend PATH or use the full path:
> ```bash
> export PATH="$HOME/.cargo/bin:$PATH"
> # or alias for the session:
> alias uh="$HOME/.cargo/bin/uh"
> ```
> The Claude Code hooks already do this automatically. If you see 401 / "invalid api key" errors, your shell PATH is wrong.

**Set your agent name for the worktree you're in (once per shell):**
```bash
export UH_AGENT_NAME=claude-zed-surface   # matches your track/worktree branch
```
Default name if unset: `claude`.

**Checkin happens automatically on first prompt. To do it manually:**
```bash
~/.cargo/bin/uh checkin --name $UH_AGENT_NAME --capabilities rust,api,zed
# → shows your assigned tasks, other agents' current work, activity since last session
```

**Before starting any task:**
```bash
~/.cargo/bin/uh next                                    # unblocked, unclaimed tasks (excludes in_progress)
~/.cargo/bin/uh task claim <id> --name $UH_AGENT_NAME  # claim it — sets assignee + marks your current task
```
`uh next` filters out `in_progress` tasks. Claim before you start so other agents see it taken.

**When done with a task:**
```bash
~/.cargo/bin/uh task update <id> --status done
# checkout fires automatically on session end, or manually:
~/.cargo/bin/uh checkout --name $UH_AGENT_NAME --summary "what was done"
```

**When blocked:**
```bash
~/.cargo/bin/uh task block <blocking-id> <blocked-id>
~/.cargo/bin/uh report "<issue title>" --desc "..."
~/.cargo/bin/uh handoff <task-id> --summary "..." --findings "..." --next-steps "..."
```

**Discovering new work:**
```bash
~/.cargo/bin/uh task create "<title>" --epic <epic-id> --priority high
```

**See what all agents are doing:**
```bash
~/.cargo/bin/uh agents          # roster: name, session open/idle, current task
~/.cargo/bin/uh context         # full workspace overview including active agents
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
make run-prism      # cargo run -p prism
make run-uh         # cargo run -p uglyhat --bin uglyhat
make install-uh     # install uh CLI to ~/.cargo/bin
make install-prism-cli  # install prism CLI to ~/.cargo/bin
make docker-up      # docker compose up -d
make docker-deps    # start only postgres + clickhouse
make dev-setup      # one-time: create config, start deps, create virtual key
make dev            # daily: start deps + run prism locally
make dev-min        # minimal: no Docker, no virtual keys, just cargo run -p prism
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

The `uh` binary (`~/.cargo/bin/uh`) operates in **local mode** (direct SQLite, no server) by default.
**Always use `~/.cargo/bin/uh`** — the Homebrew `uh` at `/opt/homebrew/bin/uh` is an unrelated Go binary.

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
- `.gitignore` only excludes `/target` — **do not commit `.env`, `.uglyhat.json`, or any credentials**.
- Fitness cache runs on hardcoded synthetic data — feedback loop (LLM-judge → fitness) not yet wired to live traffic.
