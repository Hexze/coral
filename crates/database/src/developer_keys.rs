use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};


pub mod permissions {
    pub const PLAYER_DATA: i64 = 1 << 0;
    pub const GUILD: i64 = 1 << 1;
    pub const ALL_SESSIONS: i64 = 1 << 2;
    pub const RESOLVE: i64 = 1 << 3;

    pub fn label(perm: i64) -> &'static str {
        match perm {
            PLAYER_DATA => "Player Data",
            GUILD => "Guild",
            ALL_SESSIONS => "All Sessions",
            RESOLVE => "Resolve",
            _ => "Unknown",
        }
    }

    pub const ALL: &[i64] = &[PLAYER_DATA, GUILD, ALL_SESSIONS, RESOLVE];
}


#[derive(Debug, Clone, FromRow)]
pub struct DeveloperKey {
    pub id: i64,
    pub member_id: i64,
    pub api_key: String,
    pub label: String,
    pub permissions: i64,
    pub rate_limit: i32,
    pub request_count: i64,
    pub locked: bool,
    pub created_at: DateTime<Utc>,
}


impl DeveloperKey {
    pub fn has_permission(&self, perm: i64) -> bool { self.permissions & perm != 0 }
}


pub struct DeveloperKeyRepository<'a> {
    pool: &'a PgPool,
}


impl<'a> DeveloperKeyRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self { Self { pool } }

    pub async fn get_by_api_key(&self, api_key: &str) -> Result<Option<DeveloperKey>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, member_id, api_key, label, permissions, rate_limit, request_count, locked, created_at
             FROM developer_keys WHERE api_key = $1",
        )
        .bind(api_key)
        .fetch_optional(self.pool)
        .await
    }

    pub async fn get_by_member_id(&self, member_id: i64) -> Result<Option<DeveloperKey>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, member_id, api_key, label, permissions, rate_limit, request_count, locked, created_at
             FROM developer_keys WHERE member_id = $1",
        )
        .bind(member_id)
        .fetch_optional(self.pool)
        .await
    }

    pub async fn create(
        &self, member_id: i64, api_key: &str, label: &str, permissions: i64, rate_limit: i32,
    ) -> Result<DeveloperKey, sqlx::Error> {
        sqlx::query_as(
            "INSERT INTO developer_keys (member_id, api_key, label, permissions, rate_limit)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id, member_id, api_key, label, permissions, rate_limit, request_count, locked, created_at",
        )
        .bind(member_id)
        .bind(api_key)
        .bind(label)
        .bind(permissions)
        .bind(rate_limit)
        .fetch_one(self.pool)
        .await
    }

    pub async fn set_api_key(&self, member_id: i64, api_key: &str) -> Result<bool, sqlx::Error> {
        sqlx::query("UPDATE developer_keys SET api_key = $2 WHERE member_id = $1")
            .bind(member_id)
            .bind(api_key)
            .execute(self.pool)
            .await
            .map(|r| r.rows_affected() > 0)
    }

    pub async fn set_locked(&self, member_id: i64, locked: bool) -> Result<bool, sqlx::Error> {
        sqlx::query("UPDATE developer_keys SET locked = $2 WHERE member_id = $1")
            .bind(member_id)
            .bind(locked)
            .execute(self.pool)
            .await
            .map(|r| r.rows_affected() > 0)
    }

    pub async fn set_permissions(&self, member_id: i64, permissions: i64) -> Result<bool, sqlx::Error> {
        sqlx::query("UPDATE developer_keys SET permissions = $2 WHERE member_id = $1")
            .bind(member_id)
            .bind(permissions)
            .execute(self.pool)
            .await
            .map(|r| r.rows_affected() > 0)
    }

    pub async fn set_rate_limit(&self, member_id: i64, rate_limit: i32) -> Result<bool, sqlx::Error> {
        sqlx::query("UPDATE developer_keys SET rate_limit = $2 WHERE member_id = $1")
            .bind(member_id)
            .bind(rate_limit)
            .execute(self.pool)
            .await
            .map(|r| r.rows_affected() > 0)
    }

    pub async fn delete(&self, member_id: i64) -> Result<bool, sqlx::Error> {
        sqlx::query("DELETE FROM developer_keys WHERE member_id = $1")
            .bind(member_id)
            .execute(self.pool)
            .await
            .map(|r| r.rows_affected() > 0)
    }

    pub async fn increment_request_count(&self, api_key: &str) -> Result<(), sqlx::Error> {
        sqlx::query("UPDATE developer_keys SET request_count = request_count + 1 WHERE api_key = $1")
            .bind(api_key)
            .execute(self.pool)
            .await?;
        Ok(())
    }
}
