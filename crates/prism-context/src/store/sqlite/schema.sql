-- Workspaces
CREATE TABLE IF NOT EXISTS workspaces (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

-- Threads (named context buckets)
CREATE TABLE IF NOT EXISTS threads (
    id              TEXT PRIMARY KEY,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'active'
        CHECK(status IN ('active','archived')),
    tags            TEXT NOT NULL DEFAULT '[]',
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE(workspace_id, name)
);
CREATE INDEX IF NOT EXISTS idx_threads_workspace ON threads(workspace_id);
CREATE INDEX IF NOT EXISTS idx_threads_status ON threads(workspace_id, status);

-- Memories (atomic facts / knowledge units)
CREATE TABLE IF NOT EXISTS memories (
    id               TEXT PRIMARY KEY,
    workspace_id     TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    thread_id        TEXT REFERENCES threads(id) ON DELETE SET NULL,
    key              TEXT NOT NULL,
    value            TEXT NOT NULL,
    source           TEXT NOT NULL DEFAULT '',
    tags             TEXT NOT NULL DEFAULT '[]',
    access_count     INTEGER NOT NULL DEFAULT 0,
    last_accessed_at TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    UNIQUE(workspace_id, key)
);
CREATE INDEX IF NOT EXISTS idx_memories_workspace ON memories(workspace_id);
CREATE INDEX IF NOT EXISTS idx_memories_thread ON memories(thread_id)
    WHERE thread_id IS NOT NULL;

-- Decisions (why a choice was made)
CREATE TABLE IF NOT EXISTS decisions (
    id              TEXT PRIMARY KEY,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    thread_id       TEXT REFERENCES threads(id) ON DELETE SET NULL,
    title           TEXT NOT NULL,
    content         TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'active',
    scope           TEXT NOT NULL DEFAULT 'thread'
        CHECK(scope IN ('thread','workspace')),
    superseded_by   TEXT,
    supersedes      TEXT,
    tags            TEXT NOT NULL DEFAULT '[]',
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_decisions_workspace ON decisions(workspace_id);
CREATE INDEX IF NOT EXISTS idx_decisions_thread ON decisions(thread_id)
    WHERE thread_id IS NOT NULL;

-- Agents
CREATE TABLE IF NOT EXISTS agents (
    id                TEXT PRIMARY KEY,
    workspace_id      TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name              TEXT NOT NULL,
    state             TEXT NOT NULL DEFAULT 'idle',
    capabilities      TEXT NOT NULL DEFAULT '[]',
    current_thread_id TEXT REFERENCES threads(id) ON DELETE SET NULL,
    last_checkin      TEXT,
    last_heartbeat    TEXT,
    parent_agent_id   TEXT REFERENCES agents(id) ON DELETE SET NULL,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    UNIQUE(workspace_id, name)
);
CREATE INDEX IF NOT EXISTS idx_agents_workspace ON agents(workspace_id);

-- Agent Sessions
CREATE TABLE IF NOT EXISTS agent_sessions (
    id            TEXT PRIMARY KEY,
    agent_id      TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    thread_id     TEXT REFERENCES threads(id) ON DELETE SET NULL,
    started_at    TEXT NOT NULL,
    ended_at      TEXT,
    summary       TEXT NOT NULL DEFAULT '',
    findings      TEXT NOT NULL DEFAULT '[]',
    files_touched TEXT NOT NULL DEFAULT '[]',
    next_steps    TEXT NOT NULL DEFAULT '[]',
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent ON agent_sessions(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_workspace ON agent_sessions(workspace_id);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_open ON agent_sessions(agent_id) WHERE ended_at IS NULL;

-- Activity Log
CREATE TABLE IF NOT EXISTS activity_log (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    actor         TEXT NOT NULL DEFAULT '',
    action        TEXT NOT NULL,
    entity_type   TEXT NOT NULL,
    entity_id     TEXT NOT NULL,
    summary       TEXT NOT NULL DEFAULT '',
    detail        TEXT,
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_activity_log_workspace_time ON activity_log(workspace_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_activity_log_actor ON activity_log(workspace_id, actor)
    WHERE actor != '';

-- Decision notifications (propagation queue)
CREATE TABLE IF NOT EXISTS decision_notifications (
    id            TEXT PRIMARY KEY,
    decision_id   TEXT NOT NULL REFERENCES decisions(id) ON DELETE CASCADE,
    agent_id      TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    notified_at   TEXT NOT NULL,
    acknowledged  INTEGER NOT NULL DEFAULT 0,
    UNIQUE(decision_id, agent_id)
);

-- Handoffs (structured task delegation)
CREATE TABLE IF NOT EXISTS handoffs (
    id              TEXT PRIMARY KEY,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    from_agent_id   TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    to_agent_id     TEXT REFERENCES agents(id) ON DELETE SET NULL,
    thread_id       TEXT REFERENCES threads(id) ON DELETE SET NULL,
    task            TEXT NOT NULL,
    constraints     TEXT NOT NULL DEFAULT '{}',
    mode            TEXT NOT NULL DEFAULT 'delegate_and_await',
    status          TEXT NOT NULL DEFAULT 'pending',
    result          TEXT,
    started_at      TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_handoffs_workspace ON handoffs(workspace_id);
CREATE INDEX IF NOT EXISTS idx_handoffs_status ON handoffs(workspace_id, status);

-- Thread guardrails (ownership, locking, restrictions)
CREATE TABLE IF NOT EXISTS thread_guardrails (
    id              TEXT PRIMARY KEY,
    thread_id       TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    owner_agent_id  TEXT REFERENCES agents(id) ON DELETE SET NULL,
    locked          INTEGER NOT NULL DEFAULT 0,
    allowed_files   TEXT NOT NULL DEFAULT '[]',
    allowed_tools   TEXT NOT NULL DEFAULT '[]',
    cost_budget_usd REAL,
    cost_spent_usd  REAL NOT NULL DEFAULT 0.0,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE(thread_id)
);

-- Inbox entries (supervisory feed for human review)
CREATE TABLE IF NOT EXISTS inbox_entries (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    entry_type   TEXT NOT NULL DEFAULT 'info',
    title        TEXT NOT NULL,
    body         TEXT NOT NULL DEFAULT '',
    severity     TEXT NOT NULL DEFAULT 'info'
        CHECK(severity IN ('critical','warning','info')),
    source_agent TEXT,
    ref_type     TEXT,
    ref_id       TEXT,
    read         INTEGER NOT NULL DEFAULT 0,
    dismissed    INTEGER NOT NULL DEFAULT 0,
    resolved     INTEGER NOT NULL DEFAULT 0,
    resolution   TEXT,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_inbox_workspace ON inbox_entries(workspace_id, dismissed, read);
CREATE INDEX IF NOT EXISTS idx_inbox_dedup ON inbox_entries(workspace_id, entry_type, source_agent, dismissed, resolved);

-- Plans (groups of work packages from one intent)
CREATE TABLE IF NOT EXISTS plans (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    intent       TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'draft'
        CHECK(status IN ('draft','approved','active','completed','cancelled')),
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_plans_workspace ON plans(workspace_id, status);

-- Work packages (actionable units within a plan)
CREATE TABLE IF NOT EXISTS work_packages (
    id               TEXT PRIMARY KEY,
    workspace_id     TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    plan_id          TEXT REFERENCES plans(id) ON DELETE CASCADE,
    intent           TEXT NOT NULL,
    acceptance_criteria TEXT NOT NULL DEFAULT '[]',
    ordinal          INTEGER NOT NULL DEFAULT 0,
    status           TEXT NOT NULL DEFAULT 'draft'
        CHECK(status IN ('draft','planned','ready','in_progress','review','done','cancelled')),
    depends_on       TEXT NOT NULL DEFAULT '[]',
    thread_id        TEXT REFERENCES threads(id) ON DELETE SET NULL,
    assigned_agent   TEXT,
    tags             TEXT NOT NULL DEFAULT '[]',
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_work_packages_workspace ON work_packages(workspace_id);
CREATE INDEX IF NOT EXISTS idx_work_packages_plan ON work_packages(plan_id) WHERE plan_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_work_packages_status ON work_packages(workspace_id, status);

-- File claims (advisory file locking for multi-agent coordination)
CREATE TABLE IF NOT EXISTS file_claims (
    id           TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    file_path    TEXT NOT NULL,
    agent_name   TEXT NOT NULL,
    claimed_at   TEXT NOT NULL,
    expires_at   TEXT,
    UNIQUE(workspace_id, file_path)
);
CREATE INDEX IF NOT EXISTS idx_file_claims_workspace ON file_claims(workspace_id, agent_name);

-- Snapshots
CREATE TABLE IF NOT EXISTS snapshots (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    label         TEXT NOT NULL DEFAULT '',
    summary       TEXT NOT NULL DEFAULT '',
    content       TEXT NOT NULL DEFAULT '{}',
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_snapshots_workspace ON snapshots(workspace_id);
