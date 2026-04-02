-- Clean up any existing duplicates before adding constraint
DELETE FROM player_tags a
USING player_tags b
WHERE a.player_id = b.player_id
  AND a.tag_type = b.tag_type
  AND a.removed_on IS NULL
  AND b.removed_on IS NULL
  AND a.id > b.id;

-- One active tag per type per player
CREATE UNIQUE INDEX IF NOT EXISTS idx_player_tags_unique_active
    ON player_tags(player_id, tag_type) WHERE removed_on IS NULL;

-- Optional expiration for tags (e.g. replays_needed)
DO $$ BEGIN
    ALTER TABLE player_tags ADD COLUMN expires_at TIMESTAMPTZ;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

CREATE INDEX IF NOT EXISTS idx_player_tags_expires ON player_tags(expires_at)
    WHERE expires_at IS NOT NULL AND removed_on IS NULL;
