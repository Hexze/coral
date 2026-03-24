use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use reqwest::Client;
use serde::Deserialize;

const CACHE_TTL: Duration = Duration::from_secs(900);

#[derive(Deserialize)]
struct DiscordUser {
    username: String,
}

struct CacheEntry {
    username: String,
    fetched_at: Instant,
}

pub struct DiscordResolver {
    http: Client,
    token: String,
    cache: Mutex<HashMap<u64, CacheEntry>>,
}

impl DiscordResolver {
    pub fn new(token: String) -> Self {
        Self {
            http: Client::new(),
            token,
            cache: Mutex::new(HashMap::new()),
        }
    }

    pub async fn resolve_username(&self, user_id: u64) -> Option<String> {
        if let Some(cached) = self.from_cache(user_id) {
            return Some(cached);
        }

        let user = self
            .http
            .get(format!("https://discord.com/api/v10/users/{user_id}"))
            .header("Authorization", format!("Bot {}", self.token))
            .send()
            .await
            .ok()?
            .json::<DiscordUser>()
            .await
            .ok()?;

        self.cache.lock().unwrap().insert(
            user_id,
            CacheEntry {
                username: user.username.clone(),
                fetched_at: Instant::now(),
            },
        );

        Some(user.username)
    }

    fn from_cache(&self, user_id: u64) -> Option<String> {
        let cache = self.cache.lock().unwrap();
        let entry = cache.get(&user_id)?;
        if entry.fetched_at.elapsed() < CACHE_TTL {
            Some(entry.username.clone())
        } else {
            None
        }
    }
}
