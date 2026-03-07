CREATE TABLE IF NOT EXISTS virtual_keys (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT NOT NULL,
    key_hash        TEXT NOT NULL UNIQUE,
    key_prefix      TEXT NOT NULL,
    team_id         TEXT,
    is_active       BOOLEAN NOT NULL DEFAULT TRUE,
    rpm_limit       INTEGER,
    tpm_limit       INTEGER,
    daily_budget_usd    DOUBLE PRECISION,
    monthly_budget_usd  DOUBLE PRECISION,
    budget_action       TEXT NOT NULL DEFAULT 'reject',
    allowed_models  TEXT[] NOT NULL DEFAULT '{}',
    metadata        JSONB NOT NULL DEFAULT '{}',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at      TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_vk_key_hash ON virtual_keys (key_hash);
CREATE INDEX IF NOT EXISTS idx_vk_team_id ON virtual_keys (team_id) WHERE team_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_vk_active ON virtual_keys (is_active) WHERE is_active = TRUE;
