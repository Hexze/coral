use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::Utc;
use serde::Deserialize;
use utoipa::ToSchema;

use clients::normalize_uuid;
use database::{BlacklistRepository, Member, MemberRepository, PlayerTagRow};

use coral_redis::RateLimitResult;

use crate::cache::refresh_player_cache;
use crate::responses::{CubelifyResponse, CubelifyScore, CubelifyTag};
use crate::state::AppState;

#[derive(Deserialize, ToSchema, utoipa::IntoParams)]
pub(crate) struct CubelifyQuery {
    pub uuid: String,
    pub key: String,
    pub name: Option<String>,
    pub sources: Option<String>,
}

pub fn router(_state: AppState) -> Router<AppState> {
    Router::new().route("/cubelify", get(get_cubelify))
}

#[utoipa::path(
    get,
    path = "/v3/cubelify",
    params(CubelifyQuery),
    responses(
        (status = 200, description = "Cubelify data", body = CubelifyResponse),
    ),
    tag = "Cubelify",
)]
pub async fn get_cubelify(
    State(state): State<AppState>,
    Query(query): Query<CubelifyQuery>,
) -> Json<CubelifyResponse> {
    let result = process_cubelify(&state, &query).await;
    Json(result.unwrap_or_else(|e| e))
}

async fn process_cubelify(
    state: &AppState,
    query: &CubelifyQuery,
) -> Result<CubelifyResponse, CubelifyResponse> {
    let member = validate_api_key(state, &query.key).await?;
    check_rate_limit(state, &query.key, &member).await?;

    let uuid = normalize_uuid(&query.uuid);

    refresh_player_cache(state, &uuid, None).await;

    let tags = fetch_player_tags(state, &uuid).await?;
    Ok(build_cubelify_response(state, &tags).await)
}

async fn validate_api_key(state: &AppState, api_key: &str) -> Result<Member, CubelifyResponse> {
    let repo = MemberRepository::new(state.db.pool());

    let member = repo
        .get_by_api_key(api_key)
        .await
        .map_err(|_| CubelifyResponse::error("Internal Error", "mdi-alert-circle"))?
        .ok_or_else(|| CubelifyResponse::error("Invalid Key", "mdi-key-remove"))?;

    if member.key_locked {
        return Err(CubelifyResponse::error(
            "Your key has been locked",
            "mdi-account-lock-outline",
        ));
    }

    Ok(member)
}

async fn check_rate_limit(
    state: &AppState,
    api_key: &str,
    member: &Member,
) -> Result<(), CubelifyResponse> {
    let limit = match member.access_level {
        4.. => 3000,
        2..=3 => 1200,
        _ => 600,
    };

    match state.rate_limiter.check_and_record(api_key, limit).await {
        Ok(RateLimitResult::Allowed { .. }) => Ok(()),
        Ok(RateLimitResult::Exceeded) => Err(CubelifyResponse::error(
            "Rate limit exceeded",
            "mdi-speedometer",
        )),
        Err(_) => Err(CubelifyResponse::error(
            "Internal Error",
            "mdi-alert-circle",
        )),
    }
}

async fn fetch_player_tags(
    state: &AppState,
    uuid: &str,
) -> Result<Vec<PlayerTagRow>, CubelifyResponse> {
    BlacklistRepository::new(state.db.pool())
        .get_tags(uuid)
        .await
        .map_err(|_| CubelifyResponse::error("Internal Error", "mdi-alert-circle"))
}

async fn build_cubelify_response(state: &AppState, tags: &[PlayerTagRow]) -> CubelifyResponse {
    let mut cubelify_tags = Vec::new();
    let mut total_score = 0.0;

    for tag in tags {
        if let Some(def) = blacklist::lookup(&tag.tag_type) {
            cubelify_tags.push(CubelifyTag {
                icon: def.icon.to_string(),
                color: def.color,
                tooltip: build_tooltip(state, def.name, tag).await,
                text: None,
            });
            total_score += def.score;
        }
    }

    CubelifyResponse {
        score: CubelifyScore {
            value: total_score,
            mode: "add",
        },
        tags: cubelify_tags,
    }
}

async fn build_tooltip(state: &AppState, tag_name: &str, tag: &PlayerTagRow) -> String {
    let name = capitalize(tag_name);
    let time_ago = relative_time(tag.added_on);

    let mut tooltip = if tag.hide_username {
        format!("{name} (Added {time_ago})")
    } else {
        let added_by = state
            .discord
            .resolve_username(tag.added_by as u64)
            .await
            .unwrap_or_else(|| "Unknown".to_string());
        format!("{name} (Added by {added_by} {time_ago})")
    };

    if !tag.reason.is_empty() {
        tooltip.push_str(&format!("\n- {}", tag.reason));
    }

    tooltip
}

fn relative_time(timestamp: chrono::DateTime<Utc>) -> String {
    let delta = Utc::now() - timestamp;
    let seconds = delta.num_seconds();

    if seconds < 60 {
        return "just now".to_string();
    }

    let (value, unit) = if seconds < 3600 {
        (seconds / 60, "min")
    } else if seconds < 86400 {
        (seconds / 3600, "hr")
    } else if seconds < 2_592_000 {
        (seconds / 86400, "d")
    } else if seconds < 31_536_000 {
        (seconds / 2_592_000, "mon")
    } else {
        (seconds / 31_536_000, "yr")
    };

    format!("{value}{unit} ago")
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}
