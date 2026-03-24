use std::collections::HashMap;

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use clients::{is_uuid, normalize_uuid};
use database::BlacklistRepository;

use crate::error::ApiError;
use crate::responses::TagResponse;
use crate::state::AppState;

const MAX_BATCH_SIZE: usize = 100;

#[derive(Deserialize, ToSchema)]
pub(crate) struct BatchRequest {
    pub uuids: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct BatchResponse {
    pub players: HashMap<String, Vec<TagResponse>>,
}

pub fn router() -> Router<AppState> {
    Router::new().route("/players", post(batch_lookup))
}

#[utoipa::path(
    post,
    path = "/v3/players",
    request_body = BatchRequest,
    responses(
        (status = 200, description = "Batch lookup completed", body = BatchResponse),
        (status = 400, description = "Invalid request", body = crate::error::ErrorResponse),
        (status = 401, description = "Unauthorized", body = crate::error::ErrorResponse),
    ),
    tag = "Batch",
)]
pub async fn batch_lookup(
    State(state): State<AppState>,
    Json(request): Json<BatchRequest>,
) -> Result<Json<BatchResponse>, ApiError> {
    if request.uuids.is_empty() {
        return Err(ApiError::BadRequest("uuids array is empty".into()));
    }

    if request.uuids.len() > MAX_BATCH_SIZE {
        return Err(ApiError::BadRequest(format!(
            "batch size exceeds maximum of {MAX_BATCH_SIZE}"
        )));
    }

    let uuids: Vec<String> = request
        .uuids
        .iter()
        .filter(|u| is_uuid(u))
        .map(|u| normalize_uuid(u))
        .collect();

    let repo = BlacklistRepository::new(state.db.pool());

    let results = repo
        .get_players_batch(&uuids)
        .await
        .map_err(|e| ApiError::Internal(format!("batch lookup failed: {e}")))?;

    let players = results
        .into_iter()
        .map(|(uuid, tags)| {
            let tag_responses = tags.iter().map(TagResponse::from_db).collect();
            (uuid, tag_responses)
        })
        .collect();

    Ok(Json(BatchResponse { players }))
}
