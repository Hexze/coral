CREATE TABLE reminders (
    id BIGSERIAL PRIMARY KEY,
    discord_id BIGINT NOT NULL,
    channel_id BIGINT NOT NULL,
    guild_id BIGINT,
    message TEXT NOT NULL,
    remind_at TIMESTAMPTZ NOT NULL,
    repeat_interval BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_reminders_pending ON reminders (remind_at);
CREATE INDEX idx_reminders_user ON reminders (discord_id);
