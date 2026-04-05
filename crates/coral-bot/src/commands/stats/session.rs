use std::collections::HashMap;
use std::time::Instant;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serenity::all::*;

use database::{
    AccountRepository, CacheRepository, MemberRepository, SessionMarker, SessionRepository,
};
use render::SessionType;

use crate::framework::Data;
use super::{
    AutoPreset, CACHE_TTL_SECS, GameStats, PERIODS, SessionCacheEntry, SessionRenderData, StatsError,
    create_session_dropdown, disable_components, evict_expired, extract_modal_field,
    extract_tag_icons, fetch_skin, image_gallery, map_api_error,
    period_session_type, player_option, resolve_uuid, send_deferred_error,
    send_ephemeral_modal, spawn_expiry_with_retain, update_original_components, v2_update,
};


enum SwitchResult {
    Ok(Vec<u8>, Vec<CreateComponent<'static>>),
    Expired,
    Ephemeral(Vec<u8>),
}


pub fn register() -> CreateCommand<'static> {
    CreateCommand::new("session")
        .description("View your session stats over time")
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "bedwars", "View BedWars session stats")
                .add_sub_option(player_option()),
        )
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "duels", "View Duels session stats")
                .add_sub_option(player_option()),
        )
}


fn game_subcommands(name: &'static str) -> CreateCommand<'static> {
    CreateCommand::new(name)
        .description(format!("View your {name} session stats"))
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "bedwars", "View BedWars session stats")
                .add_sub_option(player_option()),
        )
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "duels", "View Duels session stats")
                .add_sub_option(player_option()),
        )
}


pub fn register_daily() -> CreateCommand<'static> { game_subcommands("daily") }

pub fn register_weekly() -> CreateCommand<'static> { game_subcommands("weekly") }

pub fn register_monthly() -> CreateCommand<'static> { game_subcommands("monthly") }


pub async fn run(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    route(ctx, command, data, None).await
}


pub async fn run_daily(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    route(ctx, command, data, Some("daily")).await
}


pub async fn run_weekly(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    route(ctx, command, data, Some("weekly")).await
}


pub async fn run_monthly(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    route(ctx, command, data, Some("monthly")).await
}


async fn route(ctx: &Context, command: &CommandInteraction, data: &Data, preferred: Option<&str>) -> Result<()> {
    match command.data.options.first().map(|o| o.name.as_str()) {
        Some("duels") => super::duels::session_run(ctx, command, data, preferred).await,
        _ => super::bedwars::session_run(ctx, command, data, preferred).await,
    }
}


pub(super) async fn run_with_preferred_view<G: GameStats>(
    ctx: &Context,
    command: &CommandInteraction,
    data: &Data,
    preferred: Option<&str>,
) -> Result<()> {
    let player_input = command.data.options.first().and_then(|o| match &o.value {
        CommandDataOptionValue::SubCommand(sub) => sub.first().and_then(|s| s.value.as_str()).map(|s| s.to_string()),
        _ => o.value.as_str().map(|s| s.to_string()),
    });
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
        precompute_session::<G>(data, &player, discord_id),
    );
    defer_result?;

    match result {
        Ok(session_cache) => {
            let latest_marker = if preferred.is_none() {
                session_cache.markers.last().map(|m| format!("marker:{}", m.name))
                    .filter(|key| session_cache.render_data.previous_stats.contains_key(key))
            } else {
                None
            };
            let preferred_missing = preferred
                .filter(|key| !session_cache.render_data.previous_stats.contains_key(*key));
            let initial_period = preferred
                .filter(|key| session_cache.render_data.previous_stats.contains_key(*key))
                .or(latest_marker.as_deref())
                .or_else(|| {
                    PERIODS.iter()
                        .map(|p| p.key())
                        .find(|key| session_cache.render_data.previous_stats.contains_key(*key))
                })
                .unwrap_or("daily");
            let initial_mode = G::default_mode(&session_cache.render_data.current_stats);
            let initial_png = render_selected_png::<G>(&session_cache, initial_period, &initial_mode);
            let uuid = session_cache.uuid.clone();

            let is_owner = AccountRepository::new(data.db.pool())
                .is_owned_by(&uuid, discord_id)
                .await
                .unwrap_or(false);

            let mut components = build_session_components::<G>(
                &cache_key, &uuid, initial_period, &initial_mode,
                &session_cache.render_data.current_stats, &session_cache.descriptions,
                &session_cache.markers, &session_cache.auto_presets, is_owner,
            );
            let expiry_key = cache_key.clone();

            if let Some(period_key) = preferred_missing {
                let period_label = database::Period::from_str(period_key)
                    .map(|p| p.label().to_lowercase())
                    .unwrap_or_else(|| period_key.to_string());
                let showing = database::Period::from_str(initial_period)
                    .map(|p| p.label().to_lowercase())
                    .unwrap_or_else(|| initial_period.to_string());
                components.push(CreateComponent::TextDisplay(
                    CreateTextDisplay::new(format!("-# No data for {period_label}, showing {showing} session")),
                ));
            }

            {
                let mut cache = G::session_cache(data).lock().unwrap();
                evict::<G>(&mut cache);
                let mut session_cache = session_cache;
                session_cache.current_period = initial_period.to_string();
                session_cache.current_mode = initial_mode;
                session_cache.is_owner = is_owner;
                cache.insert(cache_key, session_cache);
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
                    ctx.http.clone(),
                    command.token.to_string(),
                    G::session_cache(data).clone(),
                    expiry_key,
                    |entry: &SessionCacheEntry<G>| entry.last_interaction,
                    vec![image_gallery()],
                );
            } else {
                send_deferred_error(ctx, command, "No Historical Data", "No snapshot data available yet. Check back later!").await?;
            }
        }
        Err(StatsError::PlayerNotFound) => {
            send_deferred_error(ctx, command, "Player Not Found", &format!("Could not find player: {player}")).await?;
        }
        Err(StatsError::NoStats(username)) => {
            send_deferred_error(ctx, command, &format!("{username}'s Session Stats"), &format!("This player has no {} stats", G::GAME_NAME)).await?;
        }
        Err(StatsError::ApiError) => {
            send_deferred_error(ctx, command, "Error", "Something went wrong. Please try again later.").await?;
        }
    }

    Ok(())
}


pub(super) async fn handle_switch<G: GameStats>(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let Some(value) = super::extract_select_value(component) else { return Ok(()) };

    let parts: Vec<&str> = value.splitn(3, ':').collect();
    if parts.len() < 2 { return Ok(()) }
    let (selection, cache_key, extra) = (parts[0], parts[1], parts.get(2).copied());

    if selection == "create" {
        return handle_create_bookmark::<G>(ctx, component, data, cache_key).await;
    }

    let period_key = match selection {
        "marker" => format!("marker:{}", extra.unwrap_or("")),
        "preset" => format!("preset:{}", extra.unwrap_or("")),
        _ => selection.to_string(),
    };

    match resolve_period_switch::<G>(data, cache_key, &period_key, component.user.id.get()) {
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


pub(super) async fn handle_mode_switch<G: GameStats>(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let Some((cache_key, mode)) = G::parse_mode_interaction(component) else { return Ok(()) };

    match resolve_mode_switch::<G>(data, &cache_key, mode, component.user.id.get()) {
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


async fn handle_create_bookmark<G: GameStats>(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
    cache_key: &str,
) -> Result<()> {
    let discord_id = component.user.id.get() as i64;
    let timestamp = Utc::now();
    let name = timestamp.format("%b %-d, %Y").to_string();

    let (uuid, is_sender) = {
        let cache = G::session_cache(data).lock().unwrap();
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

    if let Err(error) = SessionRepository::new(data.db.pool())
        .create(&uuid, discord_id, &name, timestamp)
        .await
    {
        tracing::error!("Failed to create bookmark: {error}");
        return Ok(());
    }

    let snapshot_data = CacheRepository::new(data.db.pool())
        .get_snapshot_at(&uuid, timestamp)
        .await
        .ok()
        .flatten();

    let (components, png, ephemeral_png) = {
        let mut cache = G::session_cache(data).lock().unwrap();
        let Some(entry) = cache.get_mut(cache_key) else { return Ok(()) };

        let to_stats = |value: Option<serde_json::Value>| -> Option<G::Stats> {
            G::extract_stats(&entry.render_data.username, &value?, entry.render_data.guild_info.clone())
        };

        let key = format!("marker:{name}");
        let mut bookmark_png = None;

        if let Some(previous_stats) = to_stats(snapshot_data) {
            let session_type = SessionType::Custom(name.to_string());
            entry.descriptions.insert(
                key.clone(),
                G::format_delta(&entry.render_data.current_stats, &previous_stats, &entry.current_mode),
            );
            entry.render_data.previous_stats.insert(
                key.clone(),
                (previous_stats, session_type, timestamp),
            );
            if is_sender {
                entry.current_period = key.clone();
            }
            bookmark_png = render_selected_png::<G>(entry, &key, &entry.current_mode.clone());
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

        let png = render_selected_png::<G>(entry, &entry.current_period.clone(), &entry.current_mode.clone());
        let components = build_session_components::<G>(
            cache_key, &entry.uuid, &entry.current_period, &entry.current_mode,
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


pub(super) async fn handle_mgmt_rename_button<G: GameStats>(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let rest = component.data.custom_id.strip_prefix(G::MGMT_RENAME_PREFIX).unwrap_or("");
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
                CreateModal::new(
                    format!("{}{rest}", G::RENAME_MODAL_PREFIX),
                    format!("Rename \"{old_name}\""),
                )
                .components(vec![CreateModalComponent::Label(
                    CreateLabel::input_text("New Name", input),
                )]),
            ),
        )
        .await?;

    Ok(())
}


pub(super) async fn handle_rename_modal<G: GameStats>(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
) -> Result<()> {
    let rest = modal.data.custom_id.strip_prefix(G::RENAME_MODAL_PREFIX).unwrap_or("");
    let parts: Vec<&str> = rest.splitn(3, ':').collect();
    if parts.len() < 3 { return Ok(()) }
    let (cache_key, uuid, old_name) = (parts[0], parts[1], parts[2]);

    let new_name = extract_modal_field(modal, "new_name").unwrap_or(old_name);
    let discord_id = modal.user.id.get() as i64;

    match SessionRepository::new(data.db.pool())
        .rename(uuid, discord_id, old_name, new_name)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            send_ephemeral_modal(ctx, modal, "Session not found").await?;
            return Ok(());
        }
        Err(error) => {
            tracing::error!("Failed to rename session: {error}");
            send_ephemeral_modal(ctx, modal, "Failed to rename session").await?;
            return Ok(());
        }
    }

    let (is_sender, cached_uuid) = {
        let cache = G::session_cache(data).lock().unwrap();
        let Some(entry) = cache.get(cache_key) else { return Ok(()) };
        (entry.sender_id == modal.user.id.get(), entry.uuid.clone())
    };

    let fresh_markers = SessionRepository::new(data.db.pool())
        .list(&cached_uuid, discord_id)
        .await
        .unwrap_or_default();

    let (components, png) = {
        let mut cache = G::session_cache(data).lock().unwrap();
        let Some(entry) = cache.get_mut(cache_key) else { return Ok(()) };

        let old_key = format!("marker:{old_name}");
        let new_key = format!("marker:{new_name}");

        if let Some(desc) = entry.descriptions.remove(&old_key) {
            entry.descriptions.insert(new_key.clone(), desc);
        }
        if let Some(previous) = entry.render_data.previous_stats.remove(&old_key) {
            entry.render_data.previous_stats.insert(
                new_key.clone(),
                (previous.0, SessionType::Custom(new_name.to_string()), previous.2),
            );
        }
        if is_sender && entry.current_period == old_key {
            entry.current_period = new_key;
        }
        entry.markers = fresh_markers;
        if is_sender {
            entry.last_interaction = Instant::now();
        }

        let png = render_selected_png::<G>(entry, &entry.current_period.clone(), &entry.current_mode.clone());
        let components = build_session_components::<G>(
            cache_key, &entry.uuid, &entry.current_period, &entry.current_mode,
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


pub(super) async fn handle_mgmt_delete_button<G: GameStats>(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let custom_id = component.data.custom_id.strip_prefix(G::MGMT_DELETE_PREFIX).unwrap_or("");
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

    let (components, png) = {
        let cache = G::session_cache(data).lock().unwrap();
        let Some(entry) = cache.get(cache_key) else { return Ok(()) };
        let png = render_selected_png::<G>(entry, &entry.current_period.clone(), &entry.current_mode.clone());
        (build_confirm_delete_components::<G>(cache_key, uuid, name, entry), png)
    };

    component.create_response(&ctx.http, v2_update(components, png)).await?;

    Ok(())
}


pub(super) async fn handle_confirm_delete_button<G: GameStats>(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let custom_id = component.data.custom_id.strip_prefix(G::CONFIRM_DELETE_PREFIX).unwrap_or("");
    let parts: Vec<&str> = custom_id.splitn(3, ':').collect();
    if parts.len() < 3 { return Ok(()) }
    let (cache_key, uuid, name) = (parts[0], parts[1], parts[2]);
    let discord_id = component.user.id.get() as i64;

    match SessionRepository::new(data.db.pool())
        .delete(uuid, discord_id, name)
        .await
    {
        Ok(false) | Err(_) => return Ok(()),
        Ok(true) => {}
    }

    let fresh_markers = SessionRepository::new(data.db.pool())
        .list(uuid, discord_id)
        .await
        .unwrap_or_default();

    let result = {
        let mut cache = G::session_cache(data).lock().unwrap();
        let Some(entry) = cache.get_mut(cache_key) else {
            return Ok(());
        };

        let deleted_key = format!("marker:{name}");
        entry.descriptions.remove(&deleted_key);
        entry.render_data.previous_stats.remove(&deleted_key);

        if entry.current_period == deleted_key {
            entry.current_period = PERIODS
                .iter()
                .map(|p| p.key().to_string())
                .find(|key| entry.render_data.previous_stats.contains_key(key))
                .unwrap_or_else(|| "daily".to_string());
        }
        entry.markers = fresh_markers;
        entry.last_interaction = Instant::now();

        let png = render_selected_png::<G>(entry, &entry.current_period.clone(), &entry.current_mode.clone());
        let components = build_session_components::<G>(
            cache_key, &entry.uuid, &entry.current_period, &entry.current_mode,
            &entry.render_data.current_stats, &entry.descriptions,
            &entry.markers, &entry.auto_presets, entry.is_owner,
        );
        (components, png)
    };

    component.create_response(&ctx.http, v2_update(result.0, result.1)).await?;

    Ok(())
}


fn render_selected_png<G: GameStats>(
    entry: &SessionCacheEntry<G>,
    period_key: &str,
    mode: &G::ModeSelection,
) -> Option<Vec<u8>> {
    let (previous, session_type, timestamp) = entry.render_data.previous_stats.get(period_key)?;
    G::render_session(
        &entry.render_data.current_stats,
        previous,
        session_type.clone(),
        *timestamp,
        mode,
        entry.render_data.skin.as_ref(),
        &entry.render_data.snapshots,
        &entry.render_data.tag_icons,
    )
    .ok()
}


fn build_session_components<G: GameStats>(
    cache_key: &str,
    uuid: &str,
    current_period: &str,
    current_mode: &G::ModeSelection,
    current_stats: &G::Stats,
    descriptions: &HashMap<String, String>,
    markers: &[SessionMarker],
    auto_presets: &[AutoPreset],
    is_owner: bool,
) -> Vec<CreateComponent<'static>> {
    let mut container_rows = vec![
        CreateContainerComponent::ActionRow(CreateActionRow::SelectMenu(
            create_session_dropdown::<G>(
                cache_key, current_period, descriptions, markers, auto_presets, is_owner,
            ),
        )),
        CreateContainerComponent::ActionRow(CreateActionRow::SelectMenu(
            G::create_mode_dropdown(G::SESSION_MODE_ID, cache_key, current_mode, current_stats),
        )),
    ];

    if let Some(marker_name) = current_period.strip_prefix("marker:") {
        if is_owner {
            container_rows.push(CreateContainerComponent::ActionRow(
                CreateActionRow::Buttons(
                    vec![
                        CreateButton::new(format!("{}{cache_key}:{uuid}:{marker_name}", G::MGMT_RENAME_PREFIX))
                            .label("Rename")
                            .style(ButtonStyle::Primary),
                        CreateButton::new(format!("{}{cache_key}:{uuid}:{marker_name}", G::MGMT_DELETE_PREFIX))
                            .label("Delete")
                            .style(ButtonStyle::Danger),
                    ]
                    .into(),
                ),
            ));
        }
    }

    vec![CreateComponent::Container(CreateContainer::new(container_rows))]
}


fn build_confirm_delete_components<G: GameStats>(
    cache_key: &str,
    uuid: &str,
    marker_name: &str,
    entry: &SessionCacheEntry<G>,
) -> Vec<CreateComponent<'static>> {
    let container_rows = vec![
        CreateContainerComponent::ActionRow(CreateActionRow::SelectMenu(
            create_session_dropdown::<G>(
                cache_key, &entry.current_period, &entry.descriptions,
                &entry.markers, &entry.auto_presets, entry.is_owner,
            ),
        )),
        CreateContainerComponent::ActionRow(CreateActionRow::SelectMenu(
            G::create_mode_dropdown(G::SESSION_MODE_ID, cache_key, &entry.current_mode, &entry.render_data.current_stats),
        )),
        CreateContainerComponent::ActionRow(CreateActionRow::Buttons(
            vec![
                CreateButton::new(format!("{}{cache_key}:{uuid}:{marker_name}", G::CONFIRM_DELETE_PREFIX))
                    .label(format!("Confirm Delete \"{}\"", marker_name))
                    .style(ButtonStyle::Danger),
            ]
            .into(),
        )),
    ];

    vec![CreateComponent::Container(CreateContainer::new(container_rows))]
}


fn resolve_period_switch<G: GameStats>(
    data: &Data,
    cache_key: &str,
    period_key: &str,
    user_id: u64,
) -> SwitchResult {
    let mut cache = G::session_cache(data).lock().unwrap();
    let Some(entry) = cache.get_mut(cache_key) else {
        return SwitchResult::Expired;
    };

    if entry.last_interaction.elapsed().as_secs() > CACHE_TTL_SECS {
        cache.remove(cache_key);
        return SwitchResult::Expired;
    }

    if entry.sender_id != user_id {
        let is_restricted = period_key.strip_prefix("preset:").is_some_and(|pk|
            entry.auto_presets.iter().any(|p| p.key == pk && p.restricted)
        );
        if is_restricted {
            return SwitchResult::Expired;
        }
        return match render_selected_png::<G>(entry, period_key, &entry.current_mode.clone()) {
            Some(png) => SwitchResult::Ephemeral(png),
            None => SwitchResult::Expired,
        };
    }

    let mode = entry.current_mode.clone();
    let png = match render_selected_png::<G>(entry, period_key, &mode) {
        Some(png) => png,
        None => return SwitchResult::Expired,
    };
    entry.current_period = period_key.to_string();
    entry.last_interaction = Instant::now();

    let components = build_session_components::<G>(
        cache_key, &entry.uuid, &entry.current_period, &entry.current_mode,
        &entry.render_data.current_stats, &entry.descriptions,
        &entry.markers, &entry.auto_presets, entry.is_owner,
    );
    SwitchResult::Ok(png, components)
}


fn resolve_mode_switch<G: GameStats>(
    data: &Data,
    cache_key: &str,
    mode: G::ModeSelection,
    user_id: u64,
) -> SwitchResult {
    let mut cache = G::session_cache(data).lock().unwrap();
    let Some(entry) = cache.get_mut(cache_key) else {
        return SwitchResult::Expired;
    };

    if entry.last_interaction.elapsed().as_secs() > CACHE_TTL_SECS {
        cache.remove(cache_key);
        return SwitchResult::Expired;
    }

    if entry.sender_id != user_id {
        return match render_selected_png::<G>(entry, &entry.current_period.clone(), &mode) {
            Some(png) => SwitchResult::Ephemeral(png),
            None => SwitchResult::Expired,
        };
    }

    let period = entry.current_period.clone();
    let png = match render_selected_png::<G>(entry, &period, &mode) {
        Some(png) => png,
        None => return SwitchResult::Expired,
    };
    entry.current_mode = mode;
    entry.last_interaction = Instant::now();

    let components = build_session_components::<G>(
        cache_key, &entry.uuid, &entry.current_period, &entry.current_mode,
        &entry.render_data.current_stats, &entry.descriptions,
        &entry.markers, &entry.auto_presets, entry.is_owner,
    );
    SwitchResult::Ok(png, components)
}


async fn precompute_session<G: GameStats>(
    data: &Data,
    player: &str,
    discord_id: i64,
) -> Result<SessionCacheEntry<G>, StatsError> {
    let cached_uuid = resolve_uuid(data, player).await;
    let (resp, guild_result, skin_result) =
        fetch_player(data, player, cached_uuid.as_deref()).await?;

    let hypixel_data = resp.hypixel.ok_or(StatsError::PlayerNotFound)?;
    let username = resp.username.clone();
    let uuid = resp.uuid.clone();

    let guild_info = guild_result.ok().flatten().map(|guild| super::to_guild_info(&guild));
    let skin_image = skin_result.map(|skin| skin.data);
    let current_stats = G::extract_stats(&username, &hypixel_data, guild_info.clone())
        .ok_or_else(|| StatsError::NoStats(username.clone()))?;

    let cache_repo = CacheRepository::new(data.db.pool());
    let (session_snapshots, markers, auto_presets, ws_snapshots) = {
        let (s, m, a) = fetch_snapshots::<G>(data, &uuid, discord_id, &current_stats).await;
        let ws = cache_repo
            .get_all_snapshots_mapped(&uuid, G::extract_winstreak_snapshot)
            .await;
        (s, m, a, ws)
    };

    let tags = extract_tag_icons(&resp.tags);
    let default_mode = G::default_mode(&current_stats);
    let (previous_stats, descriptions, marker_list) = build_previous_views::<G>(
        &current_stats, session_snapshots, &markers, &auto_presets,
        &username, guild_info.clone(), &default_mode,
    );

    Ok(SessionCacheEntry {
        uuid,
        sender_id: discord_id as u64,
        is_owner: false,
        descriptions,
        markers: marker_list,
        auto_presets,
        current_period: "daily".to_string(),
        current_mode: default_mode,
        render_data: SessionRenderData {
            current_stats,
            previous_stats,
            skin: skin_image,
            tag_icons: tags,
            snapshots: ws_snapshots.unwrap_or_default(),
            username,
            guild_info,
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
    StatsError,
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
                fetch_skin(data, &resp.uuid, resp.skin_url.as_deref(), resp.slim),
            );
            Ok((resp, guild, skin))
        }
        None => {
            let resp = data.api.get_player_stats(player).await.map_err(map_api_error)?;
            let (guild, skin) = tokio::join!(
                data.api.get_guild(&resp.uuid, Some("player")),
                fetch_skin(data, &resp.uuid, resp.skin_url.as_deref(), resp.slim),
            );
            Ok((resp, guild, skin))
        }
    }
}


async fn fetch_snapshots<G: GameStats>(
    data: &Data,
    uuid: &str,
    discord_id: i64,
    current_stats: &G::Stats,
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
        G::detect_auto_presets(&cache_repo, uuid, current_stats),
    );

    if markers.is_empty() {
        if let Ok(marker) = session_repo.create(uuid, discord_id, "main", now).await {
            markers.push(marker);
        }
    }

    let mut timestamps: Vec<DateTime<Utc>> = PERIODS.iter().map(|period| period.last_reset(now)).collect();
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


fn build_previous_views<G: GameStats>(
    current_stats: &G::Stats,
    snapshots: Vec<Option<(DateTime<Utc>, serde_json::Value)>>,
    markers: &[SessionMarker],
    auto_presets: &[AutoPreset],
    username: &str,
    guild_info: Option<hypixel::GuildInfo>,
    mode: &G::ModeSelection,
) -> (
    HashMap<String, (G::Stats, SessionType, DateTime<Utc>)>,
    HashMap<String, String>,
    Vec<SessionMarker>,
) {
    let now = Utc::now();
    let mut previous_stats = HashMap::new();
    let mut descriptions = HashMap::new();
    let mut snapshot_iter = snapshots.into_iter();

    let to_stats = |value: Option<serde_json::Value>| -> Option<G::Stats> {
        G::extract_stats(username, &value?, guild_info.clone())
    };

    let mut register_view =
        |key: String, previous: G::Stats, session_type: SessionType, timestamp: DateTime<Utc>| {
            descriptions.insert(key.clone(), G::format_delta(current_stats, &previous, mode));
            previous_stats.insert(key, (previous, session_type, timestamp));
        };

    for period in PERIODS {
        let target_time = period.last_reset(now);
        if let Some(previous) = to_stats(snapshot_iter.next().flatten().map(|(_, value)| value)) {
            register_view(period.key().to_string(), previous, period_session_type(period), target_time);
        }
    }

    for period in PERIODS {
        let Some((fp_key, fp_label)) = period.fixed_preset() else { continue };
        let target_time = now - period.duration();
        if let Some(previous) = to_stats(snapshot_iter.next().flatten().map(|(_, value)| value)) {
            register_view(fp_key.to_string(), previous, SessionType::Custom(fp_label.to_string()), target_time);
        }
    }

    for marker in markers {
        if let Some(previous) = to_stats(snapshot_iter.next().flatten().map(|(_, value)| value)) {
            register_view(
                format!("marker:{}", marker.name),
                previous,
                SessionType::Custom(marker.name.clone()),
                marker.snapshot_timestamp,
            );
        }
    }

    for preset in auto_presets {
        if let Some(previous) = to_stats(snapshot_iter.next().flatten().map(|(_, value)| value)) {
            register_view(
                format!("preset:{}", preset.key),
                previous,
                SessionType::Custom(preset.label.clone()),
                preset.timestamp,
            );
        }
    }

    (previous_stats, descriptions, markers.to_vec())
}


fn evict<G: GameStats>(cache: &mut HashMap<String, SessionCacheEntry<G>>) {
    evict_expired(cache, |e| e.last_interaction);
}
