CREATE INDEX IF NOT EXISTS idx_snapshots_username_lower ON player_snapshots(LOWER(username), timestamp DESC);
