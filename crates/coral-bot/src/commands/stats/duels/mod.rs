pub mod cards;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::{DateTime, Utc};
use hypixel::parsing::duels_winstreaks;
use hypixel::{DuelsStats, DuelsView, DuelsWinstreak, DuelsWinstreakSnapshot, GuildInfo, extract_duels_stats, extract_duels_winstreak_snapshot};
use image::DynamicImage;
use serenity::all::*;

use database::CacheRepository;
use render::{SessionType, TagIcon};

use crate::framework::Data;
use cards::{render_duels, render_duels_session};
use super::{AutoPreset, GameStats, OverallCache, SessionCacheEntry, encode_png, extract_select_value, overall, session};


fn create_duels_dropdown(
    custom_id: &str,
    cache_key: &str,
    current: DuelsView,
    stats: &DuelsStats,
) -> CreateSelectMenu<'static> {
    let options: Vec<CreateSelectMenuOption<'static>> = stats
        .active_views()
        .into_iter()
        .filter_map(|view| {
            let view_stats = stats.view_stats(view)?;
            let title = view_stats.title.clone();
            let description = format!(
                "{:.2} W/L, {:.2} K/D",
                hypixel::ratio(view_stats.wins, view_stats.losses),
                hypixel::ratio(view_stats.kills, view_stats.deaths),
            );
            Some(
                CreateSelectMenuOption::new(title, format!("{}:{cache_key}", view.slug()))
                    .default_selection(view == current)
                    .description(description),
            )
        })
        .collect();

    let placeholder = stats
        .view_stats(current)
        .map(|view| view.title)
        .unwrap_or_else(|| "Overall".to_string());

    CreateSelectMenu::new(
        custom_id.to_string(),
        CreateSelectMenuKind::String { options: options.into() },
    )
    .placeholder(placeholder)
}


fn parse_duels_value(value: &str) -> Option<(&str, DuelsView)> {
    let (view_str, cache_key) = value.split_once(':')?;
    Some((cache_key, DuelsView::from_slug(view_str)?))
}


pub struct Duels;


impl GameStats for Duels {
    type Stats = DuelsStats;
    type ModeSelection = DuelsView;
    type WinstreakSnapshot = DuelsWinstreakSnapshot;

    const GAME_NAME: &'static str = "Duels";
    const ATTACHMENT_NAME: &'static str = "duels.png";
    const OVERALL_MODE_ID: &'static str = "duels_mode";
    const SESSION_MODE_ID: &'static str = "session_duels_mode";
    const SESSION_SWITCH_ID: &'static str = "session_duels_switch";
    const MGMT_RENAME_PREFIX: &'static str = "session_duels_mgmt_rename:";
    const MGMT_DELETE_PREFIX: &'static str = "session_duels_mgmt_delete:";
    const CONFIRM_DELETE_PREFIX: &'static str = "session_duels_confirm_delete:";
    const RENAME_MODAL_PREFIX: &'static str = "session_duels_rename_modal:";

    fn extract_stats(username: &str, data: &serde_json::Value, guild: Option<GuildInfo>) -> Option<DuelsStats> {
        extract_duels_stats(username, data, guild)
    }

    fn extract_winstreak_snapshot(v: &serde_json::Value) -> Option<DuelsWinstreakSnapshot> {
        extract_duels_winstreak_snapshot(v)
    }

    fn default_mode(stats: &DuelsStats) -> DuelsView {
        stats.default_view()
    }

    fn create_mode_dropdown(custom_id: &str, cache_key: &str, mode: &DuelsView, stats: &DuelsStats) -> CreateSelectMenu<'static> {
        create_duels_dropdown(custom_id, cache_key, *mode, stats)
    }

    fn parse_mode_interaction(component: &ComponentInteraction) -> Option<(String, DuelsView)> {
        let value = extract_select_value(component)?;
        let (cache_key, view) = parse_duels_value(value)?;
        Some((cache_key.to_string(), view))
    }

    fn render_overall(stats: &DuelsStats, mode: &DuelsView, skin: Option<&DynamicImage>, snapshots: &[(DateTime<Utc>, DuelsWinstreakSnapshot)], tags: &[TagIcon]) -> Result<Vec<u8>> {
        let ws = duels_winstreaks::calculate(snapshots, *mode);
        encode_png(&render_duels(stats, *mode, skin, &ws, tags))
    }

    fn render_session(current: &DuelsStats, previous: &DuelsStats, session_type: SessionType, started: DateTime<Utc>, mode: &DuelsView, skin: Option<&DynamicImage>, snapshots: &[(DateTime<Utc>, DuelsWinstreakSnapshot)], tags: &[TagIcon]) -> Result<Vec<u8>> {
        let ws = duels_winstreaks::calculate(snapshots, *mode);
        encode_png(&render_duels_session(current, previous, session_type, started, None, *mode, skin, &ws, tags))
    }

    fn format_delta(current: &DuelsStats, previous: &DuelsStats, _mode: &DuelsView) -> String {
        let wins = current.overview.wins.saturating_sub(previous.overview.wins);
        let kills = current.overview.kills.saturating_sub(previous.overview.kills);
        let deaths = current.overview.deaths.saturating_sub(previous.overview.deaths);
        let kd = if deaths == 0 { kills as f64 } else { kills as f64 / deaths as f64 };
        format!("+{} wins, +{} kills, {:.2} kd", wins, kills, kd)
    }

    async fn detect_auto_presets(cache_repo: &CacheRepository<'_>, uuid: &str, stats: &DuelsStats) -> Vec<AutoPreset> {
        let ws_hidden = matches!(stats.overview.current_winstreak, DuelsWinstreak::Unknown);
        detect_auto_presets_duels(cache_repo, uuid, ws_hidden).await
    }

    fn overall_cache(data: &Data) -> &Arc<Mutex<HashMap<String, OverallCache<Self>>>> {
        &data.duels_images
    }

    fn session_cache(data: &Data) -> &Arc<Mutex<HashMap<String, SessionCacheEntry<Self>>>> {
        &data.session_duels_images
    }
}


pub fn register() -> CreateCommand<'static> {
    CreateCommand::new("duels")
        .description("View a player's Duels stats")
        .add_option(CreateCommandOption::new(
            CommandOptionType::String,
            "player",
            "Player name or UUID",
        ))
}


pub async fn run(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    overall::run::<Duels>(ctx, command, data).await
}


pub async fn handle_mode_switch(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    overall::handle_mode_switch::<Duels>(ctx, component, data).await
}


pub async fn session_run(ctx: &Context, command: &CommandInteraction, data: &Data, preferred: Option<&str>) -> Result<()> {
    session::run_with_preferred_view::<Duels>(ctx, command, data, preferred).await
}


pub async fn handle_session_switch(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    session::handle_switch::<Duels>(ctx, component, data).await
}


pub async fn handle_session_mode_switch(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    session::handle_mode_switch::<Duels>(ctx, component, data).await
}


pub async fn handle_mgmt_rename_button(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    session::handle_mgmt_rename_button::<Duels>(ctx, component, data).await
}


pub async fn handle_rename_modal(ctx: &Context, modal: &ModalInteraction, data: &Data) -> Result<()> {
    session::handle_rename_modal::<Duels>(ctx, modal, data).await
}


pub async fn handle_mgmt_delete_button(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    session::handle_mgmt_delete_button::<Duels>(ctx, component, data).await
}


pub async fn handle_confirm_delete_button(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    session::handle_confirm_delete_button::<Duels>(ctx, component, data).await
}


struct SnapshotFields {
    losses: u64,
}


async fn detect_auto_presets_duels(cache_repo: &CacheRepository<'_>, uuid: &str, ws_hidden: bool) -> Vec<AutoPreset> {
    let snapshots = cache_repo
        .get_all_snapshots_mapped(uuid, |value| {
            let duels = value.get("stats")?.get("Duels")?;
            Some(SnapshotFields {
                losses: duels.get("losses").and_then(|entry| entry.as_u64()).unwrap_or(0),
            })
        })
        .await
        .unwrap_or_default();

    if snapshots.is_empty() {
        return vec![];
    }

    let mut presets = Vec::new();
    if let Some(timestamp) = snapshots.windows(2).rev().find_map(|window| {
        let (_, before) = &window[0];
        let (timestamp, after) = &window[1];
        (after.losses > before.losses).then_some(*timestamp)
    }) {
        presets.push(AutoPreset {
            key: "since_loss".to_string(),
            label: "Since Last Loss".to_string(),
            timestamp,
            restricted: ws_hidden,
        });
    }

    presets
}
