DO $$ BEGIN
    ALTER TABLE blacklist_players ADD COLUMN evidence_thread TEXT;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;
