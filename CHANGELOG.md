# Changelog

All notable changes to PrisM will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0-alpha.1] - 2026-03-16

### Added

**LLM Gateway (crates/prism)**
- OpenAI-compatible REST API (`/v1/chat/completions`, `/v1/models`)
- Multi-provider support: OpenAI, Anthropic, LiteLLM, Groq, any OpenAI-compatible endpoint
- Health-aware multi-provider fallback chains
- Virtual API key system with rate limiting (sliding-window RPM/TPM) and daily/monthly budget tracking
- Virtual key rotation scheduler (configurable interval, runs hourly)
- Model alias system — map logical names (e.g. `smart`, `cheap`) to real models
- Intelligent model routing with fitness scoring and policy-based selection
- Request classification (chat, summarization, code generation, etc.)
- Automatic context window management (drop-oldest or error strategy)
- Event logging to ClickHouse (batched, async, versioned migrations)
- Structured audit log for all key mutations
- SSE streaming support for long-running completions
- Figment-based config (TOML + `PRISM_` env var overrides)
- Full observability stack: Prometheus, Grafana, Loki, Jaeger, OpenTelemetry
- JSON and text structured logging with optional Loki sink
- Per-key spend tracking with Postgres-backed reconciliation

**Agent Coordination (crates/prism-context)**
- SQLite-backed persistent context store (no external deps)
- Threads: named context buckets grouping related work
- Memory: key-value facts with upsert semantics that survive session boundaries
- Decisions: recorded rationale for choices made, optionally attached to threads
- Agent check-in/check-out with activity tracking and session history
- Snapshot and recall by thread, tag, or time window

**Context CLI (crates/prism-cli)**
- `prism context` subcommands: init, thread, remember, forget, decide, recall, checkin, checkout, agents, activity, snapshot
- Config discovery by walking up from `$CWD` (`.prism/context.json`)

**IDE Integration (ide/)**
- Full Zed editor integration (225 crates, GPU-accelerated via GPUI)
- Agent thread panel with task graph, execution console, and memory browser
- PrisM gateway as the language model backend (BYOK, no cloud subscription required)
- prism-hq panels: agent roster, task board, dashboard

### Changed
- Replaced AGPL license with dual license: Apache-2.0 (gateway/CLI) + GPL-3.0 (IDE)
- Removed Zed cloud subscription UI — replaced with BYOK onboarding
- First-run UX now guides users to configure their own API keys
- Bundle scripts updated: `crates/zed` paths → `ide/zed`
- Linux desktop app IDs updated: `dev.zed.Zed*` → `dev.prism.Prism*`
- macOS DMG assets renamed: `Zed-*.dmg` → `Prism-*.dmg`

### Infrastructure
- GitHub Actions CI: gateway tests, prism-context tests, IDE cargo check
- GitHub Actions release pipeline: macOS DMG + Linux binary builds, automatic GitHub Release creation
- Docker Compose: Postgres, ClickHouse, Grafana, Loki, Jaeger, OpenTelemetry Collector

[0.1.0-alpha.1]: https://github.com/harrisonju123/PrisM/releases/tag/v0.1.0-alpha.1
