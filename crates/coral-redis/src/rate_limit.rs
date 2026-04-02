use redis::AsyncCommands;

use crate::RedisPool;

const WINDOW_SECS: i64 = 300;
const KEY_PREFIX: &str = "ratelimit:";


pub enum RateLimitResult {
    Allowed { remaining: i64 },
    Exceeded,
}


#[derive(Clone)]
pub struct RateLimiter {
    pool: RedisPool,
}


impl RateLimiter {
    pub fn new(pool: RedisPool) -> Self {
        Self { pool }
    }

    pub async fn check_and_record(
        &self,
        api_key: &str,
        limit: i64,
    ) -> Result<RateLimitResult, redis::RedisError> {
        let key = format!("{KEY_PREFIX}{api_key}");
        let now = chrono::Utc::now().timestamp();
        let mut conn = self.pool.connection();

        redis::pipe()
            .atomic()
            .cmd("ZREMRANGEBYSCORE").arg(&key).arg("-inf").arg(now - WINDOW_SECS).ignore()
            .cmd("ZADD").arg(&key).arg(now).arg(format!("{now}:{}", uuid::Uuid::new_v4())).ignore()
            .cmd("EXPIRE").arg(&key).arg(WINDOW_SECS + 10).ignore()
            .query_async::<()>(&mut conn)
            .await?;

        let count: i64 = conn.zcard(&key).await?;
        match count > limit {
            true => Ok(RateLimitResult::Exceeded),
            false => Ok(RateLimitResult::Allowed { remaining: limit - count }),
        }
    }

    pub async fn check_tag_limit(
        &self,
        discord_id: i64,
        access_level: i16,
    ) -> Result<RateLimitResult, redis::RedisError> {
        let limit = tag_limit_for_access(access_level);
        if limit == 0 { return Ok(RateLimitResult::Allowed { remaining: i64::MAX }); }
        self.check_and_record(&format!("tag:{discord_id}"), limit).await
    }
}


fn tag_limit_for_access(access_level: i16) -> i64 {
    match access_level {
        5.. => 0,
        3..=4 => 60,
        2 => 30,
        1 => 15,
        _ => 10,
    }
}
