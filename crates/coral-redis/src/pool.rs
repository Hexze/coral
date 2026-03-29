use std::time::Duration;

use redis::Client;
use redis::aio::ConnectionManager;

#[derive(Clone)]
pub struct RedisPool {
    manager: ConnectionManager,
}

impl RedisPool {
    pub async fn connect(url: &str) -> Result<Self, redis::RedisError> {
        let client = Client::open(url)?;
        let manager = tokio::time::timeout(Duration::from_secs(10), ConnectionManager::new(client))
            .await
            .map_err(|_| {
                redis::RedisError::from((
                    redis::ErrorKind::IoError,
                    "Timed out connecting to Redis after 10s",
                ))
            })??;
        Ok(Self { manager })
    }

    pub fn connection(&self) -> ConnectionManager {
        self.manager.clone()
    }
}
