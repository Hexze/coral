use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};


#[derive(Debug, Clone, FromRow)]
pub struct SessionMarker {
    pub id: i64,
    pub uuid: String,
    pub discord_id: i64,
    pub name: String,
    pub snapshot_timestamp: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}


pub struct SessionRepository<'a> {
    pool: &'a PgPool,
}


impl<'a> SessionRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self { Self { pool } }

    pub async fn create(
        &self,
        uuid: &str,
        discord_id: i64,
        name: &str,
        snapshot_timestamp: DateTime<Utc>,
    ) -> Result<SessionMarker, sqlx::Error> {
        sqlx::query_as(
            "INSERT INTO session_markers (uuid, discord_id, name, snapshot_timestamp)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (uuid, discord_id, name) DO UPDATE
             SET snapshot_timestamp = $4, created_at = NOW()
             RETURNING id, uuid, discord_id, name, snapshot_timestamp, created_at",
        )
        .bind(uuid)
        .bind(discord_id)
        .bind(name)
        .bind(snapshot_timestamp)
        .fetch_one(self.pool)
        .await
    }

    pub async fn get(&self, uuid: &str, discord_id: i64, name: &str) -> Result<Option<SessionMarker>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, uuid, discord_id, name, snapshot_timestamp, created_at
             FROM session_markers WHERE uuid = $1 AND discord_id = $2 AND name = $3",
        )
        .bind(uuid)
        .bind(discord_id)
        .bind(name)
        .fetch_optional(self.pool)
        .await
    }

    pub async fn list(&self, uuid: &str, discord_id: i64) -> Result<Vec<SessionMarker>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, uuid, discord_id, name, snapshot_timestamp, created_at
             FROM session_markers WHERE uuid = $1 AND discord_id = $2
             ORDER BY created_at DESC",
        )
        .bind(uuid)
        .bind(discord_id)
        .fetch_all(self.pool)
        .await
    }

    pub async fn delete(&self, uuid: &str, discord_id: i64, name: &str) -> Result<bool, sqlx::Error> {
        sqlx::query("DELETE FROM session_markers WHERE uuid = $1 AND discord_id = $2 AND name = $3")
            .bind(uuid)
            .bind(discord_id)
            .bind(name)
            .execute(self.pool)
            .await
            .map(|r| r.rows_affected() > 0)
    }

    pub async fn rename(&self, uuid: &str, discord_id: i64, old_name: &str, new_name: &str) -> Result<bool, sqlx::Error> {
        sqlx::query("UPDATE session_markers SET name = $4 WHERE uuid = $1 AND discord_id = $2 AND name = $3")
            .bind(uuid)
            .bind(discord_id)
            .bind(old_name)
            .bind(new_name)
            .execute(self.pool)
            .await
            .map(|r| r.rows_affected() > 0)
    }
}
