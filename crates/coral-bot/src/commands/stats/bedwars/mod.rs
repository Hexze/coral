pub mod cards;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::{DateTime, Utc};
use hypixel::parsing::winstreaks;
use hypixel::{BedwarsPlayerStats, GuildInfo, Mode, WinstreakSnapshot, experience_for_level, extract_bedwars_stats};
use image::DynamicImage;
use serenity::all::*;

use database::CacheRepository;
use render::{SessionType, TagIcon};

use crate::framework::Data;
use cards::{render_bedwars, render_session};
use super::{AutoPreset, GameStats, OverallCache, SessionCacheEntry, encode_png, overall, session};


const MODE_CHOICES: &[(Mode, &str)] = &[
    (Mode::Solos, "solos"),
    (Mode::Doubles, "doubles"),
    (Mode::Threes, "threes"),
    (Mode::Fours, "fours"),
    (Mode::FourVFour, "4v4"),
];

const ALL_MODES: &[Mode] = &[
    Mode::Solos, Mode::Doubles, Mode::Threes, Mode::Fours, Mode::FourVFour,
];


fn create_mode_dropdown(
    custom_id: &str,
    cache_key: &str,
    selected: &[Mode],
    stats: &BedwarsPlayerStats,
) -> CreateSelectMenu<'static> {
    let options: Vec<CreateSelectMenuOption<'static>> = MODE_CHOICES
        .iter()
        .map(|(mode, value)| {
            let ms = stats.get_mode_stats(*mode);
            CreateSelectMenuOption::new(mode.display_name(), format!("{value}:{cache_key}"))
                .default_selection(selected.contains(mode))
                .description(format!("{:.2} fkdr, {:.2} wlr", ms.fkdr(), ms.wlr()))
        })
        .collect();

    CreateSelectMenu::new(
        custom_id.to_string(),
        CreateSelectMenuKind::String { options: options.into() },
    )
    .min_values(1)
    .max_values(MODE_CHOICES.len() as u8)
}


fn parse_mode_value(value: &str) -> Option<(&str, Mode)> {
    let (mode_str, cache_key) = value.split_once(':')?;
    Some((cache_key, Mode::from_str(mode_str)?))
}


fn extract_select_modes(component: &ComponentInteraction) -> Option<(&str, Vec<Mode>)> {
    let values = match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values,
        _ => return None,
    };
    let mut cache_key = None;
    let mut modes = Vec::new();
    for v in values {
        let (key, mode) = parse_mode_value(v.as_str())?;
        cache_key = Some(key);
        modes.push(mode);
    }
    Some((cache_key?, modes))
}


pub struct Bedwars;


impl GameStats for Bedwars {
    type Stats = BedwarsPlayerStats;
    type ModeSelection = Vec<Mode>;
    type WinstreakSnapshot = WinstreakSnapshot;

    const GAME_NAME: &'static str = "Bedwars";
    const ATTACHMENT_NAME: &'static str = "bedwars.png";
    const OVERALL_MODE_ID: &'static str = "bedwars_mode";
    const SESSION_MODE_ID: &'static str = "session_mode";
    const SESSION_SWITCH_ID: &'static str = "session_switch";
    const MGMT_RENAME_PREFIX: &'static str = "session_mgmt_rename:";
    const MGMT_DELETE_PREFIX: &'static str = "session_mgmt_delete:";
    const CONFIRM_DELETE_PREFIX: &'static str = "session_confirm_delete:";
    const RENAME_MODAL_PREFIX: &'static str = "session_rename_modal:";

    fn extract_stats(username: &str, data: &serde_json::Value, guild: Option<GuildInfo>) -> Option<BedwarsPlayerStats> {
        extract_bedwars_stats(username, data, guild)
    }

    fn extract_winstreak_snapshot(v: &serde_json::Value) -> Option<WinstreakSnapshot> {
        hypixel::extract_winstreak_snapshot(v)
    }

    fn default_mode(_stats: &BedwarsPlayerStats) -> Vec<Mode> {
        ALL_MODES.to_vec()
    }

    fn create_mode_dropdown(custom_id: &str, cache_key: &str, mode: &Vec<Mode>, stats: &BedwarsPlayerStats) -> CreateSelectMenu<'static> {
        create_mode_dropdown(custom_id, cache_key, mode, stats)
    }

    fn parse_mode_interaction(component: &ComponentInteraction) -> Option<(String, Vec<Mode>)> {
        let (cache_key, modes) = extract_select_modes(component)?;
        Some((cache_key.to_string(), modes))
    }

    fn render_overall(stats: &BedwarsPlayerStats, mode: &Vec<Mode>, skin: Option<&DynamicImage>, snapshots: &[(DateTime<Utc>, WinstreakSnapshot)], tags: &[TagIcon]) -> Result<Vec<u8>> {
        let ws = winstreaks::calculate(snapshots, mode);
        encode_png(&render_bedwars(stats, mode, skin, &ws, tags))
    }

    fn render_session(current: &BedwarsPlayerStats, previous: &BedwarsPlayerStats, session_type: SessionType, started: DateTime<Utc>, mode: &Vec<Mode>, skin: Option<&DynamicImage>, _snapshots: &[(DateTime<Utc>, WinstreakSnapshot)], tags: &[TagIcon]) -> Result<Vec<u8>> {
        encode_png(&render_session(current, previous, session_type, started, None, mode, skin, tags))
    }

    fn format_delta(current: &BedwarsPlayerStats, previous: &BedwarsPlayerStats, modes: &Vec<Mode>) -> String {
        let star_diff = current.level as i64 - previous.level as i64;
        let cur = current.get_combined_mode_stats(modes);
        let prev = previous.get_combined_mode_stats(modes);
        let fk = cur.final_kills as i64 - prev.final_kills as i64;
        let fd = cur.final_deaths as i64 - prev.final_deaths as i64;
        let fkdr = if fd == 0 { fk as f64 } else { fk as f64 / fd as f64 };
        format!("+{}\u{272B}, +{} finals, {:.2} fkdr", star_diff, fk, fkdr)
    }

    async fn detect_auto_presets(cache_repo: &CacheRepository<'_>, uuid: &str, stats: &BedwarsPlayerStats) -> Vec<AutoPreset> {
        detect_auto_presets_bedwars(cache_repo, uuid, stats.level as u64, stats.overall.winstreak.is_none()).await
    }

    fn overall_cache(data: &Data) -> &Arc<Mutex<HashMap<String, OverallCache<Self>>>> {
        &data.bedwars_images
    }

    fn session_cache(data: &Data) -> &Arc<Mutex<HashMap<String, SessionCacheEntry<Self>>>> {
        &data.session_images
    }
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
    overall::run::<Bedwars>(ctx, command, data).await
}


pub async fn handle_mode_switch(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    overall::handle_mode_switch::<Bedwars>(ctx, component, data).await
}


pub async fn session_run(ctx: &Context, command: &CommandInteraction, data: &Data, preferred: Option<&str>) -> Result<()> {
    session::run_with_preferred_view::<Bedwars>(ctx, command, data, preferred).await
}


pub async fn handle_session_switch(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    session::handle_switch::<Bedwars>(ctx, component, data).await
}


pub async fn handle_session_mode_switch(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    session::handle_mode_switch::<Bedwars>(ctx, component, data).await
}


pub async fn handle_mgmt_rename_button(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    session::handle_mgmt_rename_button::<Bedwars>(ctx, component, data).await
}


pub async fn handle_rename_modal(ctx: &Context, modal: &ModalInteraction, data: &Data) -> Result<()> {
    session::handle_rename_modal::<Bedwars>(ctx, modal, data).await
}


pub async fn handle_mgmt_delete_button(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    session::handle_mgmt_delete_button::<Bedwars>(ctx, component, data).await
}


pub async fn handle_confirm_delete_button(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    session::handle_confirm_delete_button::<Bedwars>(ctx, component, data).await
}


struct SnapshotFields {
    experience: u64,
    losses: u64,
}


async fn detect_auto_presets_bedwars(
    cache_repo: &CacheRepository<'_>,
    uuid: &str,
    current_level: u64,
    ws_hidden: bool,
) -> Vec<AutoPreset> {
    let snapshots = cache_repo
        .get_all_snapshots_mapped(uuid, |v| {
            let bw = v.get("stats")?.get("Bedwars")?;
            Some(SnapshotFields {
                experience: bw.get("Experience").and_then(|e| e.as_u64()).unwrap_or(0),
                losses: bw.get("losses_bedwars").and_then(|e| e.as_u64()).unwrap_or(0),
            })
        })
        .await
        .unwrap_or_default();

    if snapshots.is_empty() {
        return vec![];
    }

    let mut presets = Vec::new();

    let current_prestige = current_level / 100;
    if current_prestige > 0 {
        let boundary_xp = experience_for_level(current_prestige * 100);
        if let Some(ts) = snapshots.windows(2).find_map(|w| {
            let (_, before) = &w[0];
            let (ts, after) = &w[1];
            (before.experience < boundary_xp && after.experience >= boundary_xp).then_some(*ts)
        }) {
            presets.push(AutoPreset {
                key: format!("prestige_{current_prestige}"),
                label: format!("Since {}\u{272B}", current_prestige * 100),
                timestamp: ts,
                restricted: false,
            });
        }
    }

    if let Some(ts) = snapshots.windows(2).rev().find_map(|w| {
        let (_, before) = &w[0];
        let (ts, after) = &w[1];
        (after.losses > before.losses).then_some(*ts)
    }) {
        presets.push(AutoPreset {
            key: "last_loss".to_string(),
            label: "Since Last Loss".to_string(),
            timestamp: ts,
            restricted: ws_hidden,
        });
    }

    presets
}
