-- Expand access levels: add Helper (2), shift Mod->Moderator (3), Admin (4)
-- Update descending to avoid unique constraint issues

UPDATE members SET access_level = 4 WHERE access_level = 3;
UPDATE members SET access_level = 3 WHERE access_level = 2;

DO $$ BEGIN
    ALTER TABLE members ADD COLUMN tagging_disabled BOOLEAN NOT NULL DEFAULT FALSE;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;
