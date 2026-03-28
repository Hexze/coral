use std::collections::HashMap;
use std::time::Instant;

use anyhow::Result;
use chrono::{DateTime, Utc};
use hypixel::parsing::winstreaks;
use hypixel::{BedwarsPlayerStats, Mode, WinstreakSnapshot, extract_bedwars_stats, extract_winstreak_snapshot};
use image::DynamicImage;
use serenity::all::*;
use tracing::debug;

use database::{CacheRepository, MemberRepository};
use render::TagIcon;

use crate::framework::Data;
use crate::rendering::render_bedwars;
use super::{
    CACHE_TTL_SECS, create_mode_dropdown, disable_components, encode_png, extract_select_modes,
    extract_tag_icons, fetch_skin, resolve_uuid, send_deferred_error, spawn_expiry,
};


pub struct BedwarsCache {
    pub stats: BedwarsPlayerStats,
    pub skin: Option<DynamicImage>,
    pub tag_icons: Vec<TagIcon>,
    pub snapshots: Vec<(DateTime<Utc>, WinstreakSnapshot)>,
    pub modes: Vec<Mode>,
    pub sender_id: u64,
    pub last_interaction: Instant,
}


enum StatsError {
    PlayerNotFound,
    NoStats(String),
    ApiError,
}


enum CacheResult {
    Ok(Vec<u8>, CreateActionRow<'static>),
    Expired,
    Ephemeral(Vec<u8>),
}


pub fn register() -> CreateCommand<'static> {
    CreateCommand::new("bedwars")
        .description("View a player's Bedwars stats")
        .add_option(CreateCommandOption::new(
            CommandOptionType::String,
            "player",
            "Player name or UUID",
        ))
}


pub async fn run(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    let t = Instant::now();

    let player_input = command.data.options.first()
        .and_then(|o| o.value.as_str())
        .map(|s| s.to_string());
    let sender_id = command.user.id.get();
    let cache_key = command.id.to_string();

    let player = match player_input {
        Some(p) => p,
        None => {
            match MemberRepository::new(data.db.pool())
                .get_by_discord_id(sender_id as i64)
                .await
                .ok()
                .flatten()
                .and_then(|m| m.uuid)
            {
                Some(uuid) => uuid,
                None => {
                    command.defer(&ctx.http).await?;
                    return send_deferred_error(ctx, command, "Not Linked", "Link your account or provide a player name").await;
                }
            }
        }
    };

    let (defer_result, result) = tokio::join!(
        command.defer(&ctx.http),
        fetch_player_data(data, &player),
    );
    defer_result?;
    debug!(at = ?t.elapsed(), "fetch done");

    match result {
        Ok(mut cache) => {
            cache.sender_id = sender_id;
            let png = render_and_encode(&cache)?;
            debug!(at = ?t.elapsed(), "render done");

            let mode_row = CreateActionRow::SelectMenu(create_mode_dropdown(
                "bedwars_mode", &cache_key, &cache.modes, &cache.stats,
            ));
            let expiry_key = cache_key.clone();

            {
                let mut store = data.bedwars_images.lock().unwrap();
                evict_expired(&mut store);
                store.insert(cache_key, cache);
            }

            command
                .edit_response(
                    &ctx.http,
                    EditInteractionResponse::new()
                        .new_attachment(CreateAttachment::bytes(png, "bedwars.png"))
                        .components(vec![CreateComponent::ActionRow(mode_row)]),
                )
                .await?;

            spawn_expiry(
                ctx.http.clone(),
                command.token.to_string(),
                data.bedwars_images.clone(),
                expiry_key,
                |e: &BedwarsCache| e.last_interaction,
            );
            debug!(player = %player, at = ?t.elapsed(), "send done");
        }
        Err(StatsError::PlayerNotFound) => {
            send_deferred_error(ctx, command, "Player Not Found", &format!("Could not find player: {player}")).await?;
        }
        Err(StatsError::NoStats(username)) => {
            send_deferred_error(ctx, command, &format!("{username}'s Bedwars Stats"), "This player has no Bedwars stats").await?;
        }
        Err(StatsError::ApiError) => {
            send_deferred_error(ctx, command, "Error", "Something went wrong. Please try again later.").await?;
        }
    }

    Ok(())
}


pub async fn handle_mode_switch(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let Some((cache_key, modes)) = extract_select_modes(component) else { return Ok(()) };

    match resolve_mode_switch(data, cache_key, &modes, component.user.id.get()) {
        CacheResult::Ok(png, mode_row) => {
            component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .add_file(CreateAttachment::bytes(png, "bedwars.png"))
                            .components(vec![CreateComponent::ActionRow(mode_row)]),
                    ),
                )
                .await?;
        }
        CacheResult::Expired => {
            disable_components(ctx, component).await?;
        }
        CacheResult::Ephemeral(png) => {
            component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .add_file(CreateAttachment::bytes(png, "bedwars.png"))
                            .ephemeral(true),
                    ),
                )
                .await?;
        }
    }

    Ok(())
}


fn resolve_mode_switch(data: &Data, cache_key: &str, modes: &[Mode], user_id: u64) -> CacheResult {
    let mut store = data.bedwars_images.lock().unwrap();

    let Some(entry) = store.get_mut(cache_key) else {
        return CacheResult::Expired;
    };
    if entry.last_interaction.elapsed().as_secs() > CACHE_TTL_SECS {
        store.remove(cache_key);
        return CacheResult::Expired;
    }
    if entry.sender_id != user_id {
        return render_ephemeral(entry, modes);
    }

    entry.modes = modes.to_vec();
    entry.last_interaction = Instant::now();
    let mode_row = CreateActionRow::SelectMenu(create_mode_dropdown(
        "bedwars_mode", cache_key, &entry.modes, &entry.stats,
    ));

    match render_and_encode(entry) {
        Ok(png) => CacheResult::Ok(png, mode_row),
        Err(_) => CacheResult::Expired,
    }
}


fn render_ephemeral(entry: &mut BedwarsCache, modes: &[Mode]) -> CacheResult {
    let original = std::mem::replace(&mut entry.modes, modes.to_vec());
    let result = render_and_encode(entry);
    entry.modes = original;
    match result {
        Ok(png) => CacheResult::Ephemeral(png),
        Err(_) => CacheResult::Expired,
    }
}


fn render_and_encode(cache: &BedwarsCache) -> Result<Vec<u8>> {
    let winstreaks = winstreaks::calculate(&cache.snapshots, &cache.modes);
    encode_png(&render_bedwars(
        &cache.stats, &cache.modes, cache.skin.as_ref(), &winstreaks, &cache.tag_icons,
    ))
}


async fn fetch_player_data(data: &Data, player: &str) -> Result<BedwarsCache, StatsError> {
    let t = Instant::now();
    let cached_uuid = resolve_uuid(data, player).await;
    debug!(at = ?t.elapsed(), cached = cached_uuid.is_some(), "resolve");

    let (resp, guild_result, skin_result, history_result) = match cached_uuid {
        Some(ref uuid) => {
            let cache_repo = CacheRepository::new(data.db.pool());
            let (api, guild, skin, history) = tokio::join!(
                data.api.get_player_stats(player),
                data.api.get_guild(uuid, Some("player")),
                data.skin_provider.fetch(uuid),
                cache_repo.get_all_snapshots_mapped(uuid, extract_winstreak_snapshot),
            );
            let resp = api.map_err(map_api_error)?;

            if resp.uuid == *uuid {
                (resp, guild, skin, history)
            } else {
                let cache_repo = CacheRepository::new(data.db.pool());
                let (guild, skin, history) = tokio::join!(
                    data.api.get_guild(&resp.uuid, Some("player")),
                    fetch_skin(data, &resp.uuid, resp.skin_url.as_deref(), resp.slim),
                    cache_repo.get_all_snapshots_mapped(&resp.uuid, extract_winstreak_snapshot),
                );
                (resp, guild, skin, history)
            }
        }
        None => {
            let resp = data.api.get_player_stats(player).await.map_err(map_api_error)?;
            let cache_repo = CacheRepository::new(data.db.pool());
            let (guild, skin, history) = tokio::join!(
                data.api.get_guild(&resp.uuid, Some("player")),
                fetch_skin(data, &resp.uuid, resp.skin_url.as_deref(), resp.slim),
                cache_repo.get_all_snapshots_mapped(&resp.uuid, extract_winstreak_snapshot),
            );
            (resp, guild, skin, history)
        }
    };
    debug!(at = ?t.elapsed(), "api done");

    let hypixel_data = resp.hypixel.ok_or(StatsError::PlayerNotFound)?;
    let username = resp.username.clone();
    let guild_info = guild_result.ok().flatten().map(|g| super::to_guild_info(&g));
    let stats = extract_bedwars_stats(&username, &hypixel_data, guild_info)
        .ok_or_else(|| StatsError::NoStats(username.clone()))?;
    let snapshots = history_result.ok().unwrap_or_default();
    debug!(at = ?t.elapsed(), snapshots = snapshots.len(), "parse done");

    Ok(BedwarsCache {
        stats,
        skin: skin_result.map(|s| s.data),
        tag_icons: extract_tag_icons(&resp.tags),
        snapshots,
        modes: super::ALL_MODES.to_vec(),
        sender_id: 0,
        last_interaction: Instant::now(),
    })
}


fn map_api_error(e: crate::api::ApiError) -> StatsError {
    match e {
        crate::api::ApiError::NotFound => StatsError::PlayerNotFound,
        other => {
            tracing::error!("Internal API error: {other}");
            StatsError::ApiError
        }
    }
}


fn evict_expired(cache: &mut HashMap<String, BedwarsCache>) {
    cache.retain(|_, v| v.last_interaction.elapsed().as_secs() <= CACHE_TTL_SECS);
}
