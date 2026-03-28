pub mod bedwars;
pub mod prestiges;
pub mod session;

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use hypixel::{BedwarsPlayerStats, GuildInfo, Mode};
use image::RgbaImage;
use serenity::all::*;

use database::CacheRepository;
use render::TagIcon;

use crate::api::{GuildResponse, TagInfo};
use crate::framework::Data;


pub(super) const CACHE_TTL_SECS: u64 = 2 * 60;

const MODE_CHOICES: &[(Mode, &str)] = &[
    (Mode::Solos, "solos"),
    (Mode::Doubles, "doubles"),
    (Mode::Threes, "threes"),
    (Mode::Fours, "fours"),
    (Mode::FourVFour, "4v4"),
];

pub const ALL_MODES: &[Mode] = &[
    Mode::Solos, Mode::Doubles, Mode::Threes, Mode::Fours, Mode::FourVFour,
];


pub fn create_mode_dropdown(
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


pub fn parse_mode_value(value: &str) -> Option<(&str, Mode)> {
    let (mode_str, cache_key) = value.split_once(':')?;
    Some((cache_key, Mode::from_str(mode_str)?))
}


pub fn extract_select_value(component: &ComponentInteraction) -> Option<&str> {
    match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.first().map(|s| s.as_str()),
        _ => None,
    }
}


pub fn extract_select_modes(component: &ComponentInteraction) -> Option<(&str, Vec<Mode>)> {
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
