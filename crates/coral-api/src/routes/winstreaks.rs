use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use utoipa::ToSchema;

use database::CacheRepository;
use hypixel::parsing::winstreaks;
use hypixel::{Mode, extract_winstreak_snapshot};

use crate::{error::ApiError, routes::{player, session}, state::AppState};


const MODES: [Mode; 7] = [
    Mode::Overall, Mode::Solos, Mode::Doubles,
    Mode::Threes, Mode::Fours, Mode::FourVFour, Mode::Core,
];


pub fn router() -> Router<AppState> {
    Router::new().route("/player/winstreaks", get(player_winstreaks))
}


#[derive(Serialize, ToSchema)]
pub struct WinstreakResponse {
    pub uuid: String,
    #[schema(value_type = HashMap<String, Vec<StreakEntry>>)]
    pub modes: HashMap<String, Vec<StreakEntry>>,
}


#[derive(Serialize, ToSchema)]
pub struct StreakEntry {
    pub value: u64,
    pub approximate: bool,
    pub timestamp: i64,
    pub readable: String,
}


fn mode_key(mode: Mode) -> &'static str {
    match mode {
        Mode::Overall => "overall",
        Mode::Core => "core",
        Mode::Solos => "solos",
        Mode::Doubles => "doubles",
        Mode::Threes => "threes",
        Mode::Fours => "fours",
        Mode::FourVFour => "4v4",
        _ => "other",
    }
}


#[utoipa::path(
    get,
    path = "/v3/player/winstreaks",
    params(session::PlayerQuery),
    responses(
        (status = 200, body = WinstreakResponse),
        (status = 404, body = crate::error::ErrorResponse),
    ),
    tag = "Player",
    security(("api_key" = []))
)]
pub async fn player_winstreaks(
    State(state): State<AppState>,
    Query(query): Query<session::PlayerQuery>,
) -> Result<Json<WinstreakResponse>, ApiError> {
    let (uuid, _) = player::resolve_identifier(&state, &query.player).await?;

    let snapshots = CacheRepository::new(state.db.pool())
        .get_all_snapshots_mapped(&uuid, extract_winstreak_snapshot)
        .await?;

    let modes = MODES.iter().map(|&mode| {
        let history = winstreaks::calculate(&snapshots, &[mode]);
        let streaks = history.streaks.into_iter().map(|s| StreakEntry {
            value: s.value,
            approximate: s.approximate,
            timestamp: s.timestamp.timestamp_millis(),
            readable: s.timestamp.format("%b %d, %Y %H:%M UTC").to_string(),
        }).collect();
        (mode_key(mode).to_string(), streaks)
    }).collect();

    Ok(Json(WinstreakResponse { uuid, modes }))
}
