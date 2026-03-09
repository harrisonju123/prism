CREATE TABLE IF NOT EXISTS audit_events (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_type  TEXT NOT NULL,
    key_id      UUID,
    key_prefix  TEXT,
    actor       TEXT,
    details     JSONB NOT NULL DEFAULT '{}',
    ip_addr     TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_audit_events_key_id  ON audit_events (key_id) WHERE key_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_audit_events_type    ON audit_events (event_type);
CREATE INDEX IF NOT EXISTS idx_audit_events_created ON audit_events (created_at);
