use std::env;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
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
mod openapi;
mod responses;
mod routes;
mod state;

use state::{AppState, StarfishConfig};


#[tokio::main]
async fn main() -> Result<()> {
    init_logging();
    let state = init_state().await?;
    if state.starfish.is_some() {
        spawn_starfish_cleanup(state.db.clone());
    }
    serve(build_router(state)).await
}


fn spawn_starfish_cleanup(db: std::sync::Arc<database::Database>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            let repo = database::StarfishRepository::new(db.pool());
            if let Err(e) = repo.cleanup_expired().await {
                tracing::warn!("Starfish cleanup failed: {e}");
            }
        }
    });
}


fn init_logging() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
}


async fn init_state() -> Result<AppState> {
    let db = Database::connect(&env::var("DATABASE_URL").expect("DATABASE_URL required")).await?;
    if let Err(e) = db.migrate().await {
        tracing::warn!("Migration skipped: {e}");
    }
    let redis = RedisPool::connect(&env::var("REDIS_URL").expect("REDIS_URL required")).await?;
    let hypixel = HypixelClient::new(
        env::var("HYPIXEL_API_KEY").expect("HYPIXEL_API_KEY required"),
        redis.connection(),
    )?;
    let mojang = MojangClient::new();
    let skin_provider = match LocalSkinProvider::new(redis.connection()) {
        Some(p) => {
            tracing::info!("Skin renderer initialized");
            Some(Arc::new(p) as Arc<dyn SkinProvider>)
        }
        None => {
            tracing::warn!("Skin renderer unavailable (no GPU) - /player/*/skin endpoint disabled");
            None
        }
    };
    let starfish = parse_starfish_config();

    Ok(AppState::new(
        db,
        hypixel,
        mojang,
        skin_provider,
        env::var("INTERNAL_API_KEY").ok(),
        redis,
        env::var("DISCORD_TOKEN").ok(),
        starfish,
    ))
}


fn parse_starfish_config() -> Option<StarfishConfig> {
    let hmac_secret: [u8; 32] = hex::decode(env::var("STARFISH_HMAC_SECRET").ok()?).ok()?.try_into().ok()?;

    let signing_key_bytes: [u8; 32] = hex::decode(
        env::var("STARFISH_ED25519_PRIVATE_KEY").expect("STARFISH_ED25519_PRIVATE_KEY required when Starfish is enabled")
    ).expect("STARFISH_ED25519_PRIVATE_KEY must be valid hex").try_into().expect("STARFISH_ED25519_PRIVATE_KEY must be 32 bytes");
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&signing_key_bytes);

    let core_tables_bytes = match env::var("STARFISH_CORE_TABLES") {
        Ok(hex_str) => hex::decode(&hex_str).expect("STARFISH_CORE_TABLES must be valid hex"),
        Err(_) => {
            tracing::info!("STARFISH_CORE_TABLES not set, using defaults");
            crate::routes::starfish::default_core_tables_bytes()
        }
    };

    let config = StarfishConfig {
        core_tables_bytes,
        hmac_secret,
        signing_key,
        discord_client_id: env::var("STARFISH_DISCORD_CLIENT_ID").expect("STARFISH_DISCORD_CLIENT_ID required when Starfish is enabled"),
        discord_client_secret: env::var("STARFISH_DISCORD_CLIENT_SECRET").expect("STARFISH_DISCORD_CLIENT_SECRET required when Starfish is enabled"),
        github_token: env::var("STARFISH_GITHUB_TOKEN").expect("STARFISH_GITHUB_TOKEN required when Starfish is enabled"),
        github_repo: env::var("STARFISH_GITHUB_REPO").expect("STARFISH_GITHUB_REPO required when Starfish is enabled"),
    };
    tracing::info!("Starfish licensing enabled ({} bytes core tables)", config.core_tables_bytes.len());
    Some(config)
}


fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .merge(Scalar::with_url("/", openapi::ApiDoc::openapi()))
        .nest("/v3", routes::router(state.clone()))
        .nest("/api/v1/starfish", routes::starfish::router(state.clone()))
        .with_state(state)
}


#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "Service is healthy", body = serde_json::Value),
        (status = 503, description = "Service is degraded", body = serde_json::Value),
    ),
    tag = "Internal",
)]
pub async fn health_check(State(state): State<AppState>) -> Response {
    let db_ok = sqlx::query("SELECT 1").execute(state.db.pool()).await.is_ok();
    let redis_ok = redis::cmd("PING")
        .query_async::<String>(&mut state.redis.connection())
        .await
        .is_ok();
    let status = if db_ok && redis_ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    let body = serde_json::json!({
        "status": if db_ok && redis_ok { "healthy" } else { "degraded" },
        "postgres": db_ok,
        "redis": redis_ok,
    });
    (status, axum::Json(body)).into_response()
}


async fn serve(app: Router) -> Result<()> {
    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| "8000".into())
        .parse()
        .expect("PORT must be a number");
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("Coral API listening on {addr}");
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
