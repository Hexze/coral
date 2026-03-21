-- Coral Database Schema
-- Initial migration

-- Members table (users, API keys, access levels)
CREATE TABLE IF NOT EXISTS members (
    id BIGSERIAL PRIMARY KEY,
    discord_id BIGINT NOT NULL UNIQUE,
    uuid VARCHAR(32),
    api_key VARCHAR(64) UNIQUE,
    join_date TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    request_count BIGINT NOT NULL DEFAULT 0,

    -- Access levels
    is_admin BOOLEAN NOT NULL DEFAULT FALSE,
    is_mod BOOLEAN NOT NULL DEFAULT FALSE,
    is_private BOOLEAN NOT NULL DEFAULT FALSE,
    is_beta BOOLEAN NOT NULL DEFAULT FALSE,

    -- Status
    key_locked BOOLEAN NOT NULL DEFAULT FALSE,

    -- User config (stored as JSON for flexibility)
    config JSONB NOT NULL DEFAULT '{}',

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_members_uuid ON members(uuid) WHERE uuid IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_members_api_key ON members(api_key) WHERE api_key IS NOT NULL;

-- Blacklisted players
CREATE TABLE IF NOT EXISTS blacklist_players (
    id BIGSERIAL PRIMARY KEY,
    uuid VARCHAR(32) NOT NULL UNIQUE,
    is_locked BOOLEAN NOT NULL DEFAULT FALSE,
    lock_reason TEXT,
    locked_by BIGINT,
    locked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_blacklist_uuid ON blacklist_players(uuid);

-- Player tags (normalized from embedded array)
CREATE TABLE IF NOT EXISTS player_tags (
    id BIGSERIAL PRIMARY KEY,
    player_id BIGINT NOT NULL REFERENCES blacklist_players(id) ON DELETE CASCADE,
    tag_type VARCHAR(32) NOT NULL,
    reason TEXT NOT NULL,
    added_by BIGINT NOT NULL,
    added_on TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    hide_username BOOLEAN NOT NULL DEFAULT FALSE,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_player_tags_player ON player_tags(player_id);
CREATE INDEX IF NOT EXISTS idx_player_tags_added_by ON player_tags(added_by);
CREATE INDEX IF NOT EXISTS idx_player_tags_added_on ON player_tags(added_on);

-- Player snapshots (stats cache with smart compression)
-- We store full snapshots periodically, and deltas between them
CREATE TABLE IF NOT EXISTS player_snapshots (
    id BIGSERIAL PRIMARY KEY,
    uuid VARCHAR(32) NOT NULL,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    discord_id BIGINT,
    source VARCHAR(32),
    username VARCHAR(16),

    -- If is_baseline is true, data contains full snapshot
    -- If false, data contains only changed fields (delta)
    is_baseline BOOLEAN NOT NULL DEFAULT FALSE,
    data JSONB NOT NULL,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_snapshots_uuid ON player_snapshots(uuid);
CREATE INDEX IF NOT EXISTS idx_snapshots_uuid_timestamp ON player_snapshots(uuid, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_snapshots_timestamp ON player_snapshots(timestamp DESC);

-- Rate limiting
CREATE TABLE IF NOT EXISTS rate_limits (
    id BIGSERIAL PRIMARY KEY,
    api_key VARCHAR(64) NOT NULL UNIQUE,
    requests TIMESTAMPTZ[] NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- IP history for API keys
CREATE TABLE IF NOT EXISTS api_key_ips (
    id BIGSERIAL PRIMARY KEY,
    member_id BIGINT NOT NULL REFERENCES members(id) ON DELETE CASCADE,
    ip_address INET NOT NULL,
    first_seen TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_seen TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(member_id, ip_address)
);

CREATE INDEX IF NOT EXISTS idx_api_key_ips_member ON api_key_ips(member_id);

-- Minecraft alt accounts
CREATE TABLE IF NOT EXISTS minecraft_accounts (
    id BIGSERIAL PRIMARY KEY,
    member_id BIGINT NOT NULL REFERENCES members(id) ON DELETE CASCADE,
    uuid VARCHAR(32) NOT NULL,
    added_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(member_id, uuid)
);

CREATE INDEX IF NOT EXISTS idx_minecraft_accounts_member ON minecraft_accounts(member_id);
CREATE INDEX IF NOT EXISTS idx_minecraft_accounts_uuid ON minecraft_accounts(uuid);

-- Function to update updated_at timestamp
CREATE OR REPLACE FUNCTION update_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Apply updated_at triggers
DROP TRIGGER IF EXISTS members_updated_at ON members;
CREATE TRIGGER members_updated_at
    BEFORE UPDATE ON members
    FOR EACH ROW EXECUTE FUNCTION update_updated_at();

DROP TRIGGER IF EXISTS blacklist_players_updated_at ON blacklist_players;
CREATE TRIGGER blacklist_players_updated_at
    BEFORE UPDATE ON blacklist_players
    FOR EACH ROW EXECUTE FUNCTION update_updated_at();
