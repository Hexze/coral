use std::io::Cursor;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};

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
    let uuid = if is_uuid(&identifier) {
        normalize_uuid(&identifier)
    } else {
        let identity = state.mojang.resolve(&identifier).await?;
        normalize_uuid(&identity.uuid)
    };

    let tags = BlacklistRepository::new(state.db.pool())
        .get_tags(&uuid)
        .await?;

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
    let (uuid, username_hint) = if is_uuid(&identifier) {
        (normalize_uuid(&identifier), None)
    } else {
        let identity = state.mojang.resolve(&identifier).await?;
        (normalize_uuid(&identity.uuid), Some(identity.username))
    };

    let blacklist_repo = BlacklistRepository::new(state.db.pool());
    let (player_data, tags, profile) = tokio::join!(
        state.hypixel.get_player(&uuid),
        blacklist_repo.get_tags(&uuid),
        state.mojang.get_profile(&uuid),
    );
    let player_data = player_data?;
    let tags = tags?;
    let skin_url = profile.ok().and_then(|p| p.skin_url);

    let username = username_hint.unwrap_or_else(|| {
        player_data
            .as_ref()
            .and_then(|d| d.get("displayname"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| uuid.clone())
    });

    if let Some(ref data) = player_data {
        let pool = state.db.pool().clone();
        let uuid = uuid.clone();
        let data = data.clone();
        let username = username.clone();
        tokio::spawn(async move {
            let cache = CacheRepository::new(&pool);
            let _ = cache
                .store_snapshot(&uuid, &data, None, Some(SNAPSHOT_SOURCE), Some(&username))
                .await;
        });
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
    let skin_provider = state
        .skin_provider
        .as_ref()
        .ok_or_else(|| ApiError::Internal("skin rendering unavailable".into()))?;

    let uuid = if is_uuid(&identifier) {
        normalize_uuid(&identifier)
    } else {
        let identity = state.mojang.resolve(&identifier).await?;
        normalize_uuid(&identity.uuid)
    };

    let skin = skin_provider
        .fetch(&uuid)
        .await
        .ok_or_else(|| ApiError::NotFound("skin not found".into()))?;

    let png_bytes = encode_png(&skin.data)?;

    Ok(([(header::CONTENT_TYPE, "image/png")], Body::from(png_bytes)).into_response())
}

fn encode_png(image: &image::DynamicImage) -> Result<Vec<u8>, ApiError> {
    let mut buffer = Cursor::new(Vec::new());
    image
        .write_to(&mut buffer, image::ImageFormat::Png)
        .map_err(|e| ApiError::Internal(format!("failed to encode png: {e}")))?;
    Ok(buffer.into_inner())
}
