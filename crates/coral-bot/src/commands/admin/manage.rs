use anyhow::{Result, anyhow};
use serenity::all::*;

use database::{DeveloperKeyRepository, MemberRepository, permissions};

use crate::{
    commands::blacklist::channel,
    framework::{AccessRank, AccessRankExt, Data},
    interact,
    utils::{format_number, generate_api_key, resolve_username, separator, text},
};


pub fn register() -> CreateCommand<'static> {
    CreateCommand::new("manage")
        .description("Open the user management panel")
        .add_option(
            CreateCommandOption::new(CommandOptionType::User, "user", "Target user").required(true),
        )
}


pub async fn run(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    let invoker_id = command.user.id.get();
    let repo = MemberRepository::new(data.db.pool());
    let invoker = repo.get_by_discord_id(invoker_id as i64).await?;
    let invoker_rank = AccessRank::of(data, invoker_id, invoker.as_ref());
    if invoker_rank < AccessRank::Moderator {
        return interact::send_error(ctx, command, "Error", "You don't have permission to use this command").await;
    }
    let target_id = command.data.options.first()
        .and_then(|o| match &o.value {
            CommandDataOptionValue::User(id) => Some(id.get()),
            _ => None,
        })
        .ok_or_else(|| anyhow!("Missing user"))?;

    let components = build_main_view(data, invoker_rank, target_id).await;
    command.create_response(&ctx.http, CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .flags(MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL)
            .components(components),
    )).await?;
    Ok(())
}


pub(crate) async fn build_main_view(
    data: &Data,
    invoker_rank: AccessRank,
    target_id: u64,
) -> Vec<CreateComponent<'static>> {
    let repo = MemberRepository::new(data.db.pool());
    let target = repo.get_by_discord_id(target_id as i64).await.ok().flatten();
    let target_rank = AccessRank::of(data, target_id, target.as_ref());
    let can_modify = invoker_rank > target_rank || invoker_rank >= AccessRank::Owner;

    let mut parts: Vec<CreateContainerComponent> = vec![text(format!("## User Management — <@{target_id}>"))];
    match &target {
        Some(m) => {
            parts.push(separator());
            match &m.uuid {
                Some(uuid) => {
                    let username = resolve_username(uuid, data).await;
                    let name = username.as_deref().unwrap_or(uuid);
                    parts.push(text(format!("### Account\n**{name}**\n-# {uuid}")));
                }
                None => parts.push(text("### Account\nNo account linked")),
            }
            parts.push(CreateContainerComponent::ActionRow(
                CreateActionRow::buttons(vec![
                    CreateButton::new(format!("manage_accounts:{target_id}"))
                        .label("Accounts")
                        .style(ButtonStyle::Secondary)
                        .disabled(!can_modify),
                ]),
            ));
        }
        None => {
            parts.push(separator());
            parts.push(CreateContainerComponent::Section(CreateSection::new(
                vec![interact::section_text("### Account\nNot registered")],
                CreateSectionAccessory::Button(
                    CreateButton::new(format!("manage_register:{target_id}"))
                        .label("Register")
                        .style(ButtonStyle::Primary)
                        .disabled(!can_modify),
                ),
            )));
        }
    }

    if let Some(m) = &target {
        parts.push(separator());

        let options = access_level_options(invoker_rank, target_rank);
        let disabled = !can_modify || options.is_empty();
        parts.push(CreateContainerComponent::ActionRow(
            CreateActionRow::SelectMenu(
                CreateSelectMenu::new(
                    format!("manage_access_select:{target_id}"),
                    CreateSelectMenuKind::String { options: options.into() },
                )
                .placeholder(target_rank.label())
                .disabled(disabled),
            ),
        ));

        parts.push(separator());

        let status = if m.key_locked { "Locked" } else if m.api_key.is_some() { "Active" } else { "None" };
        parts.push(text(format!(
            "### Personal Key\n{status}\n-# {} requests · Registered since <t:{}:D>",
            format_number(m.request_count as u64),
            m.join_date.timestamp()
        )));

        let mut key_buttons = vec![if m.key_locked {
            CreateButton::new(format!("manage_unlock:{target_id}"))
                .label("Unlock").style(ButtonStyle::Success).disabled(!can_modify)
        } else {
            CreateButton::new(format!("manage_lock:{target_id}"))
                .label("Lock").style(ButtonStyle::Danger).disabled(!can_modify)
        }];

        if invoker_rank >= AccessRank::Helper && can_modify {
            let (label, style) = if m.tagging_disabled {
                ("Enable Tagging", ButtonStyle::Success)
            } else {
                ("Disable Tagging", ButtonStyle::Danger)
            };
            key_buttons.push(
                CreateButton::new(format!("manage_toggle_tagging:{target_id}"))
                    .label(label).style(style),
            );
        }

        parts.push(CreateContainerComponent::ActionRow(CreateActionRow::buttons(key_buttons)));
        parts.push(separator());

        let dev_key = DeveloperKeyRepository::new(data.db.pool())
            .get_by_member_id(m.id).await.ok().flatten();

        match dev_key {
            Some(dk) => {
                let status = if dk.locked { "Locked" } else { "Active" };
                let perm_labels: Vec<&str> = permissions::ALL.iter()
                    .filter(|&&p| dk.permissions & p != 0)
                    .map(|&p| permissions::label(p))
                    .collect();
                let perm_display = if perm_labels.is_empty() { "None".into() } else { perm_labels.join(", ") };

                parts.push(text(format!(
                    "### Developer Key\n{status}\n-# {} requests · {} req/5min · {perm_display}",
                    format_number(dk.request_count as u64),
                    format_number(dk.rate_limit as u64),
                )));

                let lock_button = if dk.locked {
                    CreateButton::new(format!("manage_unlock_dev:{target_id}"))
                        .label("Unlock").style(ButtonStyle::Success).disabled(!can_modify)
                } else {
                    CreateButton::new(format!("manage_lock_dev:{target_id}"))
                        .label("Lock").style(ButtonStyle::Danger).disabled(!can_modify)
                };
                let delete_button = CreateButton::new(format!("manage_delete_dev:{target_id}"))
                    .label("Delete").style(ButtonStyle::Danger).disabled(!can_modify);
                let rate_limit_button = CreateButton::new(format!("manage_dev_rate_limit:{target_id}"))
                    .label("Rate Limit").style(ButtonStyle::Secondary).disabled(!can_modify);
                parts.push(CreateContainerComponent::ActionRow(
                    CreateActionRow::buttons(vec![lock_button, delete_button, rate_limit_button]),
                ));

                let perm_options: Vec<_> = permissions::ALL.iter().map(|&p| {
                    CreateSelectMenuOption::new(permissions::label(p), p.to_string())
                        .default_selection(dk.permissions & p != 0)
                }).collect();

                parts.push(CreateContainerComponent::ActionRow(
                    CreateActionRow::SelectMenu(
                        CreateSelectMenu::new(
                            format!("manage_dev_perms:{target_id}"),
                            CreateSelectMenuKind::String { options: perm_options.into() },
                        )
                        .placeholder("Permissions")
                        .max_values(permissions::ALL.len() as u8)
                        .min_values(0)
                        .disabled(!can_modify),
                    ),
                ));
            }
            None => {
                parts.push(text("### Developer Key\nNone"));
                if can_modify {
                    parts.push(CreateContainerComponent::ActionRow(
                        CreateActionRow::buttons(vec![
                            CreateButton::new(format!("manage_create_dev:{target_id}"))
                                .label("Create Dev Key")
                                .style(ButtonStyle::Primary),
                        ]),
                    ));
                }
            }
        }
        parts.push(separator());

        let total_tags = database::BlacklistRepository::new(data.db.pool())
            .count_tags_by_user(m.discord_id)
            .await
            .unwrap_or(0);

        parts.push(text(format!(
            "Added **{}** tags to the blacklist\n\
             **{}** accepted tag reviews · **{}** rejected\n\
             **{}** accurate verdicts",
            total_tags, m.accepted_tags, m.rejected_tags, m.accurate_verdicts
        )));

        let strikes = m.config.get("strikes").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        if strikes.is_empty() {
            parts.push(text("-# No strikes"));
        } else {
            let strike_text = strikes.iter().enumerate().fold(format!("### Strikes ({})", strikes.len()), |mut acc, (i, strike)| {
                let reason = strike.get("reason").and_then(|v| v.as_str()).unwrap_or("Unknown");
                let struck_by = strike.get("struck_by").and_then(|v| v.as_u64()).unwrap_or(0);
                acc.push_str(&format!("\n{}. \"{}\" — <@{}>", i + 1, reason, struck_by));
                acc
            });
            parts.push(text(strike_text));

            if can_modify {
                let buttons: Vec<_> = (0..strikes.len().min(5))
                    .map(|i| {
                        CreateButton::new(format!("manage_remove_strike:{target_id}:{i}"))
                            .label(format!("Remove #{}", i + 1))
                            .style(ButtonStyle::Danger)
                    })
                    .collect();
                parts.push(CreateContainerComponent::ActionRow(CreateActionRow::buttons(buttons)));
            }
        }
    }

    vec![CreateComponent::Container(CreateContainer::new(parts))]
}


fn access_level_options(invoker_rank: AccessRank, current: AccessRank) -> Vec<CreateSelectMenuOption<'static>> {
    [(AccessRank::Default, "Default", "Default access"), (AccessRank::Member, "Member", "Member access"),
     (AccessRank::Helper, "Helper", "Helper access"), (AccessRank::Moderator, "Moderator", "Moderator access"),
     (AccessRank::Admin, "Admin", "Administrator access")]
        .into_iter()
        .filter(|(rank, _, _)| *rank < invoker_rank)
        .map(|(rank, label, desc)| CreateSelectMenuOption::new(label, rank.to_level().to_string())
            .description(desc).default_selection(rank == current))
        .collect()
}


pub async fn fetch_context(data: &Data, invoker_id: u64, target_id: u64) -> Result<(AccessRank, Option<database::Member>, AccessRank)> {
    let repo = MemberRepository::new(data.db.pool());
    let invoker = repo.get_by_discord_id(invoker_id as i64).await?;
    let invoker_rank = AccessRank::of(data, invoker_id, invoker.as_ref());
    let target = repo.get_by_discord_id(target_id as i64).await?;
    let target_rank = AccessRank::of(data, target_id, target.as_ref());
    Ok((invoker_rank, target, target_rank))
}


async fn refresh_main(ctx: &Context, component: &ComponentInteraction, data: &Data, invoker_rank: AccessRank, target_id: u64) -> Result<()> {
    interact::update_message(ctx, component, build_main_view(data, invoker_rank, target_id).await).await
}


async fn refresh_main_from_modal(ctx: &Context, modal: &ModalInteraction, data: &Data, invoker_rank: AccessRank, target_id: u64) -> Result<()> {
    interact::update_modal(ctx, modal, build_main_view(data, invoker_rank, target_id).await).await
}


fn require_mod_over(invoker_rank: AccessRank, target_rank: AccessRank) -> bool {
    invoker_rank >= AccessRank::Owner || (invoker_rank >= AccessRank::Moderator && invoker_rank > target_rank)
}


pub async fn handle_access_select(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let new_level = match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.first().and_then(|s| s.parse::<i16>().ok()),
        _ => None,
    };
    let Some(new_level) = new_level else { return Ok(()) };

    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid select ID"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }

    let new_rank = AccessRank::from_level(new_level);
    if new_rank >= invoker_rank {
        return interact::send_component_error(ctx, component, "Error", "Cannot assign a rank equal to or above your own").await;
    }
    if target.is_none() {
        return interact::send_component_error(ctx, component, "Error", "User is not registered").await;
    }

    MemberRepository::new(data.db.pool()).set_access_level(target_id as i64, new_level).await?;
    channel::post_access_changed(ctx, data, target_id, target_rank, new_rank, invoker_id).await;
    refresh_main(ctx, component, data, invoker_rank, target_id).await
}


pub async fn handle_lock_button(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid button ID"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, _, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }

    MemberRepository::new(data.db.pool()).lock_key(target_id as i64).await?;
    channel::post_key_locked(ctx, data, target_id, invoker_id).await;
    refresh_main(ctx, component, data, invoker_rank, target_id).await
}


pub async fn handle_unlock_button(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid button ID"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, _, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }

    MemberRepository::new(data.db.pool()).unlock_key(target_id as i64).await?;
    channel::post_key_unlocked(ctx, data, target_id, invoker_id).await;
    refresh_main(ctx, component, data, invoker_rank, target_id).await
}


pub async fn handle_register_button(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid button ID"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, _, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }

    let input = CreateInputText::new(InputTextStyle::Short, "username")
        .placeholder("Minecraft username")
        .min_length(1)
        .max_length(16);
    let modal = CreateModal::new(format!("manage_register_modal:{target_id}"), "Register User")
        .components(vec![CreateModalComponent::Label(
            CreateLabel::input_text("Minecraft Username", input),
        )]);

    component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}


pub async fn handle_register_modal(ctx: &Context, modal: &ModalInteraction, data: &Data) -> Result<()> {
    let target_id = interact::parse_id(&modal.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid modal ID"))?;
    let username = interact::extract_modal_value(&modal.data.components, "username");
    let invoker_id = modal.user.id.get();
    let (invoker_rank, _, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_modal_error(ctx, modal, "Error", "Insufficient permissions").await;
    }

    let stats = match data.api.get_player_stats(&username).await {
        Ok(s) => s,
        Err(_) => {
            return interact::send_modal_error(
                ctx, modal, "Error", &format!("Could not find player: {username}"),
            )
            .await;
        }
    };

    let uuid = stats.uuid.replace('-', "");
    let discord_user = UserId::new(target_id).to_user(&ctx.http).await;
    let discord_name = discord_user.as_ref().map(|u| u.name.as_str()).unwrap_or("");

    let verified = stats.hypixel.as_ref()
        .map(|h| crate::accounts::is_discord_linked(h, discord_name))
        .unwrap_or(false);

    if verified {
        crate::accounts::link_primary(ctx, data, target_id, &uuid).await?;
        return refresh_main_from_modal(ctx, modal, data, invoker_rank, target_id).await;
    }

    let container = CreateComponent::Container(
        CreateContainer::new(vec![
            text(format!(
                "## Discord Mismatch\n**{}** (`{uuid}`) does not have <@{target_id}>'s Discord linked in Hypixel social settings.\n\nForce link anyway?",
                stats.username
            )),
            separator(),
            CreateContainerComponent::ActionRow(CreateActionRow::buttons(vec![
                CreateButton::new(format!("manage_force_link:{target_id}:{uuid}"))
                    .label("Force Link")
                    .style(ButtonStyle::Danger),
            ])),
        ])
        .accent_color(channel::COLOR_ERROR),
    );

    modal
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL)
                    .components(vec![container]),
            ),
        )
        .await?;
    Ok(())
}


pub async fn handle_force_link(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let (target_id, uuid) = interact::parse_ids(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid button ID"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, _, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }

    crate::accounts::link_primary(ctx, data, target_id, &uuid).await?;
    refresh_main(ctx, component, data, invoker_rank, target_id).await
}


pub async fn handle_toggle_tagging(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid button ID"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if invoker_rank < AccessRank::Helper || (invoker_rank <= target_rank && invoker_rank < AccessRank::Owner) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }

    let Some(target_member) = target else {
        return interact::send_component_error(ctx, component, "Error", "User is not registered").await;
    };

    let new_state = !target_member.tagging_disabled;
    MemberRepository::new(data.db.pool())
        .set_tagging_disabled(target_id as i64, new_state)
        .await?;
    channel::post_tagging_toggled(ctx, data, target_id, new_state, invoker_id).await;
    refresh_main(ctx, component, data, invoker_rank, target_id).await
}


pub async fn handle_remove_strike(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let (target_id, strike_index_str) = interact::parse_ids(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid button ID"))?;
    let strike_index: usize = strike_index_str.parse()
        .map_err(|_| anyhow!("Invalid strike index"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, _, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }

    MemberRepository::new(data.db.pool()).remove_strike(target_id as i64, strike_index).await?;
    refresh_main(ctx, component, data, invoker_rank, target_id).await
}


pub async fn handle_create_dev_key(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid button ID"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }
    let Some(m) = target else {
        return interact::send_component_error(ctx, component, "Error", "User is not registered").await;
    };

    DeveloperKeyRepository::new(data.db.pool())
        .create(m.id, &generate_api_key(), "Developer Key", 0, 600)
        .await?;
    refresh_main(ctx, component, data, invoker_rank, target_id).await
}


pub async fn handle_lock_dev_key(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid button ID"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }
    let Some(m) = target else { return Ok(()) };

    DeveloperKeyRepository::new(data.db.pool()).set_locked(m.id, true).await?;
    refresh_main(ctx, component, data, invoker_rank, target_id).await
}


pub async fn handle_unlock_dev_key(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid button ID"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }
    let Some(m) = target else { return Ok(()) };

    DeveloperKeyRepository::new(data.db.pool()).set_locked(m.id, false).await?;
    refresh_main(ctx, component, data, invoker_rank, target_id).await
}


pub async fn handle_delete_dev_key(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid button ID"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }
    let Some(m) = target else { return Ok(()) };

    DeveloperKeyRepository::new(data.db.pool()).delete(m.id).await?;
    refresh_main(ctx, component, data, invoker_rank, target_id).await
}


pub async fn handle_dev_rate_limit_button(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid button ID"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }
    let Some(m) = target else { return Ok(()) };

    let current = DeveloperKeyRepository::new(data.db.pool())
        .get_by_member_id(m.id).await?.map(|dk| dk.rate_limit.to_string()).unwrap_or_default();

    let input = CreateInputText::new(InputTextStyle::Short, "rate_limit")
        .placeholder("e.g. 3000")
        .value(current)
        .min_length(1)
        .max_length(10);
    let modal = CreateModal::new(format!("manage_dev_rate_limit_modal:{target_id}"), "Set Rate Limit")
        .components(vec![CreateModalComponent::Label(
            CreateLabel::input_text("Requests per 5 minutes", input),
        )]);
    component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}


pub async fn handle_dev_rate_limit_modal(ctx: &Context, modal: &ModalInteraction, data: &Data) -> Result<()> {
    let target_id = interact::parse_id(&modal.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid modal ID"))?;
    let invoker_id = modal.user.id.get();
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_modal_error(ctx, modal, "Error", "Insufficient permissions").await;
    }
    let Some(m) = target else { return Ok(()) };

    let value = interact::extract_modal_value(&modal.data.components, "rate_limit");

    let Ok(rate_limit) = value.trim().parse::<i32>() else {
        return interact::send_modal_error(ctx, modal, "Error", "Invalid number").await;
    };

    if rate_limit < 0 {
        return interact::send_modal_error(ctx, modal, "Error", "Rate limit must be positive").await;
    }

    DeveloperKeyRepository::new(data.db.pool()).set_rate_limit(m.id, rate_limit).await?;

    let components = build_main_view(data, invoker_rank, target_id).await;
    modal.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(
        CreateInteractionResponseMessage::new()
            .flags(MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL)
            .components(components),
    )).await?;
    Ok(())
}


pub async fn handle_dev_perms_select(ctx: &Context, component: &ComponentInteraction, data: &Data) -> Result<()> {
    let selected: Vec<i64> = match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.iter().filter_map(|s| s.parse().ok()).collect(),
        _ => vec![],
    };

    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("Invalid select ID"))?;
    let invoker_id = component.user.id.get();
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !require_mod_over(invoker_rank, target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions").await;
    }
    let Some(m) = target else { return Ok(()) };

    DeveloperKeyRepository::new(data.db.pool())
        .set_permissions(m.id, selected.iter().fold(0i64, |acc, &p| acc | p))
        .await?;
    refresh_main(ctx, component, data, invoker_rank, target_id).await
}
