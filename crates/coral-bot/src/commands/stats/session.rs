use std::collections::HashMap;
use std::time::Instant;

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use hypixel::parsing::bedwars::{GuildInfo, Stats};
use hypixel::{Mode, experience_for_level, extract_bedwars_stats};
use image::DynamicImage;
use serenity::all::*;
use tracing::debug;

use database::{AccountRepository, CacheRepository, MemberRepository, Period, SessionMarker, SessionRepository};
use render::TagIcon;

use crate::framework::Data;
use crate::rendering::{SessionType, render_session};
use super::{
    CACHE_TTL_SECS, create_mode_dropdown, disable_components, encode_png, extract_select_value,
    extract_tag_icons, fetch_skin, parse_mode_value, resolve_uuid, send_deferred_error,
    spawn_expiry_with_retain,
};


#[derive(Clone)]
struct AutoPreset {
    key: String,
    label: String,
    timestamp: DateTime<Utc>,
}


pub struct SessionCache {
    uuid: String,
    sender_id: u64,
    is_owner: bool,
    images: HashMap<String, Vec<u8>>,
    descriptions: HashMap<String, String>,
    markers: Vec<SessionMarker>,
    auto_presets: Vec<AutoPreset>,
    mode: Mode,
    current_view: String,
    render_data: SessionRenderData,
    last_interaction: Instant,
}


struct SessionRenderData {
    current_stats: Stats,
    previous_stats: HashMap<String, (Stats, SessionType, DateTime<Utc>)>,
    skin: Option<DynamicImage>,
    tag_icons: Vec<TagIcon>,
    username: String,
    guild_info: Option<GuildInfo>,
}


fn period_session_type(period: Period) -> SessionType {
    match period {
        Period::Daily => SessionType::Daily,
        Period::Weekly => SessionType::Weekly,
        Period::Monthly => SessionType::Monthly,
        Period::Yearly => SessionType::Yearly,
    }
}


const PERIODS: [Period; 4] = [Period::Daily, Period::Weekly, Period::Monthly, Period::Yearly];


enum SessionError {
    PlayerNotFound,
    NoStats(String),
    ApiError,
}


enum SwitchResult {
    Ok(Vec<u8>, Vec<CreateComponent<'static>>),
    Expired,
    Ephemeral(Vec<u8>),
}


enum ModeOwnership {
    Sender,
    Ephemeral(Vec<u8>),
    Expired,
}


fn image_gallery() -> CreateComponent<'static> {
    CreateComponent::MediaGallery(CreateMediaGallery::new(vec![CreateMediaGalleryItem::new(
        CreateUnfurledMediaItem::new("attachment://session.png"),
    )]))
}


fn sanitize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '*' | '_' | '~' | '`' | '|' | '>' | '\\' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}


fn v2_update(
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


fn view_display_name(view: &str) -> String {
    view.strip_prefix("marker:").unwrap_or(view).to_string()
}


fn format_duration(duration: Duration) -> String {
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


fn format_stats_delta(current: &Stats, previous: &Stats, mode: Mode) -> String {
    let star_diff = current.level as i64 - previous.level as i64;
    let cur = current.get_mode_stats(mode);
    let prev = previous.get_mode_stats(mode);
    let fk = cur.final_kills as i64 - prev.final_kills as i64;
    let fd = cur.final_deaths as i64 - prev.final_deaths as i64;
    let fkdr = if fd == 0 { fk as f64 } else { fk as f64 / fd as f64 };
    format!("+{}\u{272B}, +{} finals, {:.2} fkdr", star_diff, fk, fkdr)
}


fn extract_modal_field<'a>(modal: &'a ModalInteraction, field_name: &str) -> Option<&'a str> {
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


async fn send_ephemeral_modal(ctx: &Context, modal: &ModalInteraction, content: &str) -> Result<()> {
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


async fn update_original_components(
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


fn evict_expired(cache: &mut HashMap<String, SessionCache>) {
    cache.retain(|_, v| v.last_interaction.elapsed().as_secs() <= CACHE_TTL_SECS);
}


fn build_session_components(
    cache_key: &str,
    uuid: &str,
    current_view: &str,
    mode: Mode,
    stats: &Stats,
    descriptions: &HashMap<String, String>,
    markers: &[SessionMarker],
    auto_presets: &[AutoPreset],
    is_owner: bool,
) -> Vec<CreateComponent<'static>> {
    let mode_row = CreateComponent::ActionRow(CreateActionRow::SelectMenu(create_mode_dropdown(
        "session_mode", cache_key, mode, stats,
    )));

    let mut container_rows = vec![CreateContainerComponent::ActionRow(
        CreateActionRow::SelectMenu(create_session_dropdown(
            cache_key, current_view, descriptions, markers, auto_presets, is_owner,
        )),
    )];

    if let Some(marker_name) = current_view.strip_prefix("marker:") {
        if is_owner {
            container_rows.push(CreateContainerComponent::ActionRow(
                CreateActionRow::Buttons(
                    vec![
                        CreateButton::new(format!("session_mgmt_rename:{cache_key}:{uuid}:{marker_name}"))
                            .label("Rename")
                            .style(ButtonStyle::Primary),
                        CreateButton::new(format!("session_mgmt_delete:{cache_key}:{uuid}:{marker_name}"))
                            .label("Delete")
                            .style(ButtonStyle::Danger),
                    ]
                    .into(),
                ),
            ));
        }
    }

    vec![
        mode_row,
        CreateComponent::Container(CreateContainer::new(container_rows)),
    ]
}


fn create_session_dropdown(
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
        let desc = descriptions.get(key).map(|s| s.as_str()).unwrap_or("No Data");
        let elapsed = now.signed_duration_since(period.last_reset(now));

        options.push(
            CreateSelectMenuOption::new(
                format!("{} ({})", period.label(), format_duration(elapsed)),
                format!("{key}:{cache_key}"),
            )
            .default_selection(current == key)
            .description(desc.to_string()),
        );

        if let Some((fp_key, fp_label)) = period.fixed_preset() {
            let fp_desc = descriptions.get(fp_key).map(|s| s.as_str()).unwrap_or("No Data");
            options.push(
                CreateSelectMenuOption::new(fp_label, format!("{fp_key}:{cache_key}"))
                    .default_selection(current == fp_key)
                    .description(fp_desc.to_string()),
            );
        }
    }

    for preset in auto_presets {
        let key = format!("preset:{}", preset.key);
        let age = format_duration(now.signed_duration_since(preset.timestamp));
        let mut option = CreateSelectMenuOption::new(
            format!("{} ({})", preset.label, age),
            format!("preset:{}:{}", cache_key, preset.key),
        )
        .default_selection(current == key);
        if let Some(desc) = descriptions.get(&key) {
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
        .map(|p| p.label().to_string())
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
        "session_switch",
        CreateSelectMenuKind::String { options: options.into() },
    )
    .placeholder(placeholder)
}


pub fn register() -> CreateCommand<'static> {
    CreateCommand::new("session")
        .description("View your session stats over time")
        .add_option(CreateCommandOption::new(
            CommandOptionType::String,
            "player",
            "Minecraft username or UUID",
        ))
}


pub async fn run(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    let player_input = command.data.options.first()
        .and_then(|o| o.value.as_str())
        .map(|s| s.to_string());
    let discord_id = command.user.id.get() as i64;

    let player = match player_input {
        Some(p) => p,
        None => {
            match MemberRepository::new(data.db.pool())
                .get_by_discord_id(discord_id)
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

    let cache_key = command.id.to_string();
    let (defer_result, result) = tokio::join!(
        command.defer(&ctx.http),
        precompute_session(data, &player, discord_id, Mode::Overall),
    );
    defer_result?;

    match result {
        Ok(session_cache) => {
            let initial_view = PERIODS
                .iter()
                .map(|p| p.key())
                .find(|k| session_cache.images.contains_key(*k))
                .unwrap_or("daily");
            let initial_png = session_cache.images.get(initial_view).cloned();
            let uuid = session_cache.uuid.clone();

            let is_owner = AccountRepository::new(data.db.pool())
                .is_owned_by(&uuid, discord_id)
                .await
                .unwrap_or(false);

            let components = build_session_components(
                &cache_key, &uuid, initial_view, Mode::Overall,
                &session_cache.render_data.current_stats, &session_cache.descriptions,
                &session_cache.markers, &session_cache.auto_presets, is_owner,
            );
            let expiry_key = cache_key.clone();

            {
                let mut cache = data.session_images.lock().unwrap();
                evict_expired(&mut cache);
                let mut sc = session_cache;
                sc.current_view = initial_view.to_string();
                sc.is_owner = is_owner;
                cache.insert(cache_key, sc);
            }

            if let Some(png) = initial_png {
                let mut all = vec![image_gallery()];
                all.extend(components);

                command
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .flags(MessageFlags::IS_COMPONENTS_V2)
                            .new_attachment(CreateAttachment::bytes(png, "session.png"))
                            .components(all),
                    )
                    .await?;

                spawn_expiry_with_retain(
                    ctx.http.clone(), command.token.to_string(),
                    data.session_images.clone(), expiry_key,
                    |e: &SessionCache| e.last_interaction,
                    vec![image_gallery()],
                );
            } else {
                send_deferred_error(ctx, command, "No Historical Data", "No snapshot data available yet. Check back later!").await?;
            }
        }
        Err(SessionError::PlayerNotFound) => {
            send_deferred_error(ctx, command, "Player Not Found", &format!("Could not find player: {player}")).await?;
        }
        Err(SessionError::NoStats(username)) => {
            send_deferred_error(ctx, command, &format!("{username}'s Session Stats"), "This player has no Bedwars stats").await?;
        }
        Err(SessionError::ApiError) => {
            send_deferred_error(ctx, command, "Error", "Something went wrong. Please try again later.").await?;
        }
    }

    Ok(())
}


pub async fn handle_switch(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let Some(value) = extract_select_value(component) else { return Ok(()) };

    let parts: Vec<&str> = value.splitn(3, ':').collect();
    if parts.len() < 2 { return Ok(()) }
    let (selection, cache_key, extra) = (parts[0], parts[1], parts.get(2).copied());

    if selection == "create" {
        return handle_create_bookmark(ctx, component, data, cache_key).await;
    }

    let image_key = match selection {
        "marker" => format!("marker:{}", extra.unwrap_or("")),
        "preset" => format!("preset:{}", extra.unwrap_or("")),
        _ => selection.to_string(),
    };

    match resolve_view_switch(data, cache_key, &image_key, component.user.id.get()) {
        SwitchResult::Ok(png, components) => {
            component.create_response(&ctx.http, v2_update(components, Some(png))).await?;
        }
        SwitchResult::Expired => {
            disable_components(ctx, component).await?;
        }
        SwitchResult::Ephemeral(png) => {
            component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .add_file(CreateAttachment::bytes(png, "session.png"))
                            .ephemeral(true),
                    ),
                )
                .await?;
        }
    }

    Ok(())
}


pub async fn handle_mode_switch(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let Some(value) = extract_select_value(component) else { return Ok(()) };
    let Some((cache_key, mode)) = parse_mode_value(value) else { return Ok(()) };

    match check_mode_sender(data, cache_key, mode, component.user.id.get()) {
        ModeOwnership::Ephemeral(png) => {
            component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .add_file(CreateAttachment::bytes(png, "session.png"))
                            .ephemeral(true),
                    ),
                )
                .await?;
        }
        ModeOwnership::Expired => {
            disable_components(ctx, component).await?;
        }
        ModeOwnership::Sender => match rerender_for_mode(data, cache_key, mode) {
            Some((png, components)) => {
                component.create_response(&ctx.http, v2_update(components, Some(png))).await?;
            }
            None => {
                disable_components(ctx, component).await?;
            }
        },
    }

    Ok(())
}


async fn handle_create_bookmark(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
    cache_key: &str,
) -> Result<()> {
    let discord_id = component.user.id.get() as i64;
    let timestamp = Utc::now();
    let name = timestamp.format("%b %-d, %Y").to_string();

    let (uuid, is_sender) = {
        let cache = data.session_images.lock().unwrap();
        let Some(entry) = cache.get(cache_key) else { return Ok(()) };
        (entry.uuid.clone(), entry.sender_id == component.user.id.get())
    };

    let is_owner = AccountRepository::new(data.db.pool())
        .is_owned_by(&uuid, discord_id)
        .await
        .unwrap_or(false);

    if !is_owner {
        component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("You can only create bookmarks for accounts linked to you.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    if let Err(e) = SessionRepository::new(data.db.pool())
        .create(&uuid, discord_id, &name, timestamp)
        .await
    {
        tracing::error!("Failed to create bookmark: {e}");
        return Ok(());
    }

    let snapshot_data = CacheRepository::new(data.db.pool())
        .get_snapshot_at(&uuid, timestamp)
        .await
        .ok()
        .flatten();

    let (components, png, ephemeral_png) = {
        let mut cache = data.session_images.lock().unwrap();
        let Some(entry) = cache.get_mut(cache_key) else { return Ok(()) };

        let to_stats = |v: Option<serde_json::Value>| -> Option<Stats> {
            extract_bedwars_stats(&entry.render_data.username, &v?, entry.render_data.guild_info.clone())
        };

        let key = format!("marker:{name}");
        let mut bookmark_png = None;

        if let Some(prev_stats) = to_stats(snapshot_data) {
            let session_type = SessionType::Custom(name.to_string());
            let image = render_session(
                &entry.render_data.current_stats, &prev_stats,
                session_type.clone(), timestamp, None, entry.mode,
                entry.render_data.skin.as_ref(), &entry.render_data.tag_icons,
            );
            if let Ok(png) = encode_png(&image) {
                bookmark_png = Some(png.clone());
                entry.images.insert(key.clone(), png);
            }
            entry.descriptions.insert(
                key.clone(),
                format_stats_delta(&entry.render_data.current_stats, &prev_stats, entry.mode),
            );
            entry.render_data.previous_stats.insert(key.clone(), (prev_stats, session_type, timestamp));
            if is_sender {
                entry.current_view = key;
            }
        }

        entry.markers.push(SessionMarker {
            id: 0,
            uuid: uuid.to_string(),
            discord_id,
            name: name.to_string(),
            snapshot_timestamp: timestamp,
            created_at: timestamp,
        });
        if is_sender {
            entry.last_interaction = Instant::now();
        }

        let png = entry.images.get(&entry.current_view).cloned();
        let components = build_session_components(
            cache_key, &entry.uuid, &entry.current_view, entry.mode,
            &entry.render_data.current_stats, &entry.descriptions,
            &entry.markers, &entry.auto_presets, entry.is_owner,
        );
        (components, png, bookmark_png)
    };

    if is_sender {
        component.create_response(&ctx.http, v2_update(components, png)).await?;
    } else {
        let mut msg = CreateInteractionResponseMessage::new()
            .content("Bookmark created!")
            .ephemeral(true);
        if let Some(png) = ephemeral_png {
            msg = msg.add_file(CreateAttachment::bytes(png, "session.png"));
        }
        component.create_response(&ctx.http, CreateInteractionResponse::Message(msg)).await?;
        update_original_components(ctx, component, components).await;
    }

    Ok(())
}


pub async fn handle_mgmt_rename_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let rest = component.data.custom_id.strip_prefix("session_mgmt_rename:").unwrap_or("");
    let parts: Vec<&str> = rest.splitn(3, ':').collect();
    if parts.len() < 3 { return Ok(()) }
    let (uuid, old_name) = (parts[1], parts[2]);

    let is_owner = AccountRepository::new(data.db.pool())
        .is_owned_by(uuid, component.user.id.get() as i64)
        .await
        .unwrap_or(false);

    if !is_owner {
        component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("You can only manage bookmarks for accounts linked to you.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    let input = CreateInputText::new(InputTextStyle::Short, "new_name")
        .placeholder("New session name")
        .min_length(1)
        .max_length(32);

    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Modal(
                CreateModal::new(format!("session_rename_modal:{rest}"), format!("Rename \"{old_name}\""))
                    .components(vec![CreateModalComponent::Label(
                        CreateLabel::input_text("New Name", input),
                    )]),
            ),
        )
        .await?;

    Ok(())
}


pub async fn handle_rename_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
) -> Result<()> {
    let rest = modal.data.custom_id.strip_prefix("session_rename_modal:").unwrap_or("");
    let parts: Vec<&str> = rest.splitn(3, ':').collect();
    if parts.len() < 3 { return Ok(()) }
    let (cache_key, uuid, old_name) = (parts[0], parts[1], parts[2]);

    let new_name = extract_modal_field(modal, "new_name").unwrap_or(old_name);
    let discord_id = modal.user.id.get() as i64;

    match SessionRepository::new(data.db.pool()).rename(uuid, discord_id, old_name, new_name).await {
        Ok(true) => {}
        Ok(false) => {
            send_ephemeral_modal(ctx, modal, "Session not found").await?;
            return Ok(());
        }
        Err(e) => {
            tracing::error!("Failed to rename session: {e}");
            send_ephemeral_modal(ctx, modal, "Failed to rename session").await?;
            return Ok(());
        }
    }

    let (is_sender, cached_uuid) = {
        let cache = data.session_images.lock().unwrap();
        let Some(entry) = cache.get(cache_key) else { return Ok(()) };
        (entry.sender_id == modal.user.id.get(), entry.uuid.clone())
    };

    let fresh_markers = SessionRepository::new(data.db.pool())
        .list(&cached_uuid, discord_id)
        .await
        .unwrap_or_default();

    let (components, png) = {
        let mut cache = data.session_images.lock().unwrap();
        let Some(entry) = cache.get_mut(cache_key) else { return Ok(()) };

        let old_key = format!("marker:{old_name}");
        let new_key = format!("marker:{new_name}");

        if let Some(img) = entry.images.remove(&old_key) {
            entry.images.insert(new_key.clone(), img);
        }
        if let Some(desc) = entry.descriptions.remove(&old_key) {
            entry.descriptions.insert(new_key.clone(), desc);
        }
        if let Some(ps) = entry.render_data.previous_stats.remove(&old_key) {
            entry.render_data.previous_stats.insert(
                new_key.clone(),
                (ps.0, SessionType::Custom(new_name.to_string()), ps.2),
            );
        }
        if is_sender && entry.current_view == old_key {
            entry.current_view = new_key;
        }
        entry.markers = fresh_markers;
        if is_sender {
            entry.last_interaction = Instant::now();
        }

        let png = entry.images.get(&entry.current_view).cloned();
        let components = build_session_components(
            cache_key, &entry.uuid, &entry.current_view, entry.mode,
            &entry.render_data.current_stats, &entry.descriptions,
            &entry.markers, &entry.auto_presets, entry.is_owner,
        );
        (components, png)
    };

    if is_sender {
        modal.create_response(&ctx.http, v2_update(components, png)).await?;
    } else {
        send_ephemeral_modal(ctx, modal, "Bookmark renamed.").await?;
    }

    Ok(())
}


pub async fn handle_mgmt_delete_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let custom_id = component.data.custom_id.strip_prefix("session_mgmt_delete:").unwrap_or("");
    let parts: Vec<&str> = custom_id.splitn(3, ':').collect();
    if parts.len() < 3 { return Ok(()) }
    let (cache_key, uuid, name) = (parts[0], parts[1], parts[2]);
    let discord_id = component.user.id.get() as i64;

    let is_owner = AccountRepository::new(data.db.pool())
        .is_owned_by(uuid, discord_id)
        .await
        .unwrap_or(false);

    if !is_owner {
        component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("You can only manage bookmarks for accounts linked to you.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    match SessionRepository::new(data.db.pool()).delete(uuid, discord_id, name).await {
        Ok(false) | Err(_) => return Ok(()),
        Ok(true) => {}
    }

    let fresh_markers = SessionRepository::new(data.db.pool())
        .list(uuid, discord_id)
        .await
        .unwrap_or_default();

    let (is_sender, components, png) = {
        let mut cache = data.session_images.lock().unwrap();
        let Some(entry) = cache.get_mut(cache_key) else { return Ok(()) };

        let is_sender = entry.sender_id == component.user.id.get();
        let deleted_key = format!("marker:{name}");
        entry.images.remove(&deleted_key);
        entry.descriptions.remove(&deleted_key);
        entry.render_data.previous_stats.remove(&deleted_key);

        if is_sender && entry.current_view == deleted_key {
            entry.current_view = PERIODS
                .iter()
                .map(|p| p.key().to_string())
                .find(|k| entry.images.contains_key(k))
                .unwrap_or_else(|| "daily".to_string());
        }
        entry.markers = fresh_markers;
        if is_sender {
            entry.last_interaction = Instant::now();
        }

        let png = entry.images.get(&entry.current_view).cloned();
        let components = build_session_components(
            cache_key, &entry.uuid, &entry.current_view, entry.mode,
            &entry.render_data.current_stats, &entry.descriptions,
            &entry.markers, &entry.auto_presets, entry.is_owner,
        );
        (is_sender, components, png)
    };

    if is_sender {
        component.create_response(&ctx.http, v2_update(components, png)).await?;
    } else {
        component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("Bookmark deleted.")
                        .ephemeral(true),
                ),
            )
            .await?;
        update_original_components(ctx, component, components).await;
    }

    Ok(())
}


fn resolve_view_switch(
    data: &Data,
    cache_key: &str,
    image_key: &str,
    user_id: u64,
) -> SwitchResult {
    let mut cache = data.session_images.lock().unwrap();
    let Some(entry) = cache.get_mut(cache_key) else { return SwitchResult::Expired };

    if entry.last_interaction.elapsed().as_secs() > CACHE_TTL_SECS {
        cache.remove(cache_key);
        return SwitchResult::Expired;
    }
    if entry.sender_id != user_id {
        return match entry.images.get(image_key).cloned() {
            Some(png) => SwitchResult::Ephemeral(png),
            None => SwitchResult::Expired,
        };
    }

    let Some(png) = entry.images.get(image_key).cloned() else { return SwitchResult::Expired };
    entry.current_view = image_key.to_string();
    entry.last_interaction = Instant::now();

    let components = build_session_components(
        cache_key, &entry.uuid, image_key, entry.mode,
        &entry.render_data.current_stats, &entry.descriptions,
        &entry.markers, &entry.auto_presets, entry.is_owner,
    );
    SwitchResult::Ok(png, components)
}


fn check_mode_sender(data: &Data, cache_key: &str, mode: Mode, user_id: u64) -> ModeOwnership {
    let store = data.session_images.lock().unwrap();
    let Some(entry) = store.get(cache_key) else { return ModeOwnership::Expired };

    if entry.last_interaction.elapsed().as_secs() > CACHE_TTL_SECS {
        return ModeOwnership::Expired;
    }
    if entry.sender_id != user_id {
        return entry
            .render_data
            .previous_stats
            .get(&entry.current_view)
            .and_then(|(prev_stats, session_type, started)| {
                let image = render_session(
                    &entry.render_data.current_stats, prev_stats,
                    session_type.clone(), *started, None, mode,
                    entry.render_data.skin.as_ref(), &entry.render_data.tag_icons,
                );
                encode_png(&image).ok().map(ModeOwnership::Ephemeral)
            })
            .unwrap_or(ModeOwnership::Expired);
    }

    ModeOwnership::Sender
}


fn rerender_for_mode(
    data: &Data,
    cache_key: &str,
    mode: Mode,
) -> Option<(Vec<u8>, Vec<CreateComponent<'static>>)> {
    let mut store = data.session_images.lock().unwrap();
    let entry = store.get_mut(cache_key)?;

    if entry.last_interaction.elapsed().as_secs() > CACHE_TTL_SECS {
        store.remove(cache_key);
        return None;
    }

    entry.mode = mode;
    entry.last_interaction = Instant::now();
    rerender_all_views(entry);
    update_descriptions(entry, mode);

    let png = entry.images.get(&entry.current_view)?.clone();
    let components = build_session_components(
        cache_key, &entry.uuid, &entry.current_view, mode,
        &entry.render_data.current_stats, &entry.descriptions,
        &entry.markers, &entry.auto_presets, entry.is_owner,
    );
    Some((png, components))
}


fn rerender_all_views(entry: &mut SessionCache) {
    let skin = entry.render_data.skin.as_ref();
    let keys: Vec<String> = entry.render_data.previous_stats.keys().cloned().collect();

    for key in keys {
        if let Some((prev, session_type, started)) = entry.render_data.previous_stats.get(&key) {
            let image = render_session(
                &entry.render_data.current_stats, prev, session_type.clone(),
                *started, None, entry.mode, skin, &entry.render_data.tag_icons,
            );
            if let Ok(png) = encode_png(&image) {
                entry.images.insert(key, png);
            }
        }
    }
}


fn update_descriptions(entry: &mut SessionCache, mode: Mode) {
    let keys: Vec<String> = entry.render_data.previous_stats.keys().cloned().collect();
    for key in keys {
        if let Some((prev, _, _)) = entry.render_data.previous_stats.get(&key) {
            entry.descriptions.insert(key, format_stats_delta(&entry.render_data.current_stats, prev, mode));
        }
    }
}


async fn precompute_session(
    data: &Data,
    player: &str,
    discord_id: i64,
    mode: Mode,
) -> Result<SessionCache, SessionError> {
    let t = Instant::now();
    let cached_uuid = resolve_uuid(data, player).await;
    debug!(at = ?t.elapsed(), cached = cached_uuid.is_some(), "session resolve");

    let (resp, guild_result, skin_result) =
        fetch_player(data, player, cached_uuid.as_deref()).await?;
    debug!(at = ?t.elapsed(), "session api done");

    let hypixel_data = resp.hypixel.ok_or(SessionError::PlayerNotFound)?;
    let username = resp.username.clone();
    let uuid = resp.uuid.clone();

    let guild_info = guild_result.ok().flatten().map(|g| super::to_guild_info(&g));
    let skin_image = skin_result.map(|s| s.data);
    let current_stats = extract_bedwars_stats(&username, &hypixel_data, guild_info.clone())
        .ok_or_else(|| SessionError::NoStats(username.clone()))?;

    let (snapshots, markers, auto_presets) =
        fetch_snapshots(data, &uuid, discord_id, current_stats.level as u64).await;
    debug!(at = ?t.elapsed(), "session snapshots done");

    let to_stats = |v: Option<serde_json::Value>| -> Option<Stats> {
        extract_bedwars_stats(&username, &v?, guild_info.clone())
    };

    let (images, previous_stats, descriptions, marker_list) = render_all_views(
        &current_stats, snapshots, &markers, &auto_presets,
        mode, skin_image.as_ref(), &extract_tag_icons(&resp.tags), to_stats,
    );
    debug!(at = ?t.elapsed(), "session render done");

    Ok(SessionCache {
        uuid,
        sender_id: discord_id as u64,
        is_owner: false,
        images,
        descriptions,
        markers: marker_list,
        auto_presets,
        mode,
        current_view: "daily".to_string(),
        render_data: SessionRenderData {
            current_stats,
            previous_stats,
            skin: skin_image,
            tag_icons: extract_tag_icons(&resp.tags),
            username: username.clone(),
            guild_info: guild_info.clone(),
        },
        last_interaction: Instant::now(),
    })
}


async fn fetch_player(
    data: &Data,
    player: &str,
    cached_uuid: Option<&str>,
) -> Result<
    (
        crate::api::PlayerStatsResponse,
        Result<Option<crate::api::GuildResponse>, crate::api::ApiError>,
        Option<clients::SkinImage>,
    ),
    SessionError,
> {
    match cached_uuid {
        Some(uuid) => {
            let (api, guild, skin) = tokio::join!(
                data.api.get_player_stats(player),
                data.api.get_guild(uuid, Some("player")),
                data.skin_provider.fetch(uuid),
            );
            let resp = api.map_err(map_api_error)?;
            if resp.uuid == uuid {
                return Ok((resp, guild, skin));
            }
            let (guild, skin) = tokio::join!(
                data.api.get_guild(&resp.uuid, Some("player")),
                fetch_skin(data, &resp.uuid, resp.skin_url.as_deref()),
            );
            Ok((resp, guild, skin))
        }
        None => {
            let resp = data.api.get_player_stats(player).await.map_err(map_api_error)?;
            let (guild, skin) = tokio::join!(
                data.api.get_guild(&resp.uuid, Some("player")),
                fetch_skin(data, &resp.uuid, resp.skin_url.as_deref()),
            );
            Ok((resp, guild, skin))
        }
    }
}


fn map_api_error(e: crate::api::ApiError) -> SessionError {
    match e {
        crate::api::ApiError::NotFound => SessionError::PlayerNotFound,
        other => {
            tracing::error!("Internal API error: {other}");
            SessionError::ApiError
        }
    }
}


async fn fetch_snapshots(
    data: &Data,
    uuid: &str,
    discord_id: i64,
    current_level: u64,
) -> (
    Vec<Option<(DateTime<Utc>, serde_json::Value)>>,
    Vec<SessionMarker>,
    Vec<AutoPreset>,
) {
    let session_repo = SessionRepository::new(data.db.pool());
    let cache_repo = CacheRepository::new(data.db.pool());
    let now = Utc::now();

    let (mut markers, auto_presets) = tokio::join!(
        async { session_repo.list(uuid, discord_id).await.unwrap_or_default() },
        detect_auto_presets(&cache_repo, uuid, current_level),
    );

    if markers.is_empty() {
        if let Ok(marker) = session_repo.create(uuid, discord_id, "main", now).await {
            markers.push(marker);
        }
    }

    let mut timestamps: Vec<DateTime<Utc>> = PERIODS.iter().map(|p| p.last_reset(now)).collect();
    for period in PERIODS {
        if period.fixed_preset().is_some() {
            timestamps.push(now - period.duration());
        }
    }
    for marker in &markers {
        timestamps.push(marker.snapshot_timestamp);
    }
    for preset in &auto_presets {
        timestamps.push(preset.timestamp);
    }

    let snapshots = cache_repo
        .get_snapshots_at_times(uuid, &timestamps)
        .await
        .unwrap_or_else(|_| vec![None; timestamps.len()]);

    (snapshots, markers, auto_presets)
}


fn render_all_views(
    current_stats: &Stats,
    snapshots: Vec<Option<(DateTime<Utc>, serde_json::Value)>>,
    markers: &[SessionMarker],
    auto_presets: &[AutoPreset],
    mode: Mode,
    skin: Option<&DynamicImage>,
    tag_icons: &[TagIcon],
    to_stats: impl Fn(Option<serde_json::Value>) -> Option<Stats>,
) -> (
    HashMap<String, Vec<u8>>,
    HashMap<String, (Stats, SessionType, DateTime<Utc>)>,
    HashMap<String, String>,
    Vec<SessionMarker>,
) {
    let now = Utc::now();
    let mut images = HashMap::new();
    let mut previous_stats = HashMap::new();
    let mut descriptions = HashMap::new();
    let mut snapshot_iter = snapshots.into_iter();

    let mut render_view = |key: String, prev_stats: &Stats, session_type: SessionType, timestamp: DateTime<Utc>| {
        descriptions.insert(key.clone(), format_stats_delta(current_stats, prev_stats, mode));
        let image = render_session(
            current_stats, prev_stats, session_type.clone(),
            timestamp, None, mode, skin, tag_icons,
        );
        if let Ok(png) = encode_png(&image) {
            images.insert(key.clone(), png);
        }
        previous_stats.insert(key, (prev_stats.clone(), session_type, timestamp));
    };

    for period in PERIODS {
        let target_time = period.last_reset(now);
        let snapshot = snapshot_iter.next().flatten();
        let cutoff = target_time - period.staleness();
        let in_range = snapshot.as_ref().is_some_and(|(ts, _)| *ts >= cutoff);

        if let Some(prev) = to_stats(snapshot.map(|(_, v)| v)).filter(|_| in_range) {
            render_view(period.key().to_string(), &prev, period_session_type(period), target_time);
        }
    }

    for period in PERIODS {
        let Some((fp_key, fp_label)) = period.fixed_preset() else { continue };
        let target_time = now - period.duration();
        let snapshot = snapshot_iter.next().flatten();

        if let Some(prev) = to_stats(snapshot.map(|(_, v)| v)) {
            render_view(fp_key.to_string(), &prev, SessionType::Custom(fp_label.to_string()), target_time);
        }
    }

    let marker_list: Vec<SessionMarker> = markers.to_vec();
    for marker in markers {
        if let Some(prev) = to_stats(snapshot_iter.next().flatten().map(|(_, v)| v)) {
            render_view(
                format!("marker:{}", marker.name), &prev,
                SessionType::Custom(marker.name.clone()), marker.snapshot_timestamp,
            );
        }
    }

    for preset in auto_presets {
        if let Some(prev) = to_stats(snapshot_iter.next().flatten().map(|(_, v)| v)) {
            render_view(
                format!("preset:{}", preset.key), &prev,
                SessionType::Custom(preset.label.clone()), preset.timestamp,
            );
        }
    }

    (images, previous_stats, descriptions, marker_list)
}


struct SnapshotFields {
    experience: u64,
    losses: u64,
}


async fn detect_auto_presets(
    cache_repo: &CacheRepository<'_>,
    uuid: &str,
    current_level: u64,
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
        });
    }

    presets
}
