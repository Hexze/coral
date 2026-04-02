use anyhow::Result;
use mongodb::{Database, bson::doc};
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::client::CoralClient;

const BATCH_SIZE: usize = 200;

const VALID_TAG_TYPES: &[&str] = &[
    "sniper",
    "blatant_cheater",
    "closet_cheater",
    "confirmed_cheater",
];

#[derive(Debug, Deserialize)]
struct MongoBlacklistPlayer {
    uuid: String,
    is_locked: Option<bool>,
    lock_reason: Option<String>,
    locked_by: Option<serde_json::Value>,
    lock_timestamp: Option<mongodb::bson::DateTime>,
    evidence_thread: Option<String>,
    tags: Option<Vec<MongoTag>>,
}

#[derive(Debug, Deserialize)]
struct MongoTag {
    tag_type: String,
    reason: Option<String>,
    added_by: Option<serde_json::Value>,
    added_on: Option<String>,
    hide_username: Option<bool>,
}

fn parse_i64(val: &serde_json::Value) -> Option<i64> {
    match val {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn normalize_timestamp(s: &str) -> String {
    if s.contains('+') || s.ends_with('Z') { return s.to_string(); }
    format!("{s}+00:00")
}


fn map_tag(tag: &MongoTag) -> Option<serde_json::Value> {
    let reason = tag.reason.as_deref().unwrap_or("");
    let added_by = tag.added_by.as_ref().and_then(parse_i64).unwrap_or(0);
    let added_on = tag.added_on.as_deref().map(normalize_timestamp);

    if tag.tag_type == "caution" {
        if reason.to_lowercase().contains("replays needed") {
            return Some(json!({
                "tag_type": "replays_needed",
                "reason": "",
                "added_by": added_by,
                "added_on": added_on,
                "hide_username": tag.hide_username.unwrap_or(false),
            }));
        }
        return None;
    }

    if !VALID_TAG_TYPES.contains(&tag.tag_type.as_str()) {
        return None;
    }

    Some(json!({
        "tag_type": tag.tag_type,
        "reason": reason,
        "added_by": added_by,
        "added_on": added_on,
        "hide_username": tag.hide_username.unwrap_or(false),
    }))
}

impl MongoBlacklistPlayer {
    fn to_payload(&self) -> Option<serde_json::Value> {
        let tags: Vec<serde_json::Value> = self.tags.as_deref().unwrap_or(&[])
            .iter()
            .filter_map(map_tag)
            .collect();

        if tags.is_empty() {
            return None;
        }

        let locked_at = self.lock_timestamp.map(|dt| {
            chrono::DateTime::from_timestamp_millis(dt.timestamp_millis())
                .unwrap_or_else(chrono::Utc::now)
                .to_rfc3339()
        });

        let locked_by = self.locked_by.as_ref().and_then(parse_i64);

        Some(json!({
            "uuid": self.uuid,
            "is_locked": self.is_locked.unwrap_or(false),
            "lock_reason": self.lock_reason,
            "locked_by": locked_by,
            "locked_at": locked_at,
            "evidence_thread": self.evidence_thread,
            "tags": tags,
        }))
    }
}

pub async fn migrate(mongo_db: &Database, client: &CoralClient) -> Result<usize> {
    let collection = mongo_db.collection::<MongoBlacklistPlayer>("blacklist");
    let mut cursor = collection.find(doc! {}).await?;

    let mut count = 0;
    let mut skipped = 0;
    let mut errors = 0;
    let mut batch = Vec::with_capacity(BATCH_SIZE);

    while cursor.advance().await? {
        let player = match cursor.deserialize_current() {
            Ok(p) => p,
            Err(e) => {
                warn!("Failed to deserialize blacklist player: {e}");
                errors += 1;
                continue;
            }
        };

        match player.to_payload() {
            Some(payload) => batch.push(payload),
            None => {
                skipped += 1;
                continue;
            }
        }

        if batch.len() >= BATCH_SIZE {
            let result = client.post(&json!({"type": "blacklist", "data": batch})).await?;
            let errs = result["errors"].as_u64().unwrap_or(0);
            count += batch.len() - errs as usize;
            errors += errs as usize;
            batch.clear();
            info!("  {count} players migrated ({errors} errors, {skipped} skipped)");
        }
    }

    if !batch.is_empty() {
        let result = client.post(&json!({"type": "blacklist", "data": batch})).await?;
        let errs = result["errors"].as_u64().unwrap_or(0);
        count += batch.len() - errs as usize;
        errors += errs as usize;
    }

    if errors > 0 {
        warn!("Blacklist completed with {errors} errors");
    }
    info!("Blacklist: {count} migrated, {skipped} skipped (no valid tags)");
    Ok(count)
}
