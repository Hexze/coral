use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};
use sqlx::{FromRow, PgPool};

const RECONSTRUCTION_THRESHOLD: Duration = Duration::from_millis(2);


pub enum SnapshotResult {
    Stored(i64),
    NoChange,
}


#[derive(Debug, FromRow)]
#[allow(dead_code)]
struct SnapshotRow {
    id: i64,
    is_baseline: bool,
    data: Value,
    timestamp: DateTime<Utc>,
}


pub struct CacheRepository<'a> {
    pool: &'a PgPool,
}


impl<'a> CacheRepository<'a> {
    pub fn new(pool: &'a PgPool) -> Self { Self { pool } }

    pub async fn store_snapshot(
        &self,
        uuid: &str,
        data: &Value,
        discord_id: Option<i64>,
        source: Option<&str>,
        username: Option<&str>,
    ) -> Result<SnapshotResult, sqlx::Error> {
        let latest_baseline = self.get_latest_baseline(uuid).await?;

        let id = match latest_baseline {
            None => {
                self.insert_snapshot(uuid, data, discord_id, source, username, true)
                    .await?
            }
            Some(baseline) => {
                let current = self.reconstruct_current(uuid, &baseline).await?;
                match calculate_delta(&current, data) {
                    None => return Ok(SnapshotResult::NoChange),
                    Some(delta) => {
                        self.insert_snapshot(uuid, &delta, discord_id, source, username, false)
                            .await?
                    }
                }
            }
        };

        self.maybe_promote_to_baseline(uuid, data, username).await?;
        Ok(SnapshotResult::Stored(id))
    }

    pub async fn get_snapshot_at(
        &self,
        uuid: &str,
        timestamp: DateTime<Utc>,
    ) -> Result<Option<Value>, sqlx::Error> {
        let baseline: Option<SnapshotRow> = sqlx::query_as(
            "SELECT id, is_baseline, data, timestamp FROM player_snapshots
             WHERE uuid = $1 AND is_baseline = true AND timestamp <= $2
             ORDER BY timestamp DESC LIMIT 1",
        )
        .bind(uuid)
        .bind(timestamp)
        .fetch_optional(self.pool)
        .await?;

        let Some(baseline) = baseline else { return Ok(None) };
        let deltas = self.get_deltas_in_range(uuid, &baseline, timestamp).await?;
        Ok(Some(reconstruct(&baseline.data, &deltas)))
    }

    pub async fn get_latest_snapshot(&self, uuid: &str) -> Result<Option<Value>, sqlx::Error> {
        self.get_snapshot_at(uuid, Utc::now()).await
    }

    pub async fn list_snapshot_timestamps(
        &self,
        uuid: &str,
        before: DateTime<Utc>,
        limit: i64,
    ) -> Result<Vec<(DateTime<Utc>, bool)>, sqlx::Error> {
        sqlx::query_as::<_, (DateTime<Utc>, bool)>(
            "SELECT timestamp, is_baseline FROM player_snapshots
             WHERE uuid = $1 AND timestamp < $2
             ORDER BY timestamp DESC LIMIT $3",
        )
        .bind(uuid)
        .bind(before)
        .bind(limit)
        .fetch_all(self.pool)
        .await
    }

    pub async fn get_latest_timestamp(&self, uuid: &str) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
        sqlx::query_as::<_, (DateTime<Utc>,)>(
            "SELECT timestamp FROM player_snapshots WHERE uuid = $1
             ORDER BY timestamp DESC LIMIT 1",
        )
        .bind(uuid)
        .fetch_optional(self.pool)
        .await
        .map(|r| r.map(|r| r.0))
    }

    pub async fn resolve_uuid(&self, username: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_as::<_, (String,)>(
            "SELECT uuid FROM player_snapshots WHERE LOWER(username) = $1
             ORDER BY timestamp DESC LIMIT 1",
        )
        .bind(username.to_lowercase())
        .fetch_optional(self.pool)
        .await
        .map(|r| r.map(|r| r.0))
    }

    pub async fn find_by_discord_username(&self, discord_username: &str) -> Result<Vec<(String, String)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT DISTINCT ON (uuid) uuid, username FROM player_snapshots
             WHERE is_baseline = true AND username IS NOT NULL
               AND LOWER(data->'socialMedia'->'links'->>'DISCORD') = LOWER($1)
             ORDER BY uuid, timestamp DESC",
        )
        .bind(discord_username)
        .fetch_all(self.pool)
        .await
    }

    pub async fn get_username(&self, uuid: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_as::<_, (Option<String>,)>(
            "SELECT username FROM player_snapshots
             WHERE uuid = $1 AND username IS NOT NULL
             ORDER BY timestamp DESC LIMIT 1",
        )
        .bind(uuid)
        .fetch_optional(self.pool)
        .await
        .map(|r| r.and_then(|r| r.0))
    }

    pub async fn get_all_snapshots_mapped<T>(
        &self,
        uuid: &str,
        map: impl Fn(&Value) -> Option<T>,
    ) -> Result<Vec<(DateTime<Utc>, T)>, sqlx::Error> {
        let rows: Vec<SnapshotRow> = sqlx::query_as(
            "SELECT id, is_baseline, data, timestamp FROM player_snapshots
             WHERE uuid = $1 ORDER BY timestamp ASC",
        )
        .bind(uuid)
        .fetch_all(self.pool)
        .await?;

        let mut results = Vec::with_capacity(rows.len());
        let mut current = Value::Object(Map::new());
        for row in rows {
            if row.is_baseline {
                current = row.data;
            } else {
                deep_merge_mut(&mut current, &row.data);
            }
            if let Some(mapped) = map(&current) {
                results.push((row.timestamp, mapped));
            }
        }
        Ok(results)
    }

    pub async fn get_snapshots_at_times(
        &self,
        uuid: &str,
        timestamps: &[DateTime<Utc>],
    ) -> Result<Vec<Option<(DateTime<Utc>, Value)>>, sqlx::Error> {
        if timestamps.is_empty() {
            return Ok(Vec::new());
        }

        let mut indexed: Vec<(usize, DateTime<Utc>)> = timestamps.iter().copied().enumerate().collect();
        indexed.sort_by_key(|(_, ts)| *ts);
        let latest = indexed[indexed.len() - 1].1;

        let baseline: Option<SnapshotRow> = sqlx::query_as(
            "SELECT id, is_baseline, data, timestamp FROM player_snapshots
             WHERE uuid = $1 AND is_baseline = true
             ORDER BY timestamp ASC LIMIT 1",
        )
        .bind(uuid)
        .fetch_optional(self.pool)
        .await?;

        let Some(baseline) = baseline else {
            return Ok(vec![None; timestamps.len()]);
        };

        let rows: Vec<SnapshotRow> = sqlx::query_as(
            "SELECT id, is_baseline, data, timestamp FROM player_snapshots
             WHERE uuid = $1 AND timestamp > $2 AND timestamp <= $3
             ORDER BY timestamp ASC",
        )
        .bind(uuid)
        .bind(baseline.timestamp)
        .bind(latest)
        .fetch_all(self.pool)
        .await?;

        let mut current = baseline.data;
        let mut current_ts = baseline.timestamp;
        let mut results = vec![None; timestamps.len()];
        let mut rows = rows.into_iter().peekable();

        for &(orig_idx, target_ts) in &indexed {
            while let Some(row) = rows.peek() {
                if row.timestamp > target_ts { break }
                let row = rows.next().unwrap();
                current_ts = row.timestamp;
                if row.is_baseline {
                    current = row.data;
                } else {
                    deep_merge_mut(&mut current, &row.data);
                }
            }
            results[orig_idx] = Some((current_ts, current.clone()));
        }
        Ok(results)
    }

    async fn insert_snapshot(
        &self,
        uuid: &str,
        data: &Value,
        discord_id: Option<i64>,
        source: Option<&str>,
        username: Option<&str>,
        is_baseline: bool,
    ) -> Result<i64, sqlx::Error> {
        let (id,): (i64,) = sqlx::query_as(
            "INSERT INTO player_snapshots (uuid, discord_id, source, username, is_baseline, data)
             VALUES ($1, $2, $3, $4, $5, $6)
             RETURNING id",
        )
        .bind(uuid)
        .bind(discord_id)
        .bind(source)
        .bind(username)
        .bind(is_baseline)
        .bind(data)
        .fetch_one(self.pool)
        .await?;
        Ok(id)
    }

    async fn maybe_promote_to_baseline(
        &self,
        uuid: &str,
        full_data: &Value,
        username: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let Some(baseline) = self.get_latest_baseline(uuid).await? else { return Ok(()) };
        let deltas = self.get_deltas_in_range(uuid, &baseline, Utc::now()).await?;
        if deltas.is_empty() { return Ok(()) }

        let start = Instant::now();
        let _ = reconstruct(&baseline.data, &deltas);

        if start.elapsed() > RECONSTRUCTION_THRESHOLD {
            self.insert_snapshot(uuid, full_data, None, Some("promotion"), username, true)
                .await?;
        }
        Ok(())
    }

    async fn get_deltas_in_range(
        &self,
        uuid: &str,
        baseline: &SnapshotRow,
        until: DateTime<Utc>,
    ) -> Result<Vec<Value>, sqlx::Error> {
        sqlx::query_as::<_, (Value,)>(
            "SELECT data FROM player_snapshots
             WHERE uuid = $1 AND is_baseline = false AND timestamp > $2 AND timestamp <= $3
             ORDER BY timestamp ASC",
        )
        .bind(uuid)
        .bind(baseline.timestamp)
        .bind(until)
        .fetch_all(self.pool)
        .await
        .map(|rows| rows.into_iter().map(|r| r.0).collect())
    }

    async fn get_latest_baseline(&self, uuid: &str) -> Result<Option<SnapshotRow>, sqlx::Error> {
        sqlx::query_as(
            "SELECT id, is_baseline, data, timestamp FROM player_snapshots
             WHERE uuid = $1 AND is_baseline = true
             ORDER BY timestamp DESC LIMIT 1",
        )
        .bind(uuid)
        .fetch_optional(self.pool)
        .await
    }

    async fn reconstruct_current(&self, uuid: &str, baseline: &SnapshotRow) -> Result<Value, sqlx::Error> {
        let deltas = self.get_deltas_in_range(uuid, baseline, Utc::now()).await?;
        Ok(reconstruct(&baseline.data, &deltas))
    }
}


pub fn calculate_delta(old: &Value, new: &Value) -> Option<Value> {
    match (old, new) {
        (Value::Object(old_map), Value::Object(new_map)) => {
            let delta = calculate_object_delta(old_map, new_map);
            if delta.is_empty() { None } else { Some(Value::Object(delta)) }
        }
        _ if old == new => None,
        _ => Some(new.clone()),
    }
}


fn calculate_object_delta(old: &Map<String, Value>, new: &Map<String, Value>) -> Map<String, Value> {
    let mut delta = Map::new();
    for (key, new_value) in new {
        match old.get(key) {
            Some(old_value) => {
                if let Some(field_delta) = calculate_delta(old_value, new_value) {
                    delta.insert(key.clone(), field_delta);
                }
            }
            None => { delta.insert(key.clone(), new_value.clone()); }
        }
    }
    delta
}


pub fn deep_merge_mut(base: &mut Value, delta: &Value) {
    match (base, delta) {
        (Value::Object(base_map), Value::Object(delta_map)) => {
            for (key, delta_value) in delta_map {
                match base_map.get_mut(key) {
                    Some(base_value) => deep_merge_mut(base_value, delta_value),
                    None => { base_map.insert(key.clone(), delta_value.clone()); }
                }
            }
        }
        (base, delta) => *base = delta.clone(),
    }
}


pub fn reconstruct(baseline: &Value, deltas: &[Value]) -> Value {
    let mut result = baseline.clone();
    for delta in deltas {
        deep_merge_mut(&mut result, delta);
    }
    result
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_calculate_delta_no_change() {
        let old = json!({"a": 1, "b": 2});
        let new = json!({"a": 1, "b": 2});
        assert_eq!(calculate_delta(&old, &new), None);
    }

    #[test]
    fn test_calculate_delta_simple_change() {
        let old = json!({"a": 1, "b": 2});
        let new = json!({"a": 1, "b": 3});
        assert_eq!(calculate_delta(&old, &new), Some(json!({"b": 3})));
    }

    #[test]
    fn test_calculate_delta_nested() {
        let old = json!({"stats": {"kills": 100, "deaths": 50}});
        let new = json!({"stats": {"kills": 105, "deaths": 50}});
        assert_eq!(
            calculate_delta(&old, &new),
            Some(json!({"stats": {"kills": 105}}))
        );
    }

    #[test]
    fn test_deep_merge_mut() {
        let mut base = json!({"a": 1, "b": {"c": 2, "d": 3}});
        let delta = json!({"b": {"c": 5}});
        deep_merge_mut(&mut base, &delta);
        assert_eq!(base, json!({"a": 1, "b": {"c": 5, "d": 3}}));
    }

    #[test]
    fn test_reconstruct() {
        let baseline = json!({"kills": 100, "deaths": 50});
        let deltas = vec![json!({"kills": 105}), json!({"kills": 110, "deaths": 51})];
        let result = reconstruct(&baseline, &deltas);
        assert_eq!(result, json!({"kills": 110, "deaths": 51}));
    }
}
