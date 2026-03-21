CREATE TABLE IF NOT EXISTS guild_config (
    id BIGSERIAL PRIMARY KEY,
    guild_id BIGINT NOT NULL UNIQUE,
    link_role_id BIGINT,
    nickname_template VARCHAR(256),
    link_channel_id BIGINT,
    link_message_id BIGINT,
    configured_by BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS guild_role_rules (
    id BIGSERIAL PRIMARY KEY,
    guild_id BIGINT NOT NULL REFERENCES guild_config(guild_id) ON DELETE CASCADE,
    role_id BIGINT NOT NULL,
    condition TEXT NOT NULL,
    priority INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_guild_config_guild ON guild_config(guild_id);
CREATE INDEX IF NOT EXISTS idx_guild_role_rules_guild ON guild_role_rules(guild_id);
