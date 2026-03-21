CREATE INDEX IF NOT EXISTS idx_snapshots_discord_username
ON player_snapshots (LOWER(data->'socialMedia'->'links'->>'DISCORD'))
WHERE is_baseline = true
  AND data->'socialMedia'->'links'->>'DISCORD' IS NOT NULL;
