CREATE TABLE IF NOT EXISTS budget_nodes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    parent_id UUID REFERENCES budget_nodes(id) ON DELETE CASCADE,
    node_type TEXT NOT NULL CHECK (node_type IN ('org', 'team', 'user', 'key', 'end_user')),
    node_id TEXT NOT NULL,
    daily_budget_usd DOUBLE PRECISION,
    monthly_budget_usd DOUBLE PRECISION,
    budget_action TEXT NOT NULL DEFAULT 'reject',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_budget_nodes_type_id ON budget_nodes(node_type, node_id);
CREATE INDEX IF NOT EXISTS idx_budget_nodes_parent ON budget_nodes(parent_id);
