# PrisM Platform Roadmap

Three systems, one loop: **Zed IDE** (edit) → **PrisM Gateway** (route + observe) → **prism context** (remember + coordinate).

---

## Phase 1 — Wire the Loop

Connect the three systems so data flows between them automatically.

### 1.1 Thread-aware agent sessions
> prism context threads become the context backbone for IDE agent sessions

- IDE agent panel gets a "Thread" picker (context threads, not just conversations)
- Starting an agent session calls `prism context checkin` and attaches the thread's memories + decisions to the system prompt
- Ending a session calls `prism context checkout` with summary, files touched, and next steps
- **Why first:** This is the single change that makes multi-session work coherent. Every session after the first one starts smarter.

### 1.2 Cost attribution pipeline
> PrisM events flow to prism context as activity entries

- PrisM tags each `InferenceEvent` with `x-session-id` + `x-episode-id` (IDE already sends these)
- A lightweight reconciler (cron or post-request hook) writes cost summaries to the context activity log
- `prism context recall` can answer: "how much did this thread cost so far?"
- **Why:** Cost visibility per-thread/per-agent is the most asked-for observability feature in agentic workflows.

### 1.3 Embedded context store in IDE
> Direct SQLite access from the Zed process, no CLI shelling out

- Add `prism-context` as a Cargo dependency to the IDE's PrisM provider crate
- The IDE opens the same `.prism/context.db` that the CLI uses
- Enables real-time thread/memory/decision access without subprocess overhead
- **Why:** Latency matters for inline experiences (autocomplete, context injection). CLI round-trips add ~50ms each.

---

## Phase 2 — Surface Intelligence

Make the integrated data visible and useful in the IDE.

### 2.1 Session cost gauge
> Live cost counter in the agent panel

- PrisM already computes `estimated_usd` per request; the IDE provider already tracks `session_cost_usd`
- Add a small cost badge next to the model picker: `$0.42 this session`
- Click expands to: token breakdown, per-turn costs, cache hit rate
- **Why:** Developers have no intuition for what agent sessions cost. Seeing it live changes behavior.

### 2.2 Context panel (memories + decisions)
> Sidebar panel showing what the agent "knows"

- Pulls from prism context: thread-scoped memories, recent decisions, last session's next-steps
- Editable — user can add/remove memories before starting a session
- Auto-surfaces relevant global memories by tag matching against open files
- **Why:** The black box problem. Users don't know what context the agent has. Making it visible builds trust.

### 2.3 Model routing transparency
> Show which model handled the request and why

- PrisM's `routing_decision` field already captures this
- Display in the agent panel: `claude-sonnet-4 via anthropic (fitness: 0.92, latency: 1.2s)`
- When routing falls back, show why: "primary provider rate-limited, routed to bedrock"
- **Why:** Users set "claude-sonnet-4" and assume direct Anthropic. Routing transparency prevents confusion and builds confidence in the gateway.

### 2.4 Smart model suggestions
> Use task classification to recommend models

- PrisM's classifier already categorizes requests (CodeGeneration, Refactoring, Summarization, etc.)
- After a few turns, suggest model switches: "This looks like a refactoring task — Opus scores higher for these"
- Base suggestions on PrisM's fitness scoring + historical performance from ClickHouse
- **Why:** Most users pick one model and never change. Data-driven suggestions improve quality without requiring expertise.

---

## Phase 3 — Multi-Agent Coordination

Enable multiple agents working on related tasks simultaneously.

### 3.1 Agent roster panel
> See all active agents and their work

- Pull from `prism context agents` — name, current thread, session status, last activity
- Show in IDE sidebar: which agents are active, what they're working on, what's blocked
- Click an agent to see its recent activity and session summaries
- **Why:** When running 3+ agents in worktrees, there's no visibility into what's happening. This is mission control.

### 3.2 Decision propagation
> Decisions made in one session are visible to all agents on the same thread

- Agent explicitly records decisions via tool call (`prism context decide "use sqlx not diesel" --thread <id>`)
- All agents on the same thread see decisions in their context on next turn/checkin
- IDE surfaces new decisions as notifications: "Agent claude-worktree-2 decided: use sqlx not diesel"
- **Why:** The #1 failure mode of multi-agent work is contradictory decisions. Shared decision records prevent this.

### 3.3 Handoff-driven spawning
> One agent can hand off work to another with full context

- Agent calls `prism context checkout` → prism context creates a new thread with context (summary, findings, blockers, next-steps)
- IDE picks up the handoff notification and offers to spawn a new agent worktree
- New agent session starts with the handoff context pre-loaded
- **Why:** Today handoffs are manual (copy-paste summaries). Structured handoffs preserve context across agent boundaries.

### 3.4 Thread-scoped guardrails
> PrisM enforces per-thread boundaries

- Context threads get a `scope` field: list of allowed crate/file paths
- PrisM policy engine checks tool calls against scope before execution
- Agent working on "payments" can't accidentally modify "auth" code
- **Why:** As agents get more autonomous, blast radius control becomes critical. Thread scope is the natural boundary.

---

## Phase 4 — Learning System

The platform gets smarter over time from its own usage data.

### 4.1 Fitness scoring from real usage
> ClickHouse data feeds back into model routing

- Track success signals: did the user accept the completion? Did tests pass after the edit?
- Feed into PrisM's fitness scoring: model X is 15% better at refactoring tasks than model Y
- Routing automatically shifts toward higher-performing models per task type
- **Why:** Today fitness scores are static/synthetic. Real usage data makes routing genuinely intelligent.

### 4.2 Memory auto-extraction
> Agent conversations automatically produce reusable memories

- Post-session, a lightweight LLM pass extracts key facts from the conversation
- Facts are stored as context memories, tagged by crate/domain
- Future sessions get relevant memories injected automatically
- **Why:** Most project knowledge lives in chat transcripts that no one reads again. Auto-extraction captures it.

### 4.3 Session replay + debugging
> Request inspector for agent sessions

- IDE panel that shows every LLM call in a session: request, response, model, latency, cost, tool calls
- Powered by ClickHouse `inference_events` filtered by `session_id`
- Replay a session to understand why an agent made a bad decision
- **Why:** When an agent produces bad output, there's no way to debug why. Full request-level visibility enables post-mortems.

### 4.4 Cross-session analytics
> Dashboard answering "how is my AI usage evolving?"

- Weekly cost trends, cache hit rates, model distribution, task type breakdown
- Per-thread cost efficiency: cost per accepted edit, cost per completed task
- Anomaly detection: "this thread is 3x more expensive than similar ones"
- **Why:** Without analytics, teams can't optimize. This turns PrisM from a proxy into a platform.

---

## Dependency Graph

```
Phase 1 (foundation)
├── 1.1 Thread-aware sessions ← no deps
├── 1.2 Cost attribution ← needs 1.1 (session_id linkage)
└── 1.3 Embedded context store ← no deps (but accelerates Phase 2)

Phase 2 (visibility)
├── 2.1 Cost gauge ← needs 1.2
├── 2.2 Context panel ← needs 1.1 + 1.3
├── 2.3 Routing transparency ← no deps (PrisM-only)
└── 2.4 Smart model suggestions ← needs 2.3

Phase 3 (coordination)
├── 3.1 Agent roster ← needs 1.1
├── 3.2 Decision propagation ← needs 1.1 + 2.2
├── 3.3 Handoff spawning ← needs 3.1
└── 3.4 Thread-scoped guardrails ← needs 1.1

Phase 4 (learning)
├── 4.1 Fitness from usage ← needs 1.2 + 2.3
├── 4.2 Memory auto-extraction ← needs 1.1
├── 4.3 Session replay ← needs 1.2
└── 4.4 Cross-session analytics ← needs 1.2 + 4.1
```

---

## Quick Wins (can ship independently)

| Item | Effort | Impact | Deps |
|------|--------|--------|------|
| 2.1 Cost gauge | Small | High | None (data already exists in provider) |
| 2.3 Routing transparency | Small | Medium | None (data in response headers) |
| 1.1 Thread-aware sessions | Medium | Very High | None |
| 3.1 Agent roster | Small | Medium | 1.1 |

---

## Phase CC — Claude Code Parity (IDE Agent Feature Diff)

Closes the gap between the PrisM IDE agent and Claude Code's tool surface.

### Tier 1 — Implemented ✅

| Tool | File | Description |
|------|------|-------------|
| `add_dir` | `tools/add_dir_tool.rs` | Add a working directory to the workspace mid-session via `Project::find_or_create_worktree`. |
| `task_create` | `tools/task_create_tool.rs` | Create a named task in the session-scoped `TaskStore`. |
| `task_update` | `tools/task_update_tool.rs` | Update status, description, or dependency edges of a task. |
| `task_get` | `tools/task_get_tool.rs` | Retrieve details of a specific task by ID prefix. |
| `task_list` | `tools/task_list_tool.rs` | List all tasks, optionally filtered by status. |
| `lsp` | `tools/lsp_tool.rs` | LSP code intelligence: go_to_definition, find_references, hover, document_symbols, workspace_symbols. |
| `notebook_edit` | `tools/notebook_edit_tool.rs` | Edit Jupyter notebook cells (replace/insert/delete) parsed as nbformat v4 JSON. |

**Shared infrastructure:** `tools/task_store.rs` — in-memory `TaskStore` (`Arc<Mutex<TaskStore>>`) shared across all 4 task tools per session.

### Tier 2 — Deferred

| Feature | Effort | Notes |
|---------|--------|-------|
| REPL Tool | High | Headless kernel via `runtimelib`. Blocked on ZMQ async integration without Window context. |
| Plan Mode | Medium | Thread-level read-only mode. Needs `plan_mode: bool` on `Thread` + `tool_permissions.rs` check. |
| Context Compaction | Medium | Auto-summarize old messages at 80% context fill. Needs `Thread::maybe_compact_context()`. |
| Worktree Isolation on SpawnAgent | Low | `isolation: "worktree"` option in `SpawnAgentToolInput`. |
| Cron/Loop Tool | Low | Recurring prompt execution. Low value in IDE context. |
| Deferred Tool Loading | Low | Lazy MCP schema loading to reduce context usage. |
