use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};


#[derive(Debug, Clone, FromRow)]
pub struct BlacklistPlayer {
    pub id: i64,
    pub uuid: String,
    pub is_locked: bool,
    pub lock_reason: Option<String>,
    pub locked_by: Option<i64>,
    pub locked_at: Option<DateTime<Utc>>,
    pub evidence_thread: Option<String>,
}


#[derive(Debug, Clone, FromRow)]
pub struct PlayerTagRow {
    pub id: i64,
    pub player_id: i64,
    pub tag_type: String,
    pub reason: String,
    pub added_by: i64,
    pub added_on: DateTime<Utc>,
    pub hide_username: bool,
    pub reviewed_by: Option<Vec<i64>>,
    pub removed_by: Option<i64>,
    pub removed_on: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}


const COLS: &str = "\
    pt.id, pt.player_id, pt.tag_type, pt.reason, pt.added_by, pt.added_on, \
    pt.hide_username, pt.reviewed_by, pt.removed_by, pt.removed_on, pt.expires_at";

const ACTIVE: &str = "pt.removed_on IS NULL AND (pt.expires_at IS NULL OR pt.expires_at > NOW())";


pub struct BlacklistRepository<'a> {
    pool: &'a PgPool,
}


impl<'a> BlacklistRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self { Self { pool } }

    pub async fn get_player(&self, uuid: &str) -> Result<Option<BlacklistPlayer>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, uuid, is_locked, lock_reason, locked_by, locked_at, evidence_thread
             FROM blacklist_players WHERE uuid = $1",
        )
        .bind(uuid)
        .fetch_optional(self.pool)
        .await
    }

    pub async fn get_or_create_player(&self, uuid: &str) -> Result<BlacklistPlayer, sqlx::Error> {
        if let Some(player) = self.get_player(uuid).await? {
            return Ok(player);
        }
        sqlx::query_as(
            "INSERT INTO blacklist_players (uuid) VALUES ($1)
             ON CONFLICT (uuid) DO UPDATE SET uuid = EXCLUDED.uuid
             RETURNING id, uuid, is_locked, lock_reason, locked_by, locked_at, evidence_thread",
        )
        .bind(uuid)
        .fetch_one(self.pool)
        .await
    }

    pub async fn get_tags(&self, uuid: &str) -> Result<Vec<PlayerTagRow>, sqlx::Error> {
        sqlx::query_as(&format!(
            "SELECT {COLS}
             FROM player_tags pt
             JOIN blacklist_players bp ON bp.id = pt.player_id
             WHERE bp.uuid = $1 AND {ACTIVE}
             ORDER BY pt.added_on DESC",
        ))
        .bind(uuid)
        .fetch_all(self.pool)
        .await
    }

    pub async fn get_tag_by_type(&self, uuid: &str, tag_type: &str) -> Result<Option<PlayerTagRow>, sqlx::Error> {
        sqlx::query_as(&format!(
            "SELECT {COLS}
             FROM player_tags pt
             JOIN blacklist_players bp ON bp.id = pt.player_id
             WHERE bp.uuid = $1 AND pt.tag_type = $2 AND {ACTIVE}",
        ))
        .bind(uuid)
        .bind(tag_type)
        .fetch_optional(self.pool)
        .await
    }

    pub async fn get_uuid_by_player_id(&self, player_id: i64) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_as::<_, (String,)>("SELECT uuid FROM blacklist_players WHERE id = $1")
            .bind(player_id)
            .fetch_optional(self.pool)
            .await
            .map(|r| r.map(|(uuid,)| uuid))
    }

    pub async fn get_tag_by_id(&self, tag_id: i64) -> Result<Option<PlayerTagRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, player_id, tag_type, reason, added_by, added_on,
                    hide_username, reviewed_by, removed_by, removed_on, expires_at
             FROM player_tags WHERE id = $1
               AND removed_on IS NULL AND (expires_at IS NULL OR expires_at > NOW())",
        )
        .bind(tag_id)
        .fetch_optional(self.pool)
        .await
    }

    pub async fn get_tag_history(&self, uuid: &str) -> Result<Vec<PlayerTagRow>, sqlx::Error> {
        sqlx::query_as(&format!(
            "SELECT {COLS}
             FROM player_tags pt
             JOIN blacklist_players bp ON bp.id = pt.player_id
             WHERE bp.uuid = $1
             ORDER BY pt.added_on DESC",
        ))
        .bind(uuid)
        .fetch_all(self.pool)
        .await
    }

    pub async fn add_tag(
        &self,
        uuid: &str,
        tag_type: &str,
        reason: &str,
        added_by: i64,
        hide_username: bool,
        reviewed_by: Option<&[i64]>,
    ) -> Result<i64, sqlx::Error> {
        self.add_tag_with_expiry(uuid, tag_type, reason, added_by, hide_username, reviewed_by, None).await
    }

    pub async fn add_tag_with_expiry(
        &self,
        uuid: &str,
        tag_type: &str,
        reason: &str,
        added_by: i64,
        hide_username: bool,
        reviewed_by: Option<&[i64]>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<i64, sqlx::Error> {
        let player = self.get_or_create_player(uuid).await?;
        let (id,): (i64,) = sqlx::query_as(
            "INSERT INTO player_tags (player_id, tag_type, reason, added_by, hide_username, reviewed_by, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             RETURNING id",
        )
        .bind(player.id)
        .bind(tag_type)
        .bind(reason)
        .bind(added_by)
        .bind(hide_username)
        .bind(reviewed_by)
        .bind(expires_at)
        .fetch_one(self.pool)
        .await?;
        Ok(id)
    }

    pub async fn remove_tag(&self, tag_id: i64, removed_by: i64) -> Result<bool, sqlx::Error> {
        sqlx::query(
            "UPDATE player_tags SET removed_by = $2, removed_on = NOW()
             WHERE id = $1 AND removed_on IS NULL",
        )
        .bind(tag_id)
        .bind(removed_by)
        .execute(self.pool)
        .await
        .map(|r| r.rows_affected() > 0)
    }

    pub async fn remove_tag_by_type(&self, uuid: &str, tag_type: &str, removed_by: i64) -> Result<bool, sqlx::Error> {
        sqlx::query(
            "UPDATE player_tags SET removed_by = $3, removed_on = NOW()
             FROM blacklist_players bp
             WHERE player_tags.player_id = bp.id AND bp.uuid = $1
               AND player_tags.tag_type = $2
               AND player_tags.removed_on IS NULL
               AND (player_tags.expires_at IS NULL OR player_tags.expires_at > NOW())",
        )
        .bind(uuid)
        .bind(tag_type)
        .bind(removed_by)
        .execute(self.pool)
        .await
        .map(|r| r.rows_affected() > 0)
    }

    pub async fn update_tag(
        &self,
        uuid: &str,
        tag_type: &str,
        new_reason: Option<&str>,
        new_hide: Option<bool>,
    ) -> Result<Option<PlayerTagRow>, sqlx::Error> {
        sqlx::query_as(
            "UPDATE player_tags SET
                reason = COALESCE($3, player_tags.reason),
                hide_username = COALESCE($4, player_tags.hide_username)
             FROM blacklist_players bp
             WHERE player_tags.player_id = bp.id AND bp.uuid = $1
               AND player_tags.tag_type = $2
               AND player_tags.removed_on IS NULL
               AND (player_tags.expires_at IS NULL OR player_tags.expires_at > NOW())
             RETURNING player_tags.id, player_tags.player_id, player_tags.tag_type,
                       player_tags.reason, player_tags.added_by, player_tags.added_on,
                       player_tags.hide_username, player_tags.reviewed_by,
                       player_tags.removed_by, player_tags.removed_on, player_tags.expires_at",
        )
        .bind(uuid)
        .bind(tag_type)
        .bind(new_reason)
        .bind(new_hide)
        .fetch_optional(self.pool)
        .await
    }

    pub async fn modify_tag(
        &self,
        tag_id: i64,
        tag_type: Option<&str>,
        reason: Option<&str>,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            "UPDATE player_tags
             SET tag_type = COALESCE($2, tag_type), reason = COALESCE($3, reason)
             WHERE id = $1 AND removed_on IS NULL",
        )
        .bind(tag_id)
        .bind(tag_type)
        .bind(reason)
        .execute(self.pool)
        .await
        .map(|r| r.rows_affected() > 0)
    }

    pub async fn modify_tag_full(
        &self,
        tag_id: i64,
        tag_type: Option<&str>,
        reason: Option<&str>,
        added_by: Option<i64>,
        hide_username: Option<bool>,
        reset_time: bool,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query(
            "UPDATE player_tags
             SET tag_type = COALESCE($2, tag_type), reason = COALESCE($3, reason),
                 added_by = COALESCE($4, added_by), hide_username = COALESCE($5, hide_username),
                 added_on = CASE WHEN $6 THEN NOW() ELSE added_on END
             WHERE id = $1 AND removed_on IS NULL",
        )
        .bind(tag_id)
        .bind(tag_type)
        .bind(reason)
        .bind(added_by)
        .bind(hide_username)
        .bind(reset_time)
        .execute(self.pool)
        .await
        .map(|r| r.rows_affected() > 0)
    }

    pub async fn lock_player(&self, uuid: &str, reason: &str, locked_by: i64) -> Result<bool, sqlx::Error> {
        let player = self.get_or_create_player(uuid).await?;
        sqlx::query(
            "UPDATE blacklist_players
             SET is_locked = true, lock_reason = $2, locked_by = $3, locked_at = NOW()
             WHERE id = $1",
        )
        .bind(player.id)
        .bind(reason)
        .bind(locked_by)
        .execute(self.pool)
        .await
        .map(|r| r.rows_affected() > 0)
    }

    pub async fn unlock_player(&self, uuid: &str) -> Result<bool, sqlx::Error> {
        sqlx::query(
            "UPDATE blacklist_players
             SET is_locked = false, lock_reason = NULL, locked_by = NULL, locked_at = NULL
             WHERE uuid = $1",
        )
        .bind(uuid)
        .execute(self.pool)
        .await
        .map(|r| r.rows_affected() > 0)
    }

    pub async fn get_players_batch(&self, uuids: &[String]) -> Result<Vec<(String, Vec<PlayerTagRow>)>, sqlx::Error> {
        let tags: Vec<PlayerTagRow> = sqlx::query_as(&format!(
            "SELECT {COLS}
             FROM player_tags pt
             JOIN blacklist_players bp ON bp.id = pt.player_id
             WHERE bp.uuid = ANY($1) AND {ACTIVE}
             ORDER BY bp.uuid, pt.added_on DESC",
        ))
        .bind(uuids)
        .fetch_all(self.pool)
        .await?;

        let players: Vec<BlacklistPlayer> = sqlx::query_as(
            "SELECT id, uuid, is_locked, lock_reason, locked_by, locked_at, evidence_thread
             FROM blacklist_players WHERE uuid = ANY($1)",
        )
        .bind(uuids)
        .fetch_all(self.pool)
        .await?;

        Ok(players
            .into_iter()
            .map(|p| {
                let player_tags = tags.iter().filter(|t| t.player_id == p.id).cloned().collect();
                (p.uuid, player_tags)
            })
            .collect())
    }

    pub async fn count_players(&self) -> Result<i64, sqlx::Error> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM blacklist_players")
            .fetch_one(self.pool).await?;
        Ok(count)
    }

    pub async fn count_active_tags(&self) -> Result<i64, sqlx::Error> {
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM player_tags WHERE removed_on IS NULL AND (expires_at IS NULL OR expires_at > NOW())"
        ).fetch_one(self.pool).await?;
        Ok(count)
    }

    pub async fn count_tags_by_type(&self) -> Result<Vec<(String, i64)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT tag_type, COUNT(*) as count FROM player_tags
             WHERE removed_on IS NULL AND (expires_at IS NULL OR expires_at > NOW())
             GROUP BY tag_type ORDER BY count DESC"
        ).fetch_all(self.pool).await
    }

    pub async fn count_tags_by_user(&self, discord_id: i64) -> Result<i64, sqlx::Error> {
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM player_tags WHERE added_by = $1")
            .bind(discord_id)
            .fetch_one(self.pool)
            .await?;
        Ok(count)
    }

    pub async fn cleanup_expired_tags(&self) -> Result<u64, sqlx::Error> {
        sqlx::query(
            "UPDATE player_tags SET removed_on = NOW()
             WHERE expires_at IS NOT NULL AND expires_at <= NOW() AND removed_on IS NULL",
        )
        .execute(self.pool)
        .await
        .map(|r| r.rows_affected())
    }

    pub async fn set_evidence_thread(&self, uuid: &str, thread_url: &str) -> Result<bool, sqlx::Error> {
        sqlx::query("UPDATE blacklist_players SET evidence_thread = $2 WHERE uuid = $1")
            .bind(uuid)
            .bind(thread_url)
            .execute(self.pool)
            .await
            .map(|r| r.rows_affected() > 0)
    }

    pub async fn clear_evidence_thread(&self, uuid: &str) -> Result<bool, sqlx::Error> {
        sqlx::query("UPDATE blacklist_players SET evidence_thread = NULL WHERE uuid = $1")
            .bind(uuid)
            .execute(self.pool)
            .await
            .map(|r| r.rows_affected() > 0)
    }

    pub async fn convert_tag_to_confirmed(&self, tag_id: i64) -> Result<bool, sqlx::Error> {
        sqlx::query("UPDATE player_tags SET tag_type = 'confirmed_cheater' WHERE id = $1 AND removed_on IS NULL")
            .bind(tag_id)
            .execute(self.pool)
            .await
            .map(|r| r.rows_affected() > 0)
    }

    pub async fn revert_tag_from_confirmed(&self, tag_id: i64, original_type: &str) -> Result<bool, sqlx::Error> {
        sqlx::query("UPDATE player_tags SET tag_type = $2 WHERE id = $1 AND removed_on IS NULL")
            .bind(tag_id)
            .bind(original_type)
            .execute(self.pool)
            .await
            .map(|r| r.rows_affected() > 0)
    }
}
