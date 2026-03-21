-- Add tag history fields for soft deletes
DO $$ BEGIN
    ALTER TABLE player_tags ADD COLUMN removed_by BIGINT;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE player_tags ADD COLUMN removed_on TIMESTAMPTZ;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

CREATE INDEX IF NOT EXISTS idx_player_tags_active ON player_tags(player_id) WHERE removed_on IS NULL;
CREATE INDEX IF NOT EXISTS idx_player_tags_removed_by ON player_tags(removed_by) WHERE removed_by IS NOT NULL;
