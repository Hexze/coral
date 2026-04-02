use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::{
    error::ErrorResponse,
    responses::*,
    routes::{
        batch::{BatchRequest, BatchResponse},
        cubelify::CubelifyQuery,
        guild::{GuildQuery, GuildResponse},
        resolve::ResolveResponse,
        session::*,
        tags::{AddTagBody, RemoveTagBody, UpdateTagBody, LockRequest, UuidQuery},
        verify::{RedeemCodeResponse, StoreCodeRequest},
        winstreaks::*,
    },
};


#[derive(OpenApi)]
#[openapi(
    paths(
        crate::routes::cubelify::get_cubelify,

        crate::routes::player::player_tags,
        crate::routes::batch::batch_lookup,
        crate::routes::session::session_daily,
        crate::routes::session::session_weekly,
        crate::routes::session::session_monthly,
        crate::routes::session::session_yearly,
        crate::routes::session::session_custom,
        crate::routes::session::list_markers,
        crate::routes::session::create_marker,
        crate::routes::session::rename_marker,
        crate::routes::session::delete_marker,
        crate::routes::session::list_snapshots,
        crate::routes::winstreaks::player_winstreaks,

        crate::routes::tags::add_tag,
        crate::routes::tags::remove_tag,
        crate::routes::tags::update_tag,
        crate::routes::tags::lock_player,
        crate::routes::tags::unlock_player,

        crate::routes::player::player_stats,
        crate::routes::player::player_skin,
        crate::routes::guild::get_guild,
        crate::routes::resolve::resolve_player,
        crate::routes::verify::store_code,
        crate::routes::verify::redeem_code,
        crate::health_check,
    ),
    components(
        schemas(
            ErrorResponse, SuccessResponse,
            CubelifyResponse, CubelifyQuery,
            PlayerTagsResponse, TagResponse,
            BatchRequest, BatchResponse,
            SessionDeltaResponse,
            MarkerResponse, MarkerListResponse,
            CreateMarkerRequest, RenameMarkerRequest,
            SnapshotListResponse, SnapshotDataResponse,
            WinstreakResponse, StreakEntry,
            AddTagBody, RemoveTagBody, UpdateTagBody, LockRequest, UuidQuery,
            PlayerStatsResponse,
            GuildQuery, GuildResponse,
            ResolveResponse,
            StoreCodeRequest, RedeemCodeResponse,
        )
    ),
    tags(
        (name = "Cubelify", description = "Cubelify overlay integration"),
        (name = "Player", description = "Player data — tags, sessions, markers, winstreaks, and batch lookups"),
        (name = "Blacklist", description = "Blacklist management — add, remove, overwrite, lock, and unlock tags"),
        (name = "Internal", description = "Internal endpoints requiring admin access"),
    ),
    modifiers(&SecurityAddon),
    info(
        title = "Coral API",
        description = "Unified Hypixel player data and Urchin blacklist API",
        version = "0.1.0",
    ),
    servers(
        (url = "/", description = "Current server"),
    ),
)]
pub struct ApiDoc;


struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(utoipa::openapi::Components::new);
        components.add_security_scheme(
            "api_key",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("X-API-Key"))),
        );
    }
}
