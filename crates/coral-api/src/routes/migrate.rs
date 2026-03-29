use axum::extract::{DefaultBodyLimit, State};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::ApiError;
use crate::state::AppState;

const MAX_BODY_SIZE: usize = 50 * 1024 * 1024;


pub fn router() -> Router<AppState> {
    Router::new()
        .route("/migrate", post(migrate))
        .layer(DefaultBodyLimit::max(MAX_BODY_SIZE))
}


#[derive(Deserialize)]
#[serde(tag = "type")]
enum Payload {
    #[serde(rename = "wipe_members")]
    WipeMembers,

    #[serde(rename = "wipe_blacklist")]
    WipeBlacklist,

    #[serde(rename = "wipe_cache")]
    WipeCache,

    #[serde(rename = "members")]
    Members { data: Vec<MemberPayload> },

    #[serde(rename = "blacklist")]
    Blacklist { data: Vec<BlacklistPayload> },

    #[serde(rename = "snapshots")]
    Snapshots { data: Vec<SnapshotPayload> },
}

#[derive(Deserialize)]
struct MemberPayload {
    discord_id: i64,
    uuid: Option<String>,
    join_date: Option<String>,
    request_count: i64,
    access_level: i16,
    key_locked: bool,
    config: serde_json::Value,
    minecraft_accounts: Vec<String>,
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

#[derive(Deserialize)]
struct SnapshotPayload {
    uuid: String,
    timestamp: String,
    username: String,
    is_baseline: bool,
    data: serde_json::Value,
}

#[derive(Serialize)]
struct Result {
    migrated: usize,
    errors: usize,
}


async fn migrate(
    State(state): State<AppState>,
    Json(payload): Json<Payload>,
) -> std::result::Result<Json<Result>, ApiError> {
    let pool = state.db.pool();

    match payload {
        Payload::WipeMembers => wipe_members(pool).await,
        Payload::WipeBlacklist => wipe_blacklist(pool).await,
        Payload::WipeCache => wipe_cache(pool).await,
        Payload::Members { data } => migrate_members(pool, &data).await,
        Payload::Blacklist { data } => migrate_blacklist(pool, &data).await,
        Payload::Snapshots { data } => migrate_snapshots(pool, &data).await,
    }
}


async fn wipe_members(pool: &sqlx::PgPool) -> std::result::Result<Json<Result>, ApiError> {
    let alts = sqlx::query("DELETE FROM minecraft_accounts")
        .execute(pool).await?.rows_affected();
    let members = sqlx::query("DELETE FROM members")
        .execute(pool).await?.rows_affected();

    let total = (members + alts) as usize;
    info!("Wiped members: {members} members, {alts} alts");
    Ok(Json(Result { migrated: total, errors: 0 }))
}


async fn wipe_cache(pool: &sqlx::PgPool) -> std::result::Result<Json<Result>, ApiError> {
    let snapshots = sqlx::query("DELETE FROM player_snapshots")
        .execute(pool).await?.rows_affected();
    let sessions = sqlx::query("DELETE FROM session_markers")
        .execute(pool).await?.rows_affected();

    let total = (snapshots + sessions) as usize;
    info!("Wiped cache: {snapshots} snapshots, {sessions} session markers");
    Ok(Json(Result { migrated: total, errors: 0 }))
}


async fn wipe_blacklist(pool: &sqlx::PgPool) -> std::result::Result<Json<Result>, ApiError> {
    let tags = sqlx::query("DELETE FROM player_tags")
        .execute(pool).await?.rows_affected();
    let players = sqlx::query("DELETE FROM blacklist_players")
        .execute(pool).await?.rows_affected();

    let total = (players + tags) as usize;
    info!("Wiped blacklist: {players} players, {tags} tags");
    Ok(Json(Result { migrated: total, errors: 0 }))
}


async fn migrate_members(pool: &sqlx::PgPool, data: &[MemberPayload]) -> std::result::Result<Json<Result>, ApiError> {
    let mut migrated = 0;
    let mut errors = 0;

    for member in data {
        if let Err(e) = insert_member(pool, member).await {
            warn!("Failed to migrate member {}: {e}", member.discord_id);
            errors += 1;
        } else {
            migrated += 1;
        }
    }

    info!("Migrated {migrated} members ({errors} errors)");
    Ok(Json(Result { migrated, errors }))
}


async fn migrate_blacklist(pool: &sqlx::PgPool, data: &[BlacklistPayload]) -> std::result::Result<Json<Result>, ApiError> {
    let mut migrated = 0;
    let mut errors = 0;

    for player in data {
        if let Err(e) = insert_blacklist_player(pool, player).await {
            warn!("Failed to migrate player {}: {e}", player.uuid);
            errors += 1;
        } else {
            migrated += 1;
        }
    }

    info!("Migrated {migrated} blacklist players ({errors} errors)");
    Ok(Json(Result { migrated, errors }))
}


async fn migrate_snapshots(pool: &sqlx::PgPool, data: &[SnapshotPayload]) -> std::result::Result<Json<Result>, ApiError> {
    let mut migrated = 0;
    let mut errors = 0;

    for batch in data.chunks(1000) {
        match insert_snapshot_batch(pool, batch).await {
            Ok(n) => migrated += n,
            Err(e) => {
                warn!("Failed to insert snapshot batch: {e}");
                errors += batch.len();
            }
        }
    }

    info!("Migrated {migrated} snapshots ({errors} errors)");
    Ok(Json(Result { migrated, errors }))
}


async fn insert_member(pool: &sqlx::PgPool, m: &MemberPayload) -> std::result::Result<(), sqlx::Error> {
    let join_date = m.join_date.as_ref()
        .and_then(|s| {
            chrono::DateTime::parse_from_rfc3339(s)
                .inspect_err(|e| warn!("Bad join_date '{}' for member {}: {e}", s, m.discord_id))
                .ok()
        })
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .unwrap_or_else(chrono::Utc::now);

    let api_key = uuid::Uuid::new_v4().to_string();

    let (member_id,): (i64,) = sqlx::query_as(
        r#"
        INSERT INTO members (discord_id, uuid, api_key, join_date, request_count, access_level, key_locked, config)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (discord_id) DO UPDATE SET
            uuid = EXCLUDED.uuid, api_key = EXCLUDED.api_key,
            request_count = EXCLUDED.request_count, access_level = EXCLUDED.access_level,
            key_locked = EXCLUDED.key_locked, config = EXCLUDED.config
        RETURNING id
        "#,
    )
    .bind(m.discord_id).bind(&m.uuid).bind(&api_key)
    .bind(join_date).bind(m.request_count).bind(m.access_level)
    .bind(m.key_locked).bind(&m.config)
    .fetch_one(pool).await?;

    for uuid in &m.minecraft_accounts {
        let _ = sqlx::query(
            "INSERT INTO minecraft_accounts (member_id, uuid) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        ).bind(member_id).bind(uuid).execute(pool).await;
    }

    Ok(())
}


async fn insert_blacklist_player(pool: &sqlx::PgPool, p: &BlacklistPayload) -> std::result::Result<(), sqlx::Error> {
    let locked_at = p.locked_at.as_ref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));

    let (player_id,): (i64,) = sqlx::query_as(
        r#"
        INSERT INTO blacklist_players (uuid, is_locked, lock_reason, locked_by, locked_at, evidence_thread)
        VALUES ($1, $2, $3, $4, $5, $6)
        ON CONFLICT (uuid) DO UPDATE SET
            is_locked = EXCLUDED.is_locked, lock_reason = EXCLUDED.lock_reason,
            locked_by = EXCLUDED.locked_by, locked_at = EXCLUDED.locked_at,
            evidence_thread = EXCLUDED.evidence_thread
        RETURNING id
        "#,
    )
    .bind(&p.uuid).bind(p.is_locked).bind(&p.lock_reason)
    .bind(p.locked_by).bind(locked_at).bind(&p.evidence_thread)
    .fetch_one(pool).await?;

    sqlx::query("DELETE FROM player_tags WHERE player_id = $1")
        .bind(player_id).execute(pool).await?;

    for tag in &p.tags {
        let added_on = tag.added_on.as_ref()
            .and_then(|s| {
                chrono::DateTime::parse_from_rfc3339(s)
                    .inspect_err(|e| warn!("Bad added_on '{}' for player {}: {e}", s, p.uuid))
                    .ok()
            })
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);

        let _ = sqlx::query(
            "INSERT INTO player_tags (player_id, tag_type, reason, added_by, added_on, hide_username) VALUES ($1, $2, $3, $4, $5, $6)",
        ).bind(player_id).bind(&tag.tag_type).bind(&tag.reason)
         .bind(tag.added_by).bind(added_on).bind(tag.hide_username)
         .execute(pool).await;
    }

    Ok(())
}


async fn insert_snapshot_batch(pool: &sqlx::PgPool, batch: &[SnapshotPayload]) -> std::result::Result<usize, sqlx::Error> {
    if batch.is_empty() {
        return Ok(0);
    }

    let params_per_row = 5;
    let mut query = String::from(
        "INSERT INTO player_snapshots (uuid, timestamp, source, username, is_baseline, data) VALUES ",
    );

    for (i, _) in batch.iter().enumerate() {
        if i > 0 { query.push(','); }
        let base = i * params_per_row;
        query.push_str(&format!(
            "(${}, ${}, 'migration', ${}, ${}, ${})",
            base + 1, base + 2, base + 3, base + 4, base + 5
        ));
    }

    query.push_str(" ON CONFLICT DO NOTHING");

    let mut bound = sqlx::query(&query);
    for snap in batch {
        let ts = snap.timestamp.parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap_or_else(|_| chrono::Utc::now());
        bound = bound
            .bind(&snap.uuid)
            .bind(ts)
            .bind(&snap.username)
            .bind(snap.is_baseline)
            .bind(&snap.data);
    }

    bound.execute(pool).await?;
    Ok(batch.len())
}
