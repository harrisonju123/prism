-- Enterprise RBAC: tenant users with role-based access control
CREATE TABLE IF NOT EXISTS tenant_users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    team_id TEXT NOT NULL,
    email TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('admin', 'operator', 'analyst', 'viewer')),
    active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (team_id, email)
);

CREATE INDEX IF NOT EXISTS idx_tenant_users_team_id ON tenant_users(team_id);
CREATE INDEX IF NOT EXISTS idx_tenant_users_email ON tenant_users(email);
