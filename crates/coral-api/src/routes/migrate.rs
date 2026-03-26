use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use tracing::{info, warn};

use crate::error::ApiError;
use crate::state::AppState;


pub fn router() -> Router<AppState> {
    Router::new()
        .route("/migrate/members", post(migrate_members))
        .route("/migrate/blacklist", post(migrate_blacklist))
        .route("/migrate/wipe", post(wipe))
}


#[derive(Deserialize)]
struct MemberPayload {
    discord_id: i64,
    uuid: Option<String>,
    api_key: Option<String>,
    join_date: Option<String>,
    request_count: i64,
    access_level: i16,
    key_locked: bool,
    config: serde_json::Value,
    ip_history: Vec<IpEntry>,
    minecraft_accounts: Vec<String>,
}

#[derive(Deserialize)]
struct IpEntry {
    ip_address: String,
    first_seen: Option<String>,
}

#[derive(Deserialize)]
struct BlacklistPayload {
    uuid: String,
    is_locked: bool,
    lock_reason: Option<String>,
    locked_by: Option<i64>,
    locked_at: Option<String>,
    evidence_thread: Option<String>,
    tags: Vec<TagPayload>,
}

#[derive(Deserialize)]
struct TagPayload {
    tag_type: String,
    reason: String,
    added_by: i64,
    added_on: Option<String>,
    hide_username: bool,
}


async fn migrate_members(
    State(state): State<AppState>,
    Json(members): Json<Vec<MemberPayload>>,
) -> Result<Json<MigrateResult>, ApiError> {
    let pool = state.db.pool();
    let mut migrated = 0;
    let mut errors = 0;

    for m in &members {
        let join_date = m.join_date.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);

        let result = sqlx::query(
            r#"
            INSERT INTO members (discord_id, uuid, api_key, join_date, request_count, access_level, key_locked, config)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (discord_id) DO UPDATE SET
                uuid = EXCLUDED.uuid,
                api_key = EXCLUDED.api_key,
                request_count = EXCLUDED.request_count,
                access_level = EXCLUDED.access_level,
                key_locked = EXCLUDED.key_locked,
                config = EXCLUDED.config
            "#,
        )
        .bind(m.discord_id)
        .bind(&m.uuid)
        .bind(&m.api_key)
        .bind(join_date)
        .bind(m.request_count)
        .bind(m.access_level)
        .bind(m.key_locked)
        .bind(&m.config)
        .execute(pool)
        .await;

        if let Err(e) = result {
            warn!("Failed to migrate member {}: {}", m.discord_id, e);
            errors += 1;
            continue;
        }

        let member_id: Option<(i64,)> = sqlx::query_as("SELECT id FROM members WHERE discord_id = $1")
            .bind(m.discord_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

        let Some((member_id,)) = member_id else { continue };

        for ip in &m.ip_history {
            let first_seen = ip.first_seen.as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(chrono::Utc::now);

            let _ = sqlx::query(
                "INSERT INTO api_key_ips (member_id, ip_address, first_seen, last_seen) VALUES ($1, $2::inet, $3, $3) ON CONFLICT (member_id, ip_address) DO NOTHING",
            )
            .bind(member_id)
            .bind(&ip.ip_address)
            .bind(first_seen)
            .execute(pool)
            .await;
        }

        for uuid in &m.minecraft_accounts {
            let _ = sqlx::query(
                "INSERT INTO minecraft_accounts (member_id, uuid) VALUES ($1, $2) ON CONFLICT (member_id, uuid) DO NOTHING",
            )
            .bind(member_id)
            .bind(uuid)
            .execute(pool)
            .await;
        }

        migrated += 1;
    }

    info!("Migrated {} members ({} errors)", migrated, errors);
    Ok(Json(MigrateResult { migrated, errors }))
}


async fn migrate_blacklist(
    State(state): State<AppState>,
    Json(players): Json<Vec<BlacklistPayload>>,
) -> Result<Json<MigrateResult>, ApiError> {
    let pool = state.db.pool();
    let mut migrated = 0;
    let mut errors = 0;

    for p in &players {
        let locked_at = p.locked_at.as_ref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        let player_id: Result<(i64,), _> = sqlx::query_as(
            r#"
            INSERT INTO blacklist_players (uuid, is_locked, lock_reason, locked_by, locked_at, evidence_thread)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (uuid) DO UPDATE SET
                is_locked = EXCLUDED.is_locked,
                lock_reason = EXCLUDED.lock_reason,
                locked_by = EXCLUDED.locked_by,
                locked_at = EXCLUDED.locked_at,
                evidence_thread = EXCLUDED.evidence_thread
            RETURNING id
            "#,
        )
        .bind(&p.uuid)
        .bind(p.is_locked)
        .bind(&p.lock_reason)
        .bind(p.locked_by)
        .bind(locked_at)
        .bind(&p.evidence_thread)
        .fetch_one(pool)
        .await;

        let player_id = match player_id {
            Ok((id,)) => id,
            Err(e) => {
                warn!("Failed to migrate player {}: {}", p.uuid, e);
                errors += 1;
                continue;
            }
        };

        for tag in &p.tags {
            let added_on = tag.added_on.as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(chrono::Utc::now);

            let _ = sqlx::query(
                "INSERT INTO player_tags (player_id, tag_type, reason, added_by, added_on, hide_username) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(player_id)
            .bind(&tag.tag_type)
            .bind(&tag.reason)
            .bind(tag.added_by)
            .bind(added_on)
            .bind(tag.hide_username)
            .execute(pool)
            .await;
        }

        migrated += 1;
    }

    info!("Migrated {} blacklist players ({} errors)", migrated, errors);
    Ok(Json(MigrateResult { migrated, errors }))
}


async fn wipe(
    State(state): State<AppState>,
) -> Result<Json<WipeResult>, ApiError> {
    let pool = state.db.pool();

    let tags = sqlx::query("DELETE FROM player_tags").execute(pool).await?.rows_affected();
    let players = sqlx::query("DELETE FROM blacklist_players").execute(pool).await?.rows_affected();
    let ips = sqlx::query("DELETE FROM api_key_ips").execute(pool).await?.rows_affected();
    let alts = sqlx::query("DELETE FROM minecraft_accounts").execute(pool).await?.rows_affected();
    let members = sqlx::query("DELETE FROM members").execute(pool).await?.rows_affected();

    info!("Wiped: {} members, {} players, {} tags, {} ips, {} alts", members, players, tags, ips, alts);
    Ok(Json(WipeResult { members, players, tags, ips, alts }))
}


#[derive(serde::Serialize)]
struct MigrateResult {
    migrated: usize,
    errors: usize,
}

#[derive(serde::Serialize)]
struct WipeResult {
    members: u64,
    players: u64,
    tags: u64,
    ips: u64,
    alts: u64,
}
