# PrisM Cargo.toml Intentional Overrides

Documents every intentional difference between the root `Cargo.toml` and
`zed-upstream/Cargo.toml`. These are excluded from `scripts/reconcile-cargo.sh`
reporting via the `SKIP_DEPS` and `PRISM_ONLY_DEPS` lists at the top of that script.

---

## External Dependency Overrides

| Dependency | Root Value | Upstream Value | Reason |
|---|---|---|---|
| `async-tungstenite` | `0.33.0` | `0.31.0` | `jupyter-websocket-client 1.1.0` requires `async-tungstenite >=0.32`; bumped to resolve dep conflict |
| `tokio` features | `["full"]` | (subset, e.g. `["rt", "macros", ...]`) | PrisM gateway server needs the full Tokio runtime including `io-util`, `time`, and `fs` |
| `reqwest` | `crates.io "0.12"` | `zed-reqwest` (git fork) | PrisM uses the standard crates.io reqwest; Zed uses a custom fork with TLS patches. Mixing them would cause duplicate dep issues |
| `[profile.release] lto` | `true` | `"thin"` | PrisM production binary optimizes for minimum size; Zed uses thin LTO for faster build times |
| `[profile.release] strip` | `true` | (not set) | PrisM strips debug symbols for production distribution |
| `[profile.release] debug` | (not set) | `"limited"` | PrisM strips all debug info; Zed keeps limited debug info for crash symbolication |

## PrisM-Only Dependencies

These dependencies have no upstream counterpart and are excluded from "missing in upstream" warnings:

| Dependency | Purpose |
|---|---|
| `prism` | PrisM LLM gateway crate |
| `prism-types` | Lightweight shared OpenAI-compatible types |
| `prism-client` | HTTP client for IDE→gateway SSE streaming |
| `prism-cli` | PrisM CLI tooling |
| `uglyhat-panel` | GPUI panel: uglyhat agent PM integration |
| `prism-dashboard` | GPUI panel: cost/routing observability in Zed right dock |
| `axum` | PrisM HTTP server framework |
| `axum-extra` | Axum extension utilities |
| `sqlx` | PrisM/uglyhat async SQL toolkit (Postgres + SQLite) |
| `tracing-subscriber` | PrisM log formatting and filtering |
| `figment` | PrisM TOML+env config layering |
| `sha2` | Virtual API key hashing |
| `hex` | SHA-256 output encoding |
| `dashmap` | Concurrent rate-limit state |
| `moka` | In-memory LRU/TTL cache for key lookups |
| `clickhouse` | PrisM observability event sink |
| `bb8` | Postgres connection pool for PrisM |
| `bb8-postgres` | tokio-postgres adapter for bb8 |
| `tokio-postgres` | Postgres async client (PrisM routing state) |
| `qdrant-client` | Vector DB client for embedding-based classifier |
| `fastembed` | Local embedding model for classifier tier |

---

## Workspace Member Differences

The root `Cargo.toml` adds PrisM-specific crates as members:

```
crates/prism
crates/uglyhat
crates/prism-types
crates/prism-client
crates/prism-cli
crates/uglyhat-panel
crates/prism-dashboard
```

All Zed crates from `zed-upstream/Cargo.toml` are mirrored as workspace members
with a `zed-upstream/` path prefix. When upstream adds a new crate, `reconcile-cargo.sh`
will report it as "Missing" until the root Cargo.toml is updated.

---

## Maintenance

When the reason for an override changes or is resolved:
1. Remove the entry from this file
2. Remove the dep name from `SKIP_DEPS` or `PRISM_ONLY_DEPS` in `scripts/reconcile-cargo.sh`
3. Update the root `Cargo.toml` to match upstream

Run `make reconcile-cargo` after any Cargo.toml change to verify no new unintentional drift.
