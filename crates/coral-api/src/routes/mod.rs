use axum::middleware;
use axum::Router;

use crate::auth::{allow_internal_or_auth, require_internal_or_admin, require_moderator};
use crate::state::AppState;

pub mod batch;
pub mod cubelify;
pub mod guild;
pub mod migrate;
pub mod player;
pub mod resolve;
pub mod session;
pub mod tags;
pub mod verify;
pub mod winstreaks;


pub fn router(state: AppState) -> Router<AppState> {
    let public = Router::new()
        .merge(player::public_router())
        .merge(batch::router())
        .merge(tags::router())
        .merge(session::router())
        .merge(winstreaks::router())
        .route_layer(middleware::from_fn_with_state(state.clone(), allow_internal_or_auth));

    let internal = Router::new()
        .merge(player::internal_router())
        .merge(guild::router())
        .merge(resolve::router())
        .merge(verify::router())
        .merge(migrate::router())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_internal_or_admin));

    let moderator = Router::new()
        .merge(tags::mod_router())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_moderator));

    Router::new()
        .merge(public)
        .merge(internal)
        .merge(moderator)
        .merge(cubelify::router(state))
}
