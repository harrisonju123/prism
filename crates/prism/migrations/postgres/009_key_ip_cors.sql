-- Migration 009: IP allowlist and CORS per virtual key
-- allowed_ips: JSON array of CIDR strings, null = allow all
-- allowed_origins: JSON array of origin strings, null = allow all
ALTER TABLE virtual_keys ADD COLUMN IF NOT EXISTS allowed_ips TEXT;
ALTER TABLE virtual_keys ADD COLUMN IF NOT EXISTS allowed_origins TEXT;
