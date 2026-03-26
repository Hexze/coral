use std::io::Cursor;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::Value;

use clients::{is_uuid, normalize_uuid};
use database::{BlacklistRepository, CacheRepository};

use crate::cache::SNAPSHOT_SOURCE;
use crate::error::{ApiError, ErrorResponse};
use crate::responses::{PlayerStatsResponse, PlayerTagsResponse, TagResponse};
use crate::state::AppState;


pub fn public_router() -> Router<AppState> {
    Router::new().route("/player/tags/{identifier}", get(player_tags))
}


pub fn internal_router() -> Router<AppState> {
    Router::new()
        .route("/player/stats/{identifier}", get(player_stats))
        .route("/player/skin/{identifier}", get(player_skin))
}


pub async fn resolve_identifier(state: &AppState, identifier: &str) -> Result<(String, Option<String>), ApiError> {
    if is_uuid(identifier) {
        Ok((normalize_uuid(identifier), None))
    } else {
        let id = state.mojang.resolve(identifier).await?;
        Ok((normalize_uuid(&id.uuid), Some(id.username)))
    }
}


fn resolve_username(hint: Option<String>, player_data: &Option<Value>, uuid: &str) -> String {
    hint.unwrap_or_else(|| {
        player_data.as_ref()
            .and_then(|d| d["displayname"].as_str())
            .map(String::from)
            .unwrap_or_else(|| uuid.to_string())
    })
}


fn spawn_cache_update(state: &AppState, uuid: &str, data: &Value, username: &str) {
    let (pool, uuid, data, username) = (state.db.pool().clone(), uuid.to_string(), data.clone(), username.to_string());
    tokio::spawn(async move {
        let _ = CacheRepository::new(&pool)
            .store_snapshot(&uuid, &data, None, Some(SNAPSHOT_SOURCE), Some(&username))
            .await;
    });
}


#[utoipa::path(
    get,
    path = "/v3/player/tags/{identifier}",
    params(
        ("identifier" = String, Path, description = "Player UUID or username")
    ),
    responses(
        (status = 200, description = "Player tags retrieved", body = PlayerTagsResponse),
        (status = 400, description = "Invalid identifier", body = ErrorResponse),
        (status = 404, description = "Player not found", body = ErrorResponse),
        (status = 429, description = "Rate limited", body = ErrorResponse),
        (status = 502, description = "External API error", body = ErrorResponse),
    ),
    tag = "Player",
)]
pub async fn player_tags(
    State(state): State<AppState>,
    Path(identifier): Path<String>,
) -> Result<Json<PlayerTagsResponse>, ApiError> {
    let (uuid, _) = resolve_identifier(&state, &identifier).await?;
    let tags = BlacklistRepository::new(state.db.pool()).get_tags(&uuid).await?;
    Ok(Json(PlayerTagsResponse {
        uuid,
        tags: tags.iter().map(TagResponse::from_db).collect(),
    }))
}


#[utoipa::path(
    get,
    path = "/v3/player/stats/{identifier}",
    params(
        ("identifier" = String, Path, description = "Player UUID or username")
    ),
    responses(
        (status = 200, description = "Player stats retrieved", body = PlayerStatsResponse),
        (status = 400, description = "Invalid identifier", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 404, description = "Player not found", body = ErrorResponse),
        (status = 429, description = "Rate limited", body = ErrorResponse),
        (status = 502, description = "External API error", body = ErrorResponse),
    ),
    tag = "Player",
    security(("api_key" = []))
)]
pub async fn player_stats(
    State(state): State<AppState>,
    Path(identifier): Path<String>,
) -> Result<Json<PlayerStatsResponse>, ApiError> {
    let (uuid, username_hint) = resolve_identifier(&state, &identifier).await?;

    let repo = BlacklistRepository::new(state.db.pool());
    let (player_data, tags, profile) = tokio::join!(
        state.hypixel.get_player(&uuid),
        repo.get_tags(&uuid),
        state.mojang.get_profile(&uuid),
    );
    let (player_data, tags) = (player_data?, tags?);
    let skin_url = profile.ok().and_then(|p| p.skin_url);
    let username = resolve_username(username_hint, &player_data, &uuid);

    if let Some(ref data) = player_data {
        spawn_cache_update(&state, &uuid, data, &username);
    }

    Ok(Json(PlayerStatsResponse {
        uuid,
        username,
        hypixel: player_data,
        tags: tags.iter().map(TagResponse::from_db).collect(),
        skin_url,
    }))
}


#[utoipa::path(
    get,
    path = "/v3/player/skin/{identifier}",
    params(
        ("identifier" = String, Path, description = "Player UUID or username")
    ),
    responses(
        (status = 200, description = "Player skin PNG", content_type = "image/png"),
        (status = 400, description = "Invalid identifier", body = ErrorResponse),
        (status = 401, description = "Unauthorized", body = ErrorResponse),
        (status = 404, description = "Skin not found", body = ErrorResponse),
        (status = 500, description = "Skin rendering unavailable", body = ErrorResponse),
    ),
    tag = "Player",
    security(("api_key" = []))
)]
pub async fn player_skin(
    State(state): State<AppState>,
    Path(identifier): Path<String>,
) -> Result<Response, ApiError> {
    let provider = state.skin_provider.as_ref()
        .ok_or_else(|| ApiError::Internal("skin rendering unavailable".into()))?;
    let (uuid, _) = resolve_identifier(&state, &identifier).await?;
    let skin = provider.fetch(&uuid).await
        .ok_or_else(|| ApiError::NotFound("skin not found".into()))?;

    let mut buf = Cursor::new(Vec::new());
    skin.data.write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| ApiError::Internal(format!("failed to encode png: {e}")))?;
    Ok(([(header::CONTENT_TYPE, "image/png")], Body::from(buf.into_inner())).into_response())
}
