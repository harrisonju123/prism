-- Workspaces
CREATE TABLE IF NOT EXISTS workspaces (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    metadata    TEXT,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

-- Initiatives
CREATE TABLE IF NOT EXISTS initiatives (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    description   TEXT NOT NULL DEFAULT '',
    status        TEXT NOT NULL DEFAULT 'active',
    metadata      TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_initiatives_workspace ON initiatives(workspace_id);

-- Epics
CREATE TABLE IF NOT EXISTS epics (
    id              TEXT PRIMARY KEY,
    initiative_id   TEXT NOT NULL REFERENCES initiatives(id) ON DELETE CASCADE,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'active',
    metadata        TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_epics_initiative ON epics(initiative_id);
CREATE INDEX IF NOT EXISTS idx_epics_workspace ON epics(workspace_id);

-- Tasks
CREATE TABLE IF NOT EXISTS tasks (
    id              TEXT PRIMARY KEY,
    epic_id         TEXT NOT NULL REFERENCES epics(id) ON DELETE CASCADE,
    initiative_id   TEXT NOT NULL REFERENCES initiatives(id) ON DELETE CASCADE,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    description     TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'backlog'
        CHECK(status IN ('backlog','todo','in_progress','in_review','done','cancelled')),
    priority        TEXT NOT NULL DEFAULT 'medium'
        CHECK(priority IN ('critical','high','medium','low')),
    assignee        TEXT NOT NULL DEFAULT '',
    domain_tags     TEXT NOT NULL DEFAULT '[]',
    metadata        TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tasks_epic ON tasks(epic_id);
CREATE INDEX IF NOT EXISTS idx_tasks_initiative ON tasks(initiative_id);
CREATE INDEX IF NOT EXISTS idx_tasks_workspace ON tasks(workspace_id);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(workspace_id, status);
CREATE INDEX IF NOT EXISTS idx_tasks_priority ON tasks(workspace_id, priority);
CREATE INDEX IF NOT EXISTS idx_tasks_assignee ON tasks(workspace_id, assignee)
    WHERE assignee != '';

-- Decisions (at least one parent required)
CREATE TABLE IF NOT EXISTS decisions (
    id              TEXT PRIMARY KEY,
    workspace_id    TEXT REFERENCES workspaces(id) ON DELETE CASCADE,
    initiative_id   TEXT REFERENCES initiatives(id) ON DELETE CASCADE,
    epic_id         TEXT REFERENCES epics(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    content         TEXT NOT NULL DEFAULT '',
    status          TEXT NOT NULL DEFAULT 'active',
    metadata        TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    CHECK (workspace_id IS NOT NULL OR initiative_id IS NOT NULL OR epic_id IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS idx_decisions_workspace ON decisions(workspace_id)
    WHERE workspace_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_initiative ON decisions(initiative_id)
    WHERE initiative_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_decisions_epic ON decisions(epic_id)
    WHERE epic_id IS NOT NULL;

-- Notes (exactly one parent required)
CREATE TABLE IF NOT EXISTS notes (
    id              TEXT PRIMARY KEY,
    workspace_id    TEXT REFERENCES workspaces(id) ON DELETE CASCADE,
    initiative_id   TEXT REFERENCES initiatives(id) ON DELETE CASCADE,
    epic_id         TEXT REFERENCES epics(id) ON DELETE CASCADE,
    task_id         TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    decision_id     TEXT REFERENCES decisions(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    content         TEXT NOT NULL DEFAULT '',
    metadata        TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    CHECK (
        (CASE WHEN workspace_id IS NOT NULL THEN 1 ELSE 0 END +
         CASE WHEN initiative_id IS NOT NULL THEN 1 ELSE 0 END +
         CASE WHEN epic_id IS NOT NULL THEN 1 ELSE 0 END +
         CASE WHEN task_id IS NOT NULL THEN 1 ELSE 0 END +
         CASE WHEN decision_id IS NOT NULL THEN 1 ELSE 0 END) = 1
    )
);
CREATE INDEX IF NOT EXISTS idx_notes_workspace ON notes(workspace_id)
    WHERE workspace_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_notes_initiative ON notes(initiative_id)
    WHERE initiative_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_notes_epic ON notes(epic_id)
    WHERE epic_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_notes_task ON notes(task_id)
    WHERE task_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_notes_decision ON notes(decision_id)
    WHERE decision_id IS NOT NULL;

-- API Keys
CREATE TABLE IF NOT EXISTS api_keys (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    key_hash      TEXT NOT NULL UNIQUE,
    key_prefix    TEXT NOT NULL,
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_api_keys_workspace ON api_keys(workspace_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash);

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
CREATE INDEX IF NOT EXISTS idx_activity_log_entity ON activity_log(entity_type, entity_id);

-- Task Dependencies
CREATE TABLE IF NOT EXISTS task_dependencies (
    id                TEXT PRIMARY KEY,
    blocking_task_id  TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    blocked_task_id   TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    workspace_id      TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    created_at        TEXT NOT NULL,
    UNIQUE (blocking_task_id, blocked_task_id),
    CHECK (blocking_task_id != blocked_task_id)
);
CREATE INDEX IF NOT EXISTS idx_task_deps_blocking ON task_dependencies(blocking_task_id);
CREATE INDEX IF NOT EXISTS idx_task_deps_blocked ON task_dependencies(blocked_task_id);
CREATE INDEX IF NOT EXISTS idx_task_deps_workspace ON task_dependencies(workspace_id);

-- Agents
CREATE TABLE IF NOT EXISTS agents (
    id            TEXT PRIMARY KEY,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    capabilities  TEXT NOT NULL DEFAULT '[]',
    current_task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    last_checkin  TEXT,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    UNIQUE (workspace_id, name)
);
CREATE INDEX IF NOT EXISTS idx_agents_workspace ON agents(workspace_id);

-- Agent Sessions
CREATE TABLE IF NOT EXISTS agent_sessions (
    id            TEXT PRIMARY KEY,
    agent_id      TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    started_at    TEXT NOT NULL,
    ended_at      TEXT,
    summary       TEXT NOT NULL DEFAULT '',
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_agent ON agent_sessions(agent_id);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_workspace ON agent_sessions(workspace_id);
CREATE INDEX IF NOT EXISTS idx_agent_sessions_open ON agent_sessions(agent_id) WHERE ended_at IS NULL;

-- Handoffs
CREATE TABLE IF NOT EXISTS handoffs (
    id            TEXT PRIMARY KEY,
    task_id       TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    agent_name    TEXT NOT NULL DEFAULT '',
    summary       TEXT NOT NULL DEFAULT '',
    findings      TEXT NOT NULL DEFAULT '[]',
    blockers      TEXT NOT NULL DEFAULT '[]',
    next_steps    TEXT NOT NULL DEFAULT '[]',
    artifacts     TEXT,
    created_at    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_handoffs_task ON handoffs(task_id);
CREATE INDEX IF NOT EXISTS idx_handoffs_workspace ON handoffs(workspace_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_handoffs_agent ON handoffs(workspace_id, agent_name)
    WHERE agent_name != '';
