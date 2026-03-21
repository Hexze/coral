DO $$ BEGIN
    ALTER TABLE guild_config ADD COLUMN unlinked_role_id BIGINT;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;
