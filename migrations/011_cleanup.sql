-- Remove redundant index (UNIQUE constraint already creates one)
DROP INDEX IF EXISTS idx_blacklist_uuid;

-- Remove duplicate created_at columns (player_tags.added_on and player_snapshots.timestamp serve the same purpose)
DO $$ BEGIN
    ALTER TABLE player_tags DROP COLUMN created_at;
EXCEPTION WHEN undefined_column THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE player_snapshots DROP COLUMN created_at;
EXCEPTION WHEN undefined_column THEN NULL;
END $$;
