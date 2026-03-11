CREATE TABLE IF NOT EXISTS debug_sessions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    title           TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'open',
    symptom         JSONB NOT NULL DEFAULT '{}'::jsonb,
    metadata        JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS debug_hypotheses (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      UUID NOT NULL REFERENCES debug_sessions(id) ON DELETE CASCADE,
    rank            INTEGER NOT NULL DEFAULT 0,
    statement       TEXT NOT NULL,
    confidence      DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    evidence        JSONB NOT NULL DEFAULT '[]'::jsonb,
    status          TEXT NOT NULL DEFAULT 'active',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS debug_experiments (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id      UUID NOT NULL REFERENCES debug_sessions(id) ON DELETE CASCADE,
    hypothesis_id   UUID REFERENCES debug_hypotheses(id) ON DELETE SET NULL,
    title           TEXT NOT NULL,
    description     TEXT,
    cost_level      TEXT NOT NULL DEFAULT 'medium',
    impact_level    TEXT NOT NULL DEFAULT 'medium',
    status          TEXT NOT NULL DEFAULT 'proposed',
    params          JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS debug_runs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    experiment_id   UUID NOT NULL REFERENCES debug_experiments(id) ON DELETE CASCADE,
    status          TEXT NOT NULL DEFAULT 'queued',
    started_at      TIMESTAMPTZ,
    finished_at     TIMESTAMPTZ,
    duration_ms     INTEGER,
    output          TEXT,
    artifacts       JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_debug_sessions_status ON debug_sessions(status);
CREATE INDEX IF NOT EXISTS idx_debug_hypotheses_session ON debug_hypotheses(session_id);
CREATE INDEX IF NOT EXISTS idx_debug_experiments_session ON debug_experiments(session_id);
CREATE INDEX IF NOT EXISTS idx_debug_runs_experiment ON debug_runs(experiment_id);
