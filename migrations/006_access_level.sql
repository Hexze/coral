DO $$ BEGIN
    ALTER TABLE members ADD COLUMN access_level SMALLINT NOT NULL DEFAULT 0;
EXCEPTION WHEN duplicate_column THEN NULL;
END $$;

DO $$ BEGIN
    UPDATE members SET access_level = CASE
        WHEN is_admin THEN 3
        WHEN is_mod THEN 2
        WHEN is_private THEN 1
        ELSE 0
    END
    WHERE access_level = 0;
EXCEPTION WHEN undefined_column THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE members DROP COLUMN is_admin;
EXCEPTION WHEN undefined_column THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE members DROP COLUMN is_mod;
EXCEPTION WHEN undefined_column THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE members DROP COLUMN is_beta;
EXCEPTION WHEN undefined_column THEN NULL;
END $$;

DO $$ BEGIN
    ALTER TABLE members DROP COLUMN is_private;
EXCEPTION WHEN undefined_column THEN NULL;
END $$;
