pub mod bedwars;
pub mod duels;
pub mod prestiges;
mod overall;
pub(crate) mod session;

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use hypixel::GuildInfo;
use image::{DynamicImage, RgbaImage};
use serenity::all::*;

use database::{CacheRepository, Period, SessionMarker};
use render::TagIcon;

use crate::api::{GuildResponse, TagInfo};
use crate::framework::Data;
use render::SessionType;

pub(super) const CACHE_TTL_SECS: u64 = 2 * 60;


const PERIODS: [Period; 4] = [Period::Daily, Period::Weekly, Period::Monthly, Period::Yearly];


pub trait GameStats: Sized + Send + 'static {
    type Stats: Clone + Send + 'static;
    type ModeSelection: Clone + Send + 'static;
    type WinstreakSnapshot: Clone + Send + 'static;

    const GAME_NAME: &'static str;
    const ATTACHMENT_NAME: &'static str;
    const OVERALL_MODE_ID: &'static str;
    const SESSION_MODE_ID: &'static str;
    const SESSION_SWITCH_ID: &'static str;
    const MGMT_RENAME_PREFIX: &'static str;
    const MGMT_DELETE_PREFIX: &'static str;
    const CONFIRM_DELETE_PREFIX: &'static str;
    const RENAME_MODAL_PREFIX: &'static str;

    fn extract_stats(username: &str, data: &serde_json::Value, guild: Option<GuildInfo>) -> Option<Self::Stats>;
    fn extract_winstreak_snapshot(v: &serde_json::Value) -> Option<Self::WinstreakSnapshot>;
    fn default_mode(stats: &Self::Stats) -> Self::ModeSelection;
    fn create_mode_dropdown(custom_id: &str, cache_key: &str, mode: &Self::ModeSelection, stats: &Self::Stats) -> CreateSelectMenu<'static>;
    fn parse_mode_interaction(component: &ComponentInteraction) -> Option<(String, Self::ModeSelection)>;
    fn render_overall(stats: &Self::Stats, mode: &Self::ModeSelection, skin: Option<&DynamicImage>, snapshots: &[(DateTime<Utc>, Self::WinstreakSnapshot)], tags: &[TagIcon]) -> Result<Vec<u8>>;
    fn render_session(current: &Self::Stats, previous: &Self::Stats, session_type: SessionType, started: DateTime<Utc>, mode: &Self::ModeSelection, skin: Option<&DynamicImage>, snapshots: &[(DateTime<Utc>, Self::WinstreakSnapshot)], tags: &[TagIcon]) -> Result<Vec<u8>>;
    fn format_delta(current: &Self::Stats, previous: &Self::Stats, mode: &Self::ModeSelection) -> String;
    async fn detect_auto_presets(cache_repo: &CacheRepository<'_>, uuid: &str, stats: &Self::Stats) -> Vec<AutoPreset>;
    fn overall_cache(data: &Data) -> &Arc<Mutex<HashMap<String, OverallCache<Self>>>>;
    fn session_cache(data: &Data) -> &Arc<Mutex<HashMap<String, SessionCacheEntry<Self>>>>;
}


pub struct OverallCache<G: GameStats> {
    pub stats: G::Stats,
    pub skin: Option<DynamicImage>,
    pub tag_icons: Vec<TagIcon>,
    pub snapshots: Vec<(DateTime<Utc>, G::WinstreakSnapshot)>,
    pub mode: G::ModeSelection,
    pub sender_id: u64,
    pub last_interaction: Instant,
}


pub struct SessionCacheEntry<G: GameStats> {
    pub uuid: String,
    pub sender_id: u64,
    pub is_owner: bool,
    pub descriptions: HashMap<String, String>,
    pub markers: Vec<SessionMarker>,
    pub auto_presets: Vec<AutoPreset>,
    pub current_period: String,
    pub current_mode: G::ModeSelection,
    pub render_data: SessionRenderData<G>,
    pub last_interaction: Instant,
}


pub struct SessionRenderData<G: GameStats> {
    pub current_stats: G::Stats,
    pub previous_stats: HashMap<String, (G::Stats, SessionType, DateTime<Utc>)>,
    pub skin: Option<DynamicImage>,
    pub tag_icons: Vec<TagIcon>,
    pub snapshots: Vec<(DateTime<Utc>, G::WinstreakSnapshot)>,
    pub username: String,
    pub guild_info: Option<GuildInfo>,
}


#[derive(Clone)]
pub struct AutoPreset {
    pub key: String,
    pub label: String,
    pub timestamp: DateTime<Utc>,
    pub restricted: bool,
}


fn player_option() -> CreateCommandOption<'static> {
    CreateCommandOption::new(CommandOptionType::String, "player", "Minecraft username or UUID")
}


fn period_session_type(period: Period) -> SessionType {
    match period {
        Period::Daily => SessionType::Daily,
        Period::Weekly => SessionType::Weekly,
        Period::Monthly => SessionType::Monthly,
        Period::Yearly => SessionType::Yearly,
    }
}


pub(super) fn extract_select_value(component: &ComponentInteraction) -> Option<&str> {
    match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.first().map(|s| s.as_str()),
        _ => None,
    }
}


pub fn encode_png(image: &RgbaImage) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    image.write_to(&mut buf, image::ImageFormat::Png)?;
    Ok(buf.into_inner())
}


pub fn extract_tag_icons(tags: &[TagInfo]) -> Vec<TagIcon> {
    tags.iter()
        .filter_map(|t| blacklist::lookup(&t.tag_type))
        .map(|def| (def.icon.to_string(), def.color))
        .collect()
}


pub(crate) fn looks_like_uuid(s: &str) -> bool {
    let s = s.replace('-', "");
    s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit())
}


pub(crate) fn to_guild_info(guild: &GuildResponse) -> GuildInfo {
    let player = guild.player.as_ref();
    GuildInfo {
        name: Some(guild.name.clone()),
        tag: guild.tag.clone(),
        tag_color: guild.tag_color.clone(),
        rank: player.and_then(|p| p.rank.clone()),
        joined: player.and_then(|p| p.joined),
        weekly_gexp: player.and_then(|p| p.weekly_gexp),
    }
}


pub use crate::interact::send_deferred_error;


pub async fn disable_components(ctx: &Context, component: &ComponentInteraction) -> Result<()> {
    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(extract_gallery_components(&component.message.components)),
            ),
        )
        .await?;
    Ok(())
}


fn extract_gallery_components(components: &[Component]) -> Vec<CreateComponent<'static>> {
    components
        .iter()
        .filter_map(|c| match c {
            Component::MediaGallery(g) => {
                let items: Vec<_> = g
                    .items
                    .iter()
                    .map(|item| {
                        let url = item.media.proxy_url.as_deref().unwrap_or(&item.media.url);
                        CreateMediaGalleryItem::new(CreateUnfurledMediaItem::new(url.to_string()))
                    })
                    .collect();
                Some(CreateComponent::MediaGallery(CreateMediaGallery::new(items)))
            }
            _ => None,
        })
        .collect()
}


pub(super) async fn resolve_uuid(data: &Data, player: &str) -> Option<String> {
    if looks_like_uuid(player) {
        Some(player.replace('-', "").to_lowercase())
    } else {
        CacheRepository::new(data.db.pool())
            .resolve_uuid(player)
            .await
            .ok()
            .flatten()
    }
}


pub(super) fn spawn_expiry<T: Send + 'static>(
    http: Arc<Http>,
    token: String,
    cache: Arc<Mutex<HashMap<String, T>>>,
    cache_key: String,
    get_last_interaction: fn(&T) -> Instant,
) {
    spawn_expiry_with_retain(http, token, cache, cache_key, get_last_interaction, vec![]);
}


pub(super) fn spawn_expiry_with_retain<T: Send + 'static>(
    http: Arc<Http>,
    token: String,
    cache: Arc<Mutex<HashMap<String, T>>>,
    cache_key: String,
    get_last_interaction: fn(&T) -> Instant,
    retain: Vec<CreateComponent<'static>>,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(CACHE_TTL_SECS)).await;

            let remaining = {
                let store = cache.lock().unwrap();
                store.get(&cache_key).and_then(|entry| {
                    let elapsed = get_last_interaction(entry).elapsed().as_secs();
                    (elapsed < CACHE_TTL_SECS).then(|| CACHE_TTL_SECS - elapsed)
                })
            };

            match remaining {
                Some(secs) => tokio::time::sleep(Duration::from_secs(secs)).await,
                None => {
                    cache.lock().unwrap().remove(&cache_key);
                    let mut edit = EditInteractionResponse::new().components(retain.clone());
                    if !retain.is_empty() {
                        edit = edit.flags(MessageFlags::IS_COMPONENTS_V2);
                    }
                    let _ = edit.execute(&http, &token).await;
                    break;
                }
            }
        }
    });
}


pub(super) async fn fetch_skin(
    data: &Data,
    uuid: &str,
    skin_url: Option<&str>,
    slim: bool,
) -> Option<clients::SkinImage> {
    match skin_url {
        Some(url) => data.skin_provider.fetch_with_url(uuid, url, slim).await,
        None => data.skin_provider.fetch(uuid).await,
    }
}


pub(super) enum StatsError {
    PlayerNotFound,
    NoStats(String),
    ApiError,
}


pub(super) fn map_api_error(e: crate::api::ApiError) -> StatsError {
    match e {
        crate::api::ApiError::NotFound => StatsError::PlayerNotFound,
        other => {
            tracing::error!("Internal API error: {other}");
            StatsError::ApiError
        }
    }
}


pub(super) fn evict_expired<T>(cache: &mut HashMap<String, T>, get_last_interaction: fn(&T) -> Instant) {
    cache.retain(|_, v| get_last_interaction(v).elapsed().as_secs() <= CACHE_TTL_SECS);
}


pub(super) fn image_gallery() -> CreateComponent<'static> {
    CreateComponent::MediaGallery(CreateMediaGallery::new(vec![CreateMediaGalleryItem::new(
        CreateUnfurledMediaItem::new("attachment://session.png"),
    )]))
}


pub(super) fn v2_update(
    components: Vec<CreateComponent<'static>>,
    png: Option<Vec<u8>>,
) -> CreateInteractionResponse<'static> {
    let mut all = Vec::with_capacity(components.len() + 1);
    if png.is_some() {
        all.push(image_gallery());
    }
    all.extend(components);

    let mut msg = CreateInteractionResponseMessage::new()
        .flags(MessageFlags::IS_COMPONENTS_V2)
        .components(all);
    if let Some(png) = png {
        msg = msg.add_file(CreateAttachment::bytes(png, "session.png"));
    }
    CreateInteractionResponse::UpdateMessage(msg)
}


pub(super) async fn update_original_components(
    ctx: &Context,
    component: &ComponentInteraction,
    components: Vec<CreateComponent<'static>>,
) {
    let mut all = Vec::with_capacity(components.len() + 1);
    all.push(image_gallery());
    all.extend(components);

    let edit = EditMessage::new()
        .flags(MessageFlags::IS_COMPONENTS_V2)
        .components(all);
    let msg = &component.message;
    let _ = ctx.http.edit_message(msg.channel_id, msg.id, &edit, Vec::new()).await;
}


pub(super) fn sanitize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '*' | '_' | '~' | '`' | '|' | '>' | '\\' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}


pub(super) fn format_duration(duration: ChronoDuration) -> String {
    let total_hours = duration.num_hours();
    if total_hours >= 24 {
        format!("{}d", duration.num_days())
    } else if total_hours >= 1 {
        let minutes = duration.num_minutes() % 60;
        if minutes > 0 { format!("{}h {}m", total_hours, minutes) }
        else { format!("{}h", total_hours) }
    } else {
        format!("{}m", duration.num_minutes().max(1))
    }
}


pub(super) fn view_display_name(view: &str) -> String {
    view.strip_prefix("marker:").unwrap_or(view).to_string()
}


pub(super) fn extract_modal_field<'a>(modal: &'a ModalInteraction, field_name: &str) -> Option<&'a str> {
    modal.data.components.iter().find_map(|c| {
        if let Component::Label(label) = c {
            if let LabelComponent::InputText(input) = &label.component {
                if input.custom_id == field_name {
                    return input.value.as_ref().map(|v| v.as_str());
                }
            }
        }
        None
    })
}


pub(super) async fn send_ephemeral_modal(ctx: &Context, modal: &ModalInteraction, content: &str) -> Result<()> {
    modal
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new().content(content).ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}


fn create_session_dropdown<G: GameStats>(
    cache_key: &str,
    current: &str,
    descriptions: &HashMap<String, String>,
    markers: &[SessionMarker],
    auto_presets: &[AutoPreset],
    is_owner: bool,
) -> CreateSelectMenu<'static> {
    let mut options: Vec<CreateSelectMenuOption<'static>> = Vec::new();
    let now = Utc::now();

    for period in PERIODS {
        let key = period.key();
        let desc = descriptions.get(key).map(String::as_str).unwrap_or("No Data");

        options.push(
            CreateSelectMenuOption::new(
                period.dropdown_label(now),
                format!("{key}:{cache_key}"),
            )
            .default_selection(current == key)
            .description(desc.to_string()),
        );

        if let Some((fp_key, fp_label)) = period.fixed_preset() {
            let fp_desc = descriptions.get(fp_key).map(String::as_str).unwrap_or("No Data");
            options.push(
                CreateSelectMenuOption::new(fp_label, format!("{fp_key}:{cache_key}"))
                    .default_selection(current == fp_key)
                    .description(fp_desc.to_string()),
            );
        }
    }

    for preset in auto_presets {
        if preset.restricted && !is_owner {
            continue;
        }
        let key = format!("preset:{}", preset.key);
        let age = format_duration(now.signed_duration_since(preset.timestamp));
        let mut option = CreateSelectMenuOption::new(
            format!("{} ({})", preset.label, age),
            format!("preset:{}:{}", cache_key, preset.key),
        )
        .default_selection(current == key);
        if preset.restricted {
            option = option.description("***");
        } else if let Some(desc) = descriptions.get(&key) {
            option = option.description(desc.clone());
        }
        options.push(option);
    }

    let remaining_slots = 25 - options.len() - if is_owner { 1 } else { 0 };
    for marker in markers.iter().take(remaining_slots) {
        let key = format!("marker:{}", marker.name);
        let age = format_duration(now.signed_duration_since(marker.snapshot_timestamp));
        let mut option = CreateSelectMenuOption::new(
            format!("\"{}\" ({})", sanitize(&marker.name), age),
            format!("marker:{}:{}", cache_key, marker.name),
        )
        .default_selection(current == key);
        if let Some(desc) = descriptions.get(&key) {
            option = option.description(desc.clone());
        }
        options.push(option);
    }

    if is_owner {
        options.push(
            CreateSelectMenuOption::new("Create New Bookmark", format!("create:{cache_key}"))
                .description("Bookmark your current stats"),
        );
    }

    let placeholder = PERIODS
        .iter()
        .find(|p| p.key() == current)
        .map(|p| p.dropdown_label(now))
        .or_else(|| match current {
            "past_24h" => Some("Past 24 Hours".to_string()),
            "past_7d" => Some("Past 7 Days".to_string()),
            "past_30d" => Some("Past 30 Days".to_string()),
            _ => None,
        })
        .unwrap_or_else(|| {
            auto_presets
                .iter()
                .find(|p| format!("preset:{}", p.key) == current)
                .map(|p| p.label.clone())
                .unwrap_or_else(|| view_display_name(current))
        });

    CreateSelectMenu::new(
        G::SESSION_SWITCH_ID,
        CreateSelectMenuKind::String { options: options.into() },
    )
    .placeholder(placeholder)
}
