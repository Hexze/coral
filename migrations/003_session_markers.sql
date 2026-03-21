CREATE TABLE IF NOT EXISTS session_markers (
    id BIGSERIAL PRIMARY KEY,
    uuid VARCHAR(32) NOT NULL,
    discord_id BIGINT NOT NULL,
    name VARCHAR(64) NOT NULL,
    snapshot_timestamp TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_session_markers_uuid ON session_markers(uuid);
CREATE INDEX IF NOT EXISTS idx_session_markers_discord ON session_markers(discord_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_session_markers_unique ON session_markers(uuid, discord_id, name);
