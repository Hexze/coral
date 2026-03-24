use std::env;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use utoipa::OpenApi;
use utoipa_scalar::{Scalar, Servable};

use clients::{HypixelClient, LocalSkinProvider, MojangClient, SkinProvider};
use coral_redis::RedisPool;
use database::Database;

mod auth;
mod cache;
mod discord;
mod error;
mod middleware;
mod openapi;
mod responses;
mod routes;
mod state;

use state::AppState;

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();

    let state = init_state().await?;
    let app = build_router(state);

    serve(app).await
}

fn init_logging() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
}

async fn init_state() -> Result<AppState> {
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL required");
    let redis_url = env::var("REDIS_URL").expect("REDIS_URL required");
    let hypixel_keys = parse_hypixel_keys();
    let internal_api_key = env::var("INTERNAL_API_KEY").ok();

    let db = Database::connect(&database_url).await?;
    if let Err(e) = db.migrate().await {
        tracing::warn!("Migration skipped: {e}");
    }
    let redis = RedisPool::connect(&redis_url).await?;
    let hypixel = HypixelClient::new(hypixel_keys)?;
    let mojang = MojangClient::new();
    let skin_provider = match LocalSkinProvider::new() {
        Some(provider) => {
            tracing::info!("Skin renderer initialized");
            Some(Arc::new(provider) as Arc<dyn SkinProvider>)
        }
        None => {
            tracing::warn!("Skin renderer unavailable (no GPU) - /player/*/skin endpoint disabled");
            None
        }
    };

    let discord_token = env::var("DISCORD_TOKEN").ok();

    Ok(AppState::new(
        db,
        hypixel,
        mojang,
        skin_provider,
        internal_api_key,
        redis,
        discord_token,
    ))
}

fn parse_hypixel_keys() -> Vec<String> {
    env::var("HYPIXEL_API_KEYS")
        .expect("HYPIXEL_API_KEYS required")
        .split(',')
        .map(|s| s.trim().to_string())
        .collect()
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .merge(Scalar::with_url("/docs", openapi::ApiDoc::openapi()))
        .nest("/v3", routes::router(state.clone()))
        .with_state(state)
}

#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Service is healthy", body = serde_json::Value),
        (status = 503, description = "Service is degraded", body = serde_json::Value),
    ),
    tag = "Health",
)]
pub async fn health_check(State(state): State<AppState>) -> Response {
    let db_ok = sqlx::query("SELECT 1")
        .execute(state.db.pool())
        .await
        .is_ok();

    let redis_ok = redis::cmd("PING")
        .query_async::<String>(&mut state.redis.connection())
        .await
        .is_ok();

    let body = serde_json::json!({
        "status": if db_ok && redis_ok { "healthy" } else { "degraded" },
        "postgres": db_ok,
        "redis": redis_ok,
    });

    let status = if db_ok && redis_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status, axum::Json(body)).into_response()
}

async fn serve(app: Router) -> Result<()> {
    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| "8000".to_string())
        .parse()
        .expect("PORT must be a number");

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Coral API listening on {addr}");

    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
