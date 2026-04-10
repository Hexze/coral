use std::sync::Arc;

use clients::{HypixelClient, MojangClient, SkinProvider};
use coral_redis::{EventPublisher, RateLimiter, RedisPool};
use database::Database;

use crate::discord::DiscordResolver;


pub struct StarfishConfig {
    pub core_tables_bytes: Vec<u8>,
    pub hmac_secret: [u8; 32],
    pub signing_key: ed25519_dalek::SigningKey,
    pub discord_client_id: String,
    pub discord_client_secret: String,
    pub github_token: String,
    pub github_repo: String,
}


#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Database>,
    pub hypixel: Arc<HypixelClient>,
    pub mojang: Arc<MojangClient>,
    pub skin_provider: Option<Arc<dyn SkinProvider>>,
    pub internal_api_key: Option<String>,
    pub redis: RedisPool,
    pub event_publisher: EventPublisher,
    pub rate_limiter: RateLimiter,
    pub discord: Arc<DiscordResolver>,
    pub starfish: Option<Arc<StarfishConfig>>,
}


impl AppState {
    pub fn new(
        db: Database,
        hypixel: HypixelClient,
        mojang: MojangClient,
        skin_provider: Option<Arc<dyn SkinProvider>>,
        internal_api_key: Option<String>,
        redis: RedisPool,
        discord_token: Option<String>,
        starfish: Option<StarfishConfig>,
    ) -> Self {
        Self {
            event_publisher: EventPublisher::new(redis.clone()),
            rate_limiter: RateLimiter::new(redis.clone()),
            discord: Arc::new(DiscordResolver::new(
                discord_token.unwrap_or_default(),
                redis.connection(),
            )),
            db: Arc::new(db),
            hypixel: Arc::new(hypixel),
            mojang: Arc::new(mojang),
            skin_provider,
            internal_api_key,
            redis,
            starfish: starfish.map(Arc::new),
        }
    }
}
