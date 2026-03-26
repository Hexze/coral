use axum::extract::{Path, Query, State};
use axum::routing::{get, patch};
use axum::{Extension, Json, Router};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use utoipa::{IntoParams, ToSchema};

use database::*;

use crate::auth::AuthenticatedMember;
use crate::error::ApiError;
use crate::responses::SuccessResponse;
use crate::routes::player;
use crate::state::AppState;


pub fn router() -> Router<AppState> {
    Router::new()
        .route("/player/sessions/daily", get(session_daily))
        .route("/player/sessions/weekly", get(session_weekly))
        .route("/player/sessions/monthly", get(session_monthly))
        .route("/player/sessions/yearly", get(session_yearly))
        .route("/player/sessions/custom", get(session_custom))
        .route("/player/sessions/markers", get(list_markers).post(create_marker))
        .route("/player/sessions/markers/{name}", patch(rename_marker).delete(delete_marker))
        .route("/player/sessions/snapshots", get(list_snapshots))
}


#[derive(Deserialize, IntoParams)]
pub struct PlayerQuery {
    pub player: String,
}

#[derive(Deserialize, IntoParams)]
pub struct CustomSessionQuery {
    pub player: String,
    #[serde(default)]
    pub duration: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub marker: Option<String>,
}

#[derive(Deserialize, IntoParams)]
pub struct SnapshotQuery {
    pub player: String,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub before: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateMarkerRequest {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct RenameMarkerRequest {
    pub new_name: String,
}

#[derive(Serialize, ToSchema)]
pub struct SessionDeltaResponse {
    pub uuid: String,
    pub from: String,
    #[schema(value_type = Value)]
    pub delta: Value,
}

#[derive(Serialize, ToSchema)]
pub struct MarkerResponse {
    pub id: i64,
    pub name: String,
    pub snapshot_timestamp: String,
    pub created_at: String,
}

#[derive(Serialize, ToSchema)]
pub struct MarkerListResponse {
    pub uuid: String,
    pub markers: Vec<MarkerResponse>,
}

#[derive(Serialize, ToSchema)]
pub struct SnapshotListResponse {
    pub uuid: String,
    pub snapshots: Vec<SnapshotEntry>,
}

#[derive(Serialize, ToSchema)]
pub struct SnapshotEntry {
    pub timestamp: String,
    pub is_baseline: bool,
}


macro_rules! period_handler {
    ($name:ident, $period:ident, $path:literal) => {
        #[utoipa::path(
            get, path = $path, params(PlayerQuery),
            responses((status = 200, body = SessionDeltaResponse), (status = 404, body = crate::error::ErrorResponse)),
            tag = "Sessions", security(("api_key" = []))
        )]
        pub async fn $name(
            State(state): State<AppState>,
            Query(query): Query<PlayerQuery>,
        ) -> Result<Json<SessionDeltaResponse>, ApiError> {
            delta_response(&state, &query.player, Period::$period.last_reset(Utc::now())).await
        }
    };
}

period_handler!(session_daily,   Daily,   "/v3/player/sessions/daily");
period_handler!(session_weekly,  Weekly,  "/v3/player/sessions/weekly");
period_handler!(session_monthly, Monthly, "/v3/player/sessions/monthly");
period_handler!(session_yearly,  Yearly,  "/v3/player/sessions/yearly");


#[utoipa::path(
    get,
    path = "/v3/player/sessions/custom",
    params(CustomSessionQuery),
    responses(
        (status = 200, body = SessionDeltaResponse),
        (status = 400, body = crate::error::ErrorResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
    ),
    tag = "Sessions",
    security(("api_key" = []))
)]
pub async fn session_custom(
    State(state): State<AppState>,
    Extension(member): Extension<AuthenticatedMember>,
    Query(query): Query<CustomSessionQuery>,
) -> Result<Json<SessionDeltaResponse>, ApiError> {
    let now = Utc::now();
    let (uuid, _) = player::resolve_identifier(&state, &query.player).await?;

    let from = match (&query.duration, &query.from, &query.marker) {
        (Some(d), None, None) => {
            now - parse_duration(d)
                .ok_or_else(|| ApiError::BadRequest("'duration' must be like 48h, 10d, or 2w".into()))?
        }

        (None, Some(ts), None) => {
            DateTime::parse_from_rfc3339(ts)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|_| ApiError::BadRequest("'from' must be an RFC 3339 timestamp".into()))?
        }

        (None, None, Some(name)) => {
            require_owner(&state, &uuid, member.0.discord_id).await?;
            SessionRepository::new(state.db.pool())
                .get(&uuid, member.0.discord_id, name)
                .await?
                .ok_or_else(|| ApiError::NotFound(format!("marker '{name}' not found")))?
                .snapshot_timestamp
        }

        _ => return Err(ApiError::BadRequest(
            "specify exactly one of 'duration', 'from', or 'marker'".into(),
        )),
    };

    delta_response(&state, &query.player, from).await
}


async fn delta_response(
    state: &AppState,
    player: &str,
    from: DateTime<Utc>,
) -> Result<Json<SessionDeltaResponse>, ApiError> {
    let (uuid, _) = player::resolve_identifier(state, player).await?;
    let cache = CacheRepository::new(state.db.pool());

    let snapshot = cache.get_snapshot_at(&uuid, from).await?
        .ok_or_else(|| ApiError::NotFound("no snapshot data for this player".into()))?;

    let current = cache.get_latest_snapshot(&uuid).await?
        .ok_or_else(|| ApiError::NotFound("no current data".into()))?;

    let delta = session_delta(&snapshot, &current)
        .unwrap_or(Value::Object(Map::new()));

    Ok(Json(SessionDeltaResponse {
        uuid,
        from: from.to_rfc3339(),
        delta,
    }))
}


fn parse_duration(s: &str) -> Option<Duration> {
    let (digits, unit) = s.split_at(s.len().checked_sub(1)?);
    let n: i64 = digits.parse().ok()?;
    if n <= 0 { return None; }

    match unit {
        "h" => Some(Duration::hours(n)),
        "d" => Some(Duration::days(n)),
        "w" => Some(Duration::weeks(n)),
        _ => None,
    }
}


#[utoipa::path(
    get,
    path = "/v3/player/sessions/markers",
    params(PlayerQuery),
    responses(
        (status = 200, body = MarkerListResponse),
        (status = 403, body = crate::error::ErrorResponse),
    ),
    tag = "Sessions",
    security(("api_key" = []))
)]
pub async fn list_markers(
    State(state): State<AppState>,
    Extension(member): Extension<AuthenticatedMember>,
    Query(query): Query<PlayerQuery>,
) -> Result<Json<MarkerListResponse>, ApiError> {
    let (uuid, _) = player::resolve_identifier(&state, &query.player).await?;
    require_owner(&state, &uuid, member.0.discord_id).await?;

    let markers = SessionRepository::new(state.db.pool())
        .list(&uuid, member.0.discord_id)
        .await?;

    Ok(Json(MarkerListResponse {
        uuid,
        markers: markers.iter().map(to_marker_response).collect(),
    }))
}


#[utoipa::path(
    post,
    path = "/v3/player/sessions/markers",
    params(PlayerQuery),
    request_body = CreateMarkerRequest,
    responses(
        (status = 200, body = MarkerResponse),
        (status = 403, body = crate::error::ErrorResponse),
    ),
    tag = "Sessions",
    security(("api_key" = []))
)]
pub async fn create_marker(
    State(state): State<AppState>,
    Extension(member): Extension<AuthenticatedMember>,
    Query(query): Query<PlayerQuery>,
    Json(body): Json<CreateMarkerRequest>,
) -> Result<Json<MarkerResponse>, ApiError> {
    let (uuid, _) = player::resolve_identifier(&state, &query.player).await?;
    require_owner(&state, &uuid, member.0.discord_id).await?;

    let name = body.name.unwrap_or_else(|| Utc::now().format("%b %d, %Y").to_string());
    validate_marker_name(&name)?;

    let marker = SessionRepository::new(state.db.pool())
        .create(&uuid, member.0.discord_id, &name, Utc::now())
        .await?;

    Ok(Json(to_marker_response(&marker)))
}


#[utoipa::path(
    patch,
    path = "/v3/player/sessions/markers/{name}",
    params(("name" = String, Path, description = "Current marker name"), PlayerQuery),
    request_body = RenameMarkerRequest,
    responses(
        (status = 200, body = SuccessResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
    ),
    tag = "Sessions",
    security(("api_key" = []))
)]
pub async fn rename_marker(
    State(state): State<AppState>,
    Extension(member): Extension<AuthenticatedMember>,
    Path(name): Path<String>,
    Query(query): Query<PlayerQuery>,
    Json(body): Json<RenameMarkerRequest>,
) -> Result<Json<SuccessResponse>, ApiError> {
    let (uuid, _) = player::resolve_identifier(&state, &query.player).await?;
    require_owner(&state, &uuid, member.0.discord_id).await?;
    validate_marker_name(&body.new_name)?;

    let ok = SessionRepository::new(state.db.pool())
        .rename(&uuid, member.0.discord_id, &name, &body.new_name)
        .await?;

    if !ok { return Err(ApiError::NotFound(format!("marker '{name}' not found"))); }
    Ok(Json(SuccessResponse { success: true }))
}


#[utoipa::path(
    delete,
    path = "/v3/player/sessions/markers/{name}",
    params(("name" = String, Path, description = "Marker name"), PlayerQuery),
    responses(
        (status = 200, body = SuccessResponse),
        (status = 403, body = crate::error::ErrorResponse),
        (status = 404, body = crate::error::ErrorResponse),
    ),
    tag = "Sessions",
    security(("api_key" = []))
)]
pub async fn delete_marker(
    State(state): State<AppState>,
    Extension(member): Extension<AuthenticatedMember>,
    Path(name): Path<String>,
    Query(query): Query<PlayerQuery>,
) -> Result<Json<SuccessResponse>, ApiError> {
    let (uuid, _) = player::resolve_identifier(&state, &query.player).await?;
    require_owner(&state, &uuid, member.0.discord_id).await?;

    let ok = SessionRepository::new(state.db.pool())
        .delete(&uuid, member.0.discord_id, &name)
        .await?;

    if !ok { return Err(ApiError::NotFound(format!("marker '{name}' not found"))); }
    Ok(Json(SuccessResponse { success: true }))
}



#[utoipa::path(
    get,
    path = "/v3/player/sessions/snapshots",
    params(SnapshotQuery),
    responses(
        (status = 200, body = SnapshotListResponse),
        (status = 403, body = crate::error::ErrorResponse),
    ),
    tag = "Sessions",
    security(("api_key" = []))
)]
pub async fn list_snapshots(
    State(state): State<AppState>,
    Extension(member): Extension<AuthenticatedMember>,
    Query(query): Query<SnapshotQuery>,
) -> Result<Json<SnapshotListResponse>, ApiError> {
    let (uuid, _) = player::resolve_identifier(&state, &query.player).await?;
    require_owner(&state, &uuid, member.0.discord_id).await?;

    let limit = query.limit.unwrap_or(100).min(500);
    let before = match query.before {
        Some(ref s) => DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|_| ApiError::BadRequest("'before' must be RFC 3339".into()))?,
        None => Utc::now(),
    };

    let rows = CacheRepository::new(state.db.pool())
        .list_snapshot_timestamps(&uuid, before, limit)
        .await?;

    Ok(Json(SnapshotListResponse {
        uuid,
        snapshots: rows.into_iter().map(|(ts, baseline)| SnapshotEntry {
            timestamp: ts.to_rfc3339(),
            is_baseline: baseline,
        }).collect(),
    }))
}


async fn require_owner(state: &AppState, uuid: &str, discord_id: i64) -> Result<(), ApiError> {
    let owned = AccountRepository::new(state.db.pool())
        .is_owned_by(uuid, discord_id)
        .await?;

    if !owned {
        return Err(ApiError::Forbidden("you do not own this account".into()));
    }
    Ok(())
}


fn validate_marker_name(name: &str) -> Result<(), ApiError> {
    if name.is_empty() || name.len() > 32 {
        return Err(ApiError::BadRequest("marker name must be 1-32 characters".into()));
    }
    Ok(())
}


fn to_marker_response(m: &SessionMarker) -> MarkerResponse {
    MarkerResponse {
        id: m.id,
        name: m.name.clone(),
        snapshot_timestamp: m.snapshot_timestamp.to_rfc3339(),
        created_at: m.created_at.to_rfc3339(),
    }
}
