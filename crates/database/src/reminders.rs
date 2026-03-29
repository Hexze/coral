use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, FromRow)]
pub struct Reminder {
    pub id: i64,
    pub discord_id: i64,
    pub channel_id: i64,
    pub guild_id: Option<i64>,
    pub message: String,
    pub remind_at: DateTime<Utc>,
    pub repeat_interval: Option<i64>,
    pub created_at: DateTime<Utc>,
}

pub struct ReminderRepository<'a> {
    pool: &'a PgPool,
}

impl<'a> ReminderRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    pub async fn create(
        &self,
        discord_id: i64,
        channel_id: i64,
        guild_id: Option<i64>,
        message: &str,
        remind_at: DateTime<Utc>,
        repeat_interval: Option<i64>,
    ) -> Result<Reminder, sqlx::Error> {
        sqlx::query_as(
            r#"
            INSERT INTO reminders (discord_id, channel_id, guild_id, message, remind_at, repeat_interval)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, discord_id, channel_id, guild_id, message, remind_at, repeat_interval, created_at
            "#,
        )
        .bind(discord_id)
        .bind(channel_id)
        .bind(guild_id)
        .bind(message)
        .bind(remind_at)
        .bind(repeat_interval)
        .fetch_one(self.pool)
        .await
    }

    pub async fn get_due(&self) -> Result<Vec<Reminder>, sqlx::Error> {
        sqlx::query_as(
            r#"
            DELETE FROM reminders
            WHERE remind_at <= NOW() AND repeat_interval IS NULL
            RETURNING id, discord_id, channel_id, guild_id, message, remind_at, repeat_interval, created_at
            "#,
        )
        .fetch_all(self.pool)
        .await
    }

    pub async fn get_due_repeating(&self) -> Result<Vec<Reminder>, sqlx::Error> {
        sqlx::query_as(
            r#"
            UPDATE reminders
            SET remind_at = remind_at + (repeat_interval || ' seconds')::interval
            WHERE remind_at <= NOW() AND repeat_interval IS NOT NULL
            RETURNING id, discord_id, channel_id, guild_id, message,
                      remind_at - (repeat_interval || ' seconds')::interval AS remind_at,
                      repeat_interval, created_at
            "#,
        )
        .fetch_all(self.pool)
        .await
    }

    pub async fn list_by_user(&self, discord_id: i64) -> Result<Vec<Reminder>, sqlx::Error> {
        sqlx::query_as(
            r#"
            SELECT id, discord_id, channel_id, guild_id, message, remind_at, repeat_interval, created_at
            FROM reminders
            WHERE discord_id = $1
            ORDER BY remind_at ASC
            "#,
        )
        .bind(discord_id)
        .fetch_all(self.pool)
        .await
    }

    pub async fn delete(&self, id: i64, discord_id: i64) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            DELETE FROM reminders
            WHERE id = $1 AND discord_id = $2
            "#,
        )
        .bind(id)
        .bind(discord_id)
        .execute(self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn next_due_at(&self) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
        sqlx::query_scalar(
            r#"SELECT MIN(remind_at) FROM reminders"#,
        )
        .fetch_one(self.pool)
        .await
    }

    pub async fn snooze_once(
        &self,
        discord_id: i64,
        channel_id: i64,
        guild_id: Option<i64>,
        message: &str,
        seconds: i64,
    ) -> Result<Option<Reminder>, sqlx::Error> {
        sqlx::query_as(
            r#"
            INSERT INTO reminders (discord_id, channel_id, guild_id, message, remind_at)
            SELECT $1, $2, $3, $4, NOW() + ($5 || ' seconds')::interval
            WHERE NOT EXISTS (
                SELECT 1 FROM reminders
                WHERE discord_id = $1 AND channel_id = $2 AND message = $4
                  AND remind_at > NOW() AND repeat_interval IS NULL
            )
            RETURNING id, discord_id, channel_id, guild_id, message, remind_at, repeat_interval, created_at
            "#,
        )
        .bind(discord_id)
        .bind(channel_id)
        .bind(guild_id)
        .bind(message)
        .bind(seconds.to_string())
        .fetch_optional(self.pool)
        .await
    }
}
