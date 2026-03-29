use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serenity::all::{ChannelId, Client, GatewayIntents, Token};
use tracing_subscriber::EnvFilter;

use clients::{LocalSkinProvider, SkinProvider};
use coral_redis::{EventPublisher, RedisPool};
use database::Database;

mod accounts;
mod api;
mod commands;
mod events;
mod expr;
mod framework;
mod interact;
mod rendering;
mod sync;
mod utils;

use api::CoralApiClient;
use framework::{Data, Handler};

#[tokio::main]
async fn main() -> Result<()> {
    init_logging();

    let data = init_data().await?;
    let mut client = build_client(data).await?;

    tracing::info!("Starting Coral Bot");
    client.start().await?;

    Ok(())
}

fn init_logging() {
    dotenvy::dotenv().ok();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info,wgpu_core=error,wgpu_hal=error,naga=error,serenity=warn")
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn init_data() -> Result<Data> {
    render::init_canvas();

    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL required");
    let redis_url = env::var("REDIS_URL").expect("REDIS_URL required");
    let api_url = env::var("CORAL_API_URL").expect("CORAL_API_URL required");
    let api_key = env::var("INTERNAL_API_KEY").expect("INTERNAL_API_KEY required");
    let owner_ids = parse_owner_ids();
    let blacklist_channel_id = parse_channel_id("BLACKLIST_CHANNEL_ID");
    let mod_channel_id = parse_channel_id("MOD_CHANNEL_ID");

    tracing::info!("Connecting to database...");
    let db = Database::connect(&database_url).await?;
    tracing::info!("Running migrations...");
    db.migrate().await?;
    tracing::info!("Connecting to Redis...");
    let redis = RedisPool::connect(&redis_url).await?;
    tracing::info!("Initializing services...");
    let event_publisher = EventPublisher::new(redis.clone());
    let api = CoralApiClient::new(api_url, api_key);
    let skin_provider: Arc<dyn SkinProvider> =
        Arc::new(LocalSkinProvider::new().expect("Failed to initialize skin renderer"));

    Ok(Data {
        db: Arc::new(db),
        api: Arc::new(api),
        skin_provider,
        owner_ids,
        blacklist_channel_id,
        mod_channel_id,
        review_forum_id: parse_channel_id("REVIEW_FORUM_ID"),
        evidence_forum_id: parse_channel_id("EVIDENCE_FORUM_ID"),
        redis,
        redis_url,
        event_publisher,
        bedwars_images: Arc::new(Mutex::new(HashMap::new())),
        session_images: Arc::new(Mutex::new(HashMap::new())),
        pending_overwrites: Arc::new(Mutex::new(HashMap::new())),
        sync_cooldowns: Arc::new(Mutex::new(HashMap::new())),
    })
}

fn parse_owner_ids() -> Vec<u64> {
    env::var("OWNER_IDS")
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse::<u64>().ok())
        .collect()
}

fn parse_channel_id(name: &str) -> Option<ChannelId> {
    env::var(name)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(ChannelId::new)
}

async fn build_client(data: Data) -> Result<Client> {
    let token = Token::from_env("DISCORD_TOKEN").expect("Invalid DISCORD_TOKEN");
    let intents = GatewayIntents::GUILDS
        | GatewayIntents::GUILD_MESSAGES
        | GatewayIntents::GUILD_MEMBERS
        | GatewayIntents::MESSAGE_CONTENT;

    let client = Client::builder(token, intents)
        .event_handler(Arc::new(Handler::new(data)))
        .await?;

    Ok(client)
}
