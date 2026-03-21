use anyhow::Result;
use serde_json::Value;
use serenity::all::{Context, UserId};

use database::{AccountRepository, MemberRepository};

use crate::framework::Data;

pub enum LinkCheck {
    Verified { uuid: String },
    NotVerified,
    PlayerNotFound,
    HypixelNotFound,
}

pub async fn check_link(data: &Data, player: &str, discord_username: &str) -> LinkCheck {
    let stats = match data.api.get_player_stats(player).await {
        Ok(s) => s,
        Err(_) => return LinkCheck::PlayerNotFound,
    };

    let Some(hypixel_data) = stats.hypixel else {
        return LinkCheck::HypixelNotFound;
    };

    if is_discord_linked(&hypixel_data, discord_username) {
        LinkCheck::Verified { uuid: stats.uuid }
    } else {
        LinkCheck::NotVerified
    }
}

pub async fn link_primary(ctx: &Context, data: &Data, discord_id: u64, uuid: &str) -> Result<()> {
    let repo = MemberRepository::new(data.db.pool());
    repo.create(discord_id as i64).await?;
    repo.set_uuid(discord_id as i64, uuid).await?;

    let user_id = UserId::new(discord_id);
    tokio::spawn(crate::sync::sync_user(ctx.clone(), data.clone(), user_id));

    Ok(())
}

pub async fn link_alt(
    ctx: &Context,
    data: &Data,
    discord_id: u64,
    member_id: i64,
    uuid: &str,
) -> Result<()> {
    let repo = MemberRepository::new(data.db.pool());
    let member = repo.get_by_discord_id(discord_id as i64).await?;

    if member.as_ref().and_then(|m| m.uuid.as_ref()).is_none() {
        return link_primary(ctx, data, discord_id, uuid).await;
    }

    let accounts = AccountRepository::new(data.db.pool());
    accounts.add(member_id, uuid).await?;
    Ok(())
}

pub fn is_discord_linked(player: &Value, discord_username: &str) -> bool {
    player
        .get("socialMedia")
        .and_then(|s| s.get("links"))
        .and_then(|l| l.get("DISCORD"))
        .and_then(|d| d.as_str())
        .map(|linked| linked.to_lowercase() == discord_username.to_lowercase())
        .unwrap_or(false)
}
