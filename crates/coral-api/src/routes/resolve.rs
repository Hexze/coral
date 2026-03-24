use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::ToSchema;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Serialize, ToSchema)]
pub struct ResolveResponse {
    pub uuid: String,
    pub username: String,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/resolve/{identifier}", get(resolve_player))
}

#[utoipa::path(
    get,
    path = "/v3/resolve/{identifier}",
    params(
        ("identifier" = String, Path, description = "Player UUID or username")
    ),
    responses(
        (status = 200, description = "Player resolved", body = ResolveResponse),
        (status = 400, description = "Invalid identifier", body = crate::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::error::ErrorResponse),
        (status = 404, description = "Player not found", body = crate::error::ErrorResponse),
        (status = 429, description = "Rate limited", body = crate::error::ErrorResponse),
    ),
    tag = "Internal",
    security(("api_key" = []))
)]
pub async fn resolve_player(
    State(state): State<AppState>,
    Path(identifier): Path<String>,
) -> Result<Json<ResolveResponse>, ApiError> {
    let identity = state.mojang.resolve(&identifier).await?;

    Ok(Json(ResolveResponse {
        uuid: identity.uuid,
        username: identity.username,
    }))
}
