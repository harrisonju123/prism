ALTER TABLE virtual_keys ADD COLUMN IF NOT EXISTS rotation_interval_days INTEGER;
ALTER TABLE virtual_keys ADD COLUMN IF NOT EXISTS rotated_from UUID;
ALTER TABLE virtual_keys ADD COLUMN IF NOT EXISTS grace_period_hours INTEGER DEFAULT 24;
