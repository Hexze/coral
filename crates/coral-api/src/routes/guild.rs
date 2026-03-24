use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use clients::{is_uuid, normalize_uuid};

use crate::error::ApiError;
use crate::state::AppState;

const MAX_USERNAME_LENGTH: usize = 16;
const EXP_PER_LEVEL_AFTER_15: u64 = 3_000_000;

#[derive(Deserialize, ToSchema, utoipa::IntoParams)]
pub(crate) struct GuildQuery {
    pub by: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct GuildResponse {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag_color: Option<String>,
    pub level: u32,
    pub members: usize,
    pub experience: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player: Option<GuildMemberInfo>,
}

#[derive(Serialize, ToSchema)]
pub struct GuildMemberInfo {
    pub uuid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub joined: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weekly_gexp: Option<u64>,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/guild/{identifier}", get(get_guild))
}

#[utoipa::path(
    get,
    path = "/v3/guild/{identifier}",
    params(
        ("identifier" = String, Path, description = "Guild name or player UUID/username"),
        GuildQuery
    ),
    responses(
        (status = 200, description = "Guild data retrieved", body = Option<GuildResponse>),
        (status = 400, description = "Invalid request", body = crate::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::error::ErrorResponse),
        (status = 404, description = "Guild not found", body = crate::error::ErrorResponse),
        (status = 429, description = "Rate limited", body = crate::error::ErrorResponse),
        (status = 502, description = "External API error", body = crate::error::ErrorResponse),
    ),
    tag = "Guild",
    security(("api_key" = []))
)]
pub async fn get_guild(
    State(state): State<AppState>,
    Path(identifier): Path<String>,
    Query(query): Query<GuildQuery>,
) -> Result<Json<Option<GuildResponse>>, ApiError> {
    let (guild, player_uuid) = match query.by.as_deref() {
        Some("name") => fetch_by_name(&state, &identifier).await?,
        Some("player") => fetch_by_player(&state, &identifier).await?,
        Some(other) => {
            return Err(ApiError::BadRequest(format!(
                "invalid 'by' parameter: {other}"
            )));
        }
        None => fetch_auto(&state, &identifier).await?,
    };

    let response = guild.map(|g| build_response(&g, player_uuid.as_deref()));
    Ok(Json(response))
}

async fn fetch_by_name(
    state: &AppState,
    name: &str,
) -> Result<(Option<serde_json::Value>, Option<String>), ApiError> {
    let guild = state.hypixel.get_guild_by_name(name).await?;
    Ok((guild, None))
}

async fn fetch_by_player(
    state: &AppState,
    identifier: &str,
) -> Result<(Option<serde_json::Value>, Option<String>), ApiError> {
    let uuid = if is_uuid(identifier) {
        normalize_uuid(identifier)
    } else {
        resolve_uuid(state, identifier).await?
    };
    let guild = state.hypixel.get_guild_by_player(&uuid).await?;
    Ok((guild, Some(uuid)))
}

async fn fetch_auto(
    state: &AppState,
    identifier: &str,
) -> Result<(Option<serde_json::Value>, Option<String>), ApiError> {
    if is_uuid(identifier) {
        let uuid = normalize_uuid(identifier);
        let guild = state.hypixel.get_guild_by_player(&uuid).await?;
        return Ok((guild, Some(uuid)));
    }

    if identifier.len() <= MAX_USERNAME_LENGTH {
        if let Ok(uuid) = resolve_uuid(state, identifier).await {
            let guild = state.hypixel.get_guild_by_player(&uuid).await?;
            return Ok((guild, Some(uuid)));
        }
    }

    fetch_by_name(state, identifier).await
}

async fn resolve_uuid(state: &AppState, identifier: &str) -> Result<String, ApiError> {
    let identity = state.mojang.resolve(identifier).await?;
    Ok(normalize_uuid(&identity.uuid))
}

fn build_response(guild: &serde_json::Value, player_uuid: Option<&str>) -> GuildResponse {
    let members = guild["members"].as_array();
    let exp = guild["exp"].as_u64().unwrap_or(0);

    GuildResponse {
        id: guild["_id"].as_str().unwrap_or_default().to_string(),
        name: guild["name"].as_str().unwrap_or_default().to_string(),
        tag: guild["tag"].as_str().map(String::from),
        tag_color: guild["tagColor"].as_str().map(String::from),
        level: calculate_level(exp),
        members: members.map(|m| m.len()).unwrap_or(0),
        experience: exp,
        created: parse_timestamp(guild["created"].as_i64()),
        player: player_uuid.and_then(|uuid| find_member(members, uuid)),
    }
}

fn find_member(
    members: Option<&Vec<serde_json::Value>>,
    target_uuid: &str,
) -> Option<GuildMemberInfo> {
    let members = members?;

    for member in members {
        let Some(uuid) = member["uuid"].as_str() else {
            continue;
        };

        if normalize_uuid(uuid) != target_uuid {
            continue;
        }

        let weekly_gexp = member["expHistory"]
            .as_object()
            .map(|exp| exp.values().filter_map(|v| v.as_u64()).sum());

        return Some(GuildMemberInfo {
            uuid: target_uuid.to_string(),
            rank: member["rank"].as_str().map(String::from),
            joined: parse_timestamp(member["joined"].as_i64()),
            weekly_gexp,
        });
    }

    None
}

fn parse_timestamp(millis: Option<i64>) -> Option<String> {
    millis
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|dt| dt.to_rfc3339())
}

fn calculate_level(exp: u64) -> u32 {
    const THRESHOLDS: [u64; 15] = [
        100_000, 150_000, 250_000, 500_000, 750_000, 1_000_000, 1_250_000, 1_500_000, 2_000_000,
        2_500_000, 2_500_000, 2_500_000, 2_500_000, 2_500_000, 3_000_000,
    ];

    let mut level = 0u32;
    let mut remaining = exp;

    for threshold in THRESHOLDS {
        if remaining < threshold {
            return level;
        }
        remaining -= threshold;
        level += 1;
    }

    level + (remaining / EXP_PER_LEVEL_AFTER_15) as u32
}
