use std::sync::Arc;

use clients::{HypixelClient, MojangClient, SkinProvider};
use coral_redis::{EventPublisher, RateLimiter, RedisPool};
use database::Database;

use crate::discord::DiscordResolver;

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
    ) -> Self {
        let event_publisher = EventPublisher::new(redis.clone());
        let rate_limiter = RateLimiter::new(redis.clone());
        let discord = DiscordResolver::new(discord_token.unwrap_or_default());

        Self {
            db: Arc::new(db),
            hypixel: Arc::new(hypixel),
            mojang: Arc::new(mojang),
            skin_provider,
            internal_api_key,
            redis,
            event_publisher,
            rate_limiter,
            discord: Arc::new(discord),
        }
    }
}
