-- Add review tracking columns to members
DO $$ BEGIN
    ALTER TABLE members ADD COLUMN accepted_tags BIGINT NOT NULL DEFAULT 0;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE members ADD COLUMN rejected_tags BIGINT NOT NULL DEFAULT 0;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

-- Track who approved a tag
DO $$ BEGIN
    ALTER TABLE player_tags ADD COLUMN reviewed_by BIGINT;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

-- Track accurate community review verdicts
DO $$ BEGIN
    ALTER TABLE members ADD COLUMN accurate_verdicts BIGINT NOT NULL DEFAULT 0;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;
