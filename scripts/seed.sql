-- PrisM seed data for local development
-- Applied on postgres container startup via docker-compose healthcheck init.

-- Run migrations first (idempotent)
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

-- Default development virtual key.
-- Plaintext key: prism_devkey00000000000000000000
-- SHA-256 hash of the plaintext key (precomputed):
--   echo -n "prism_devkey00000000000000000000" | sha256sum
--   => 7c89a27e5bb4d1cc1b5cc0ff5fa85fb41deb7819a476a39dbb5ab8dbb5c6b9d3
--
-- To use in development: Authorization: Bearer prism_devkey00000000000000000000
INSERT INTO virtual_keys (
    id,
    name,
    key_hash,
    key_prefix,
    team_id,
    is_active,
    rpm_limit,
    tpm_limit,
    daily_budget_usd,
    monthly_budget_usd,
    budget_action,
    allowed_models,
    metadata
) VALUES (
    '00000000-0000-0000-0000-000000000001',
    'dev-default',
    '7c89a27e5bb4d1cc1b5cc0ff5fa85fb41deb7819a476a39dbb5ab8dbb5c6b9d3',
    'prism_devk',
    'dev',
    TRUE,
    1000,
    500000,
    10.0,
    100.0,
    'warn',
    '{}',
    '{"description": "Default dev key — do not use in production"}'
) ON CONFLICT (key_hash) DO NOTHING;
