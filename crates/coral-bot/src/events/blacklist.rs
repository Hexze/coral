use serenity::all::*;

use coral_redis::{BlacklistEvent, EventSubscriber};
use database::{BlacklistRepository, CacheRepository, PlayerTagRow};

use crate::commands::blacklist::channel;
use crate::framework::Data;


pub fn spawn_subscriber(ctx: Context, data: Data) {
    let redis_url = data.redis_url.clone();

    tokio::spawn(async move {
        loop {
            let ctx = ctx.clone();
            let data = data.clone();

            let result = EventSubscriber::run(&redis_url, move |event| {
                let ctx = ctx.clone();
                let data = data.clone();
                async move {
                    if let Err(e) = handle_event(&ctx, &data, event).await {
                        tracing::error!("Failed to handle blacklist event: {e}");
                    }
                }
            })
            .await;

            if let Err(e) = result {
                tracing::error!("Blacklist event subscriber disconnected: {e}");
            }

            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}


async fn handle_event(ctx: &Context, data: &Data, event: BlacklistEvent) -> anyhow::Result<()> {
    let repo = BlacklistRepository::new(data.db.pool());
    let cache = CacheRepository::new(data.db.pool());

    match event {
        BlacklistEvent::TagAdded { uuid, tag_id, .. } => {
            let tag = fetch_tag(&repo, tag_id, "TagAdded").await?;
            let all_tags = repo.get_tags(&uuid).await.unwrap_or_default();
            let name = resolve_name(&cache, &uuid).await;
            channel::post_new_tag(ctx, data, &uuid, &name, &tag, &all_tags).await;
        }

        BlacklistEvent::TagOverwritten {
            uuid, old_tag_id, old_tag_type, old_reason, new_tag_id, overwritten_by,
        } => {
            let new_tag = fetch_tag(&repo, new_tag_id, "TagOverwritten").await?;
            let all_tags = repo.get_tags(&uuid).await.unwrap_or_default();
            let name = resolve_name(&cache, &uuid).await;
            let old_tag = mock_old_tag(old_tag_id, &new_tag, old_tag_type, old_reason);
            channel::post_tag_changed(
                ctx, data, &uuid, &name, &old_tag, &new_tag, "Tag Overwritten", overwritten_by as u64,
            )
            .await;
            channel::post_overwritten_tag(ctx, data, &uuid, &name, &new_tag, &all_tags).await;
        }

        BlacklistEvent::TagRemoved { uuid, tag_id, removed_by } => {
            let Some(tag) = repo.get_tag_by_id_any(tag_id).await? else {
                tracing::warn!("tag {tag_id} not found for TagRemoved event");
                return Ok(());
            };
            let name = resolve_name(&cache, &uuid).await;
            channel::post_tag_removed(ctx, data, &uuid, &name, &tag, removed_by as u64).await;
        }

        BlacklistEvent::PlayerLocked { uuid, locked_by, reason } => {
            let name = resolve_name(&cache, &uuid).await;
            channel::post_lock_change(ctx, data, &uuid, &name, true, Some(&reason), locked_by as u64)
                .await;
        }

        BlacklistEvent::PlayerUnlocked { uuid, unlocked_by } => {
            let name = resolve_name(&cache, &uuid).await;
            channel::post_lock_change(ctx, data, &uuid, &name, false, None, unlocked_by as u64)
                .await;
        }

        BlacklistEvent::TagEdited { uuid, tag_id, old_tag_type, old_reason, edited_by } => {
            let new_tag = fetch_tag(&repo, tag_id, "TagEdited").await?;
            let name = resolve_name(&cache, &uuid).await;
            let old_tag = mock_old_tag(tag_id, &new_tag, old_tag_type, old_reason);
            channel::post_tag_changed(
                ctx, data, &uuid, &name, &old_tag, &new_tag, "Tag Modified", edited_by as u64,
            )
            .await;
        }
    }

    Ok(())
}


async fn fetch_tag(
    repo: &BlacklistRepository<'_>,
    tag_id: i64,
    event_name: &str,
) -> anyhow::Result<PlayerTagRow> {
    repo.get_tag_by_id(tag_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("tag {tag_id} not found for {event_name} event"))
}


fn mock_old_tag(id: i64, new_tag: &PlayerTagRow, tag_type: String, reason: String) -> PlayerTagRow {
    PlayerTagRow {
        id,
        player_id: new_tag.player_id,
        tag_type,
        reason,
        added_by: new_tag.added_by,
        added_on: new_tag.added_on,
        hide_username: new_tag.hide_username,
        reviewed_by: new_tag.reviewed_by.clone(),
        removed_by: new_tag.removed_by,
        removed_on: new_tag.removed_on,
        expires_at: new_tag.expires_at,
    }
}


async fn resolve_name(cache: &CacheRepository<'_>, uuid: &str) -> String {
    cache
        .get_username(uuid)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| uuid.to_string())
}
