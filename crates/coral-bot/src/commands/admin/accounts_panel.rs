use anyhow::Result;
use serenity::all::{
    ButtonStyle, ComponentInteraction, ComponentInteractionDataKind, Context, CreateActionRow,
    CreateButton, CreateComponent, CreateContainer, CreateContainerComponent, CreateInputText,
    CreateInteractionResponse, CreateModal, CreateModalComponent, CreateSection,
    CreateSectionAccessory, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption,
    InputTextStyle, ModalInteraction,
};

use database::{AccountRepository, CacheRepository, MemberRepository};

use super::manage::fetch_context;
use crate::commands::blacklist::channel;
use crate::framework::{AccessRank, Data};
use crate::interact;
use crate::utils::{resolve_username, separator, text};

fn context_prefix(custom_id: &str) -> &'static str {
    if custom_id.starts_with("dashboard_") {
        "dashboard"
    } else if custom_id.starts_with("link_") {
        "link"
    } else {
        "manage"
    }
}

fn resolve_can_modify(
    prefix: &str,
    invoker_rank: AccessRank,
    target_rank: AccessRank,
    is_self: bool,
) -> bool {
    prefix == "dashboard"
        || is_self
        || (invoker_rank > target_rank && invoker_rank >= AccessRank::Moderator)
}

fn extract_select_value(component: &ComponentInteraction) -> Option<&str> {
    match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.first().map(|s| s.as_str()),
        _ => None,
    }
}

async fn resolve_target_discord_name(
    ctx: &Context,
    component_user: &serenity::all::User,
    target_id: u64,
) -> String {
    if target_id == component_user.id.get() {
        return component_user.name.to_string();
    }
    serenity::all::UserId::new(target_id)
        .to_user(&ctx.http)
        .await
        .map(|u| u.name.to_string())
        .unwrap_or_default()
}

// --- View builders ---

async fn build_accounts_view(
    data: &Data,
    can_modify: bool,
    target_id: u64,
    discord_name: &str,
    prefix: &str,
) -> Result<Vec<CreateComponent<'static>>> {
    let repo = MemberRepository::new(data.db.pool());
    let member = repo
        .get_by_discord_id(target_id as i64)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Member not found"))?;

    let accounts = AccountRepository::new(data.db.pool());
    let alts = accounts.list(member.id).await.unwrap_or_default();

    if member.uuid.is_none() && alts.is_empty() {
        return Ok(build_link_new_view(data, discord_name, prefix, target_id).await);
    }

    let mut parts: Vec<CreateContainerComponent> = vec![text("### Linked Accounts")];

    build_primary_section(
        &mut parts, &member, &alts, data, can_modify, prefix, target_id,
    )
    .await;
    build_alt_sections(&mut parts, &alts, can_modify, prefix, target_id, data).await;
    build_accounts_actions(&mut parts, can_modify, prefix, target_id);

    Ok(vec![CreateComponent::Container(CreateContainer::new(
        parts,
    ))])
}

async fn build_primary_section(
    parts: &mut Vec<CreateContainerComponent<'static>>,
    member: &database::Member,
    alts: &[database::MinecraftAccount],
    data: &Data,
    can_modify: bool,
    prefix: &str,
    target_id: u64,
) {
    let Some(uuid) = &member.uuid else {
        parts.push(separator());
        parts.push(text("No account linked."));
        return;
    };

    let username = resolve_username(uuid, data).await;
    let name = username.as_deref().unwrap_or(uuid);

    parts.push(separator());
    parts.push(text("**Primary**"));

    if alts.is_empty() {
        parts.push(text(format!("**`{name}`**")));
        parts.push(text(format!("-# UUID: {uuid}")));
        parts.push(CreateContainerComponent::ActionRow(
            CreateActionRow::buttons(vec![
                CreateButton::new(format!("{prefix}_remove_account:{target_id}:{uuid}"))
                    .label("Remove")
                    .style(ButtonStyle::Danger)
                    .disabled(!can_modify),
            ]),
        ));
        return;
    }

    let mut options = vec![
        CreateSelectMenuOption::new(name.to_string(), uuid.to_string()).default_selection(true),
    ];
    for alt in alts {
        let alt_name = resolve_username(&alt.uuid, data).await;
        let label = alt_name.unwrap_or_else(|| alt.uuid.clone());
        options.push(CreateSelectMenuOption::new(label, alt.uuid.clone()));
    }

    parts.push(CreateContainerComponent::ActionRow(
        CreateActionRow::SelectMenu(
            CreateSelectMenu::new(
                format!("{prefix}_swap_primary:{target_id}"),
                CreateSelectMenuKind::String {
                    options: options.into(),
                },
            )
            .disabled(!can_modify),
        ),
    ));
    parts.push(text(format!("-# UUID: {uuid}")));
}

async fn build_alt_sections(
    parts: &mut Vec<CreateContainerComponent<'static>>,
    alts: &[database::MinecraftAccount],
    can_modify: bool,
    prefix: &str,
    target_id: u64,
    data: &Data,
) {
    for alt in alts {
        let alt_name = resolve_username(&alt.uuid, data).await;
        let name = alt_name.as_deref().unwrap_or("Unknown");

        parts.push(separator());
        parts.push(CreateContainerComponent::Section(CreateSection::new(
            vec![interact::section_text(&format!(
                "**`{name}`**\n-# UUID: {}",
                alt.uuid
            ))],
            CreateSectionAccessory::Button(
                CreateButton::new(format!("{prefix}_remove_account:{target_id}:{}", alt.uuid))
                    .label("Remove")
                    .style(ButtonStyle::Danger)
                    .disabled(!can_modify),
            ),
        )));
    }
}

fn build_accounts_actions(
    parts: &mut Vec<CreateContainerComponent<'static>>,
    can_modify: bool,
    prefix: &str,
    target_id: u64,
) {
    parts.push(separator());

    let mut buttons = vec![
        CreateButton::new(format!("{prefix}_link_new:{target_id}"))
            .label("Link New Account")
            .style(ButtonStyle::Primary)
            .disabled(!can_modify),
    ];

    if prefix != "link" {
        buttons.push(
            CreateButton::new(format!("{prefix}_accounts_back:{target_id}"))
                .label("Back")
                .style(ButtonStyle::Secondary),
        );
    }

    parts.push(CreateContainerComponent::ActionRow(
        CreateActionRow::buttons(buttons),
    ));
}

async fn build_link_new_view(
    data: &Data,
    discord_name: &str,
    prefix: &str,
    target_id: u64,
) -> Vec<CreateComponent<'static>> {
    let cache = CacheRepository::new(data.db.pool());
    let matches = cache
        .find_by_discord_username(discord_name)
        .await
        .unwrap_or_default();

    let mut parts = vec![text("### Link New Account")];
    parts.extend(crate::commands::user::link::build_link_parts(
        &matches, prefix, target_id,
    ));
    parts.push(separator());
    parts.push(CreateContainerComponent::ActionRow(
        CreateActionRow::buttons(vec![
            CreateButton::new(format!("{prefix}_accounts_back:{target_id}"))
                .label("Back")
                .style(ButtonStyle::Secondary),
        ]),
    ));

    vec![CreateComponent::Container(CreateContainer::new(parts))]
}

async fn refresh_view(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
    can_modify: bool,
    target_id: u64,
) -> Result<()> {
    let prefix = context_prefix(&component.data.custom_id);
    let components = build_accounts_view(data, can_modify, target_id, "", prefix).await?;
    interact::update_message(ctx, component, components).await
}

async fn refresh_view_from_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
    can_modify: bool,
    target_id: u64,
) -> Result<()> {
    let prefix = context_prefix(&modal.data.custom_id);
    let components = build_accounts_view(data, can_modify, target_id, "", prefix).await?;
    interact::update_modal(ctx, modal, components).await
}

// --- Public API ---

pub async fn build_accounts_for_self(
    data: &Data,
    discord_id: u64,
    discord_name: &str,
) -> Result<Vec<CreateComponent<'static>>> {
    build_accounts_view(data, true, discord_id, discord_name, "link").await
}

// --- Button handlers ---

pub async fn handle_accounts_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid button ID"))?;

    let invoker_id = component.user.id.get();
    let is_self = invoker_id == target_id;
    let (invoker_rank, _, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !is_self && (invoker_rank < AccessRank::Moderator || invoker_rank <= target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions")
            .await;
    }

    let prefix = context_prefix(&component.data.custom_id);
    let can_modify = resolve_can_modify(prefix, invoker_rank, target_rank, is_self);
    let components = build_accounts_view(data, can_modify, target_id, "", prefix).await?;
    interact::update_message(ctx, component, components).await
}

pub async fn handle_dashboard_accounts_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let target_id = component.user.id.get();
    let components =
        build_accounts_view(data, true, target_id, &component.user.name, "dashboard").await?;
    interact::update_message(ctx, component, components).await
}

pub async fn handle_accounts_back_generic(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    match context_prefix(&component.data.custom_id) {
        "dashboard" => handle_dashboard_accounts_back(ctx, component, data).await,
        "manage" => handle_manage_accounts_back(ctx, component, data).await,
        "link" => {
            let target_id = component.user.id.get();
            let components =
                build_accounts_view(data, true, target_id, &component.user.name, "link").await?;
            interact::update_message(ctx, component, components).await
        }
        _ => Ok(()),
    }
}

pub async fn handle_manage_accounts_back(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid button ID"))?;

    let invoker_id = component.user.id.get();
    let (invoker_rank, _, _) = fetch_context(data, invoker_id, target_id).await?;

    let components = super::manage::build_main_view(data, invoker_rank, target_id).await;
    interact::update_message(ctx, component, components).await
}

pub async fn handle_dashboard_accounts_back(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let discord_id = component.user.id.get() as i64;
    let repo = MemberRepository::new(data.db.pool());

    let Some(member) = repo.get_by_discord_id(discord_id).await? else {
        return interact::send_component_error(ctx, component, "Error", "You are not registered.")
            .await;
    };

    let components = crate::commands::user::dashboard::build_dashboard_view(&member, data).await;
    interact::update_message(ctx, component, components).await
}

pub async fn handle_link_new(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid button ID"))?;
    let prefix = context_prefix(&component.data.custom_id);
    let discord_name = resolve_target_discord_name(ctx, &component.user, target_id).await;

    let components = build_link_new_view(data, &discord_name, prefix, target_id).await;
    interact::update_message(ctx, component, components).await
}

pub async fn handle_link_pick(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid select ID"))?;

    let Some(uuid) = extract_select_value(component) else {
        return Ok(());
    };

    let invoker_id = component.user.id.get();
    let (_, target, _) = fetch_context(data, invoker_id, target_id).await?;

    let Some(member) = &target else {
        return interact::send_component_error(ctx, component, "Error", "User is not registered")
            .await;
    };

    match crate::accounts::check_link(data, uuid, &component.user.name).await {
        crate::accounts::LinkCheck::Verified { uuid, .. } => {
            crate::accounts::link_alt(ctx, data, target_id, member.id, &uuid).await?;
            refresh_view(ctx, component, data, true, target_id).await
        }
        _ => {
            interact::send_component_error(
                ctx,
                component,
                "Verification Failed",
                "That account's Discord link no longer matches.",
            )
            .await
        }
    }
}

pub async fn handle_add_account_button(
    ctx: &Context,
    component: &ComponentInteraction,
) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid button ID"))?;
    let prefix = context_prefix(&component.data.custom_id);

    let input = CreateInputText::new(InputTextStyle::Short, "username")
        .placeholder("Minecraft username")
        .min_length(1)
        .max_length(16);

    let modal = CreateModal::new(
        format!("{prefix}_add_account_modal:{target_id}"),
        "Add Account",
    )
    .components(vec![CreateModalComponent::Label(
        serenity::all::CreateLabel::input_text("Minecraft Username", input),
    )]);

    component
        .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
        .await?;
    Ok(())
}

pub async fn handle_add_code_button(ctx: &Context, component: &ComponentInteraction) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid button ID"))?;
    let prefix = context_prefix(&component.data.custom_id);

    let input = CreateInputText::new(InputTextStyle::Short, "code")
        .placeholder("1234")
        .min_length(4)
        .max_length(4);

    let modal = CreateModal::new(
        format!("{prefix}_add_code_modal:{target_id}"),
        "Add Account via Code",
    )
    .components(vec![CreateModalComponent::Label(
        serenity::all::CreateLabel::input_text("Verification Code", input),
    )]);

    component
        .create_response(&ctx.http, CreateInteractionResponse::Modal(modal))
        .await?;
    Ok(())
}

// --- Modal handlers ---

pub async fn handle_add_code_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
) -> Result<()> {
    let prefix = context_prefix(&modal.data.custom_id);
    let target_id = interact::parse_id(&modal.data.custom_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid modal ID"))?;
    let code = interact::extract_modal_value(&modal.data.components, "code");

    let invoker_id = modal.user.id.get();
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;
    let is_self = invoker_id == target_id;

    if !is_self && (invoker_rank < AccessRank::Moderator || invoker_rank <= target_rank) {
        return interact::send_modal_error(ctx, modal, "Error", "Insufficient permissions").await;
    }

    let Some(member) = &target else {
        return interact::send_modal_error(ctx, modal, "Error", "User is not registered").await;
    };

    let Some(player) = data.verify_handle.redeem(&code) else {
        return interact::send_modal_error(
            ctx, modal, "Invalid Code",
            "That code is invalid or has expired.\n\nJoin the verification server to get a new code.",
        )
        .await;
    };

    let uuid = player.uuid.as_simple().to_string();
    crate::accounts::link_alt(ctx, data, target_id, member.id, &uuid).await?;

    let can_modify = resolve_can_modify(prefix, invoker_rank, target_rank, is_self);
    refresh_view_from_modal(ctx, modal, data, can_modify, target_id).await
}

pub async fn handle_add_account_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
) -> Result<()> {
    let prefix = context_prefix(&modal.data.custom_id);
    let target_id = interact::parse_id(&modal.data.custom_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid modal ID"))?;
    let username = interact::extract_modal_value(&modal.data.components, "username");

    let invoker_id = modal.user.id.get();
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;
    let is_self = invoker_id == target_id;

    if !is_self && (invoker_rank < AccessRank::Moderator || invoker_rank <= target_rank) {
        return interact::send_modal_error(ctx, modal, "Error", "Insufficient permissions").await;
    }

    let Some(member) = &target else {
        return interact::send_modal_error(ctx, modal, "Error", "User is not registered").await;
    };

    let stats = match data.api.get_player_stats(&username).await {
        Ok(s) => s,
        Err(_) => {
            return interact::send_modal_error(
                ctx,
                modal,
                "Error",
                &format!("Could not find player: {username}"),
            )
            .await;
        }
    };

    let uuid = stats.uuid.replace('-', "");
    let discord_name = resolve_target_discord_name_from_modal(ctx, modal, target_id).await;
    let verified = stats
        .hypixel
        .as_ref()
        .map(|h| crate::accounts::is_discord_linked(h, &discord_name))
        .unwrap_or(false);

    if verified {
        crate::accounts::link_alt(ctx, data, target_id, member.id, &uuid).await?;
        let can_modify = resolve_can_modify(prefix, invoker_rank, target_rank, is_self);
        return refresh_view_from_modal(ctx, modal, data, can_modify, target_id).await;
    }

    if is_self && invoker_rank < AccessRank::Moderator {
        return interact::send_modal_error(
            ctx,
            modal,
            "Error",
            "Your Discord must be linked in Hypixel social settings for this account",
        )
        .await;
    }

    show_force_link_prompt(ctx, modal, data, &stats.username, prefix, target_id, &uuid).await
}

async fn resolve_target_discord_name_from_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    target_id: u64,
) -> String {
    if target_id == modal.user.id.get() {
        return modal.user.name.to_string();
    }
    serenity::all::UserId::new(target_id)
        .to_user(&ctx.http)
        .await
        .map(|u| u.name.to_string())
        .unwrap_or_default()
}

async fn show_force_link_prompt(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
    player_name: &str,
    prefix: &str,
    target_id: u64,
    uuid: &str,
) -> Result<()> {
    let mut components = build_accounts_view(data, true, target_id, "", prefix).await?;

    components.push(CreateComponent::Container(
        CreateContainer::new(vec![
            text(format!(
                "**{player_name}** does not have <@{target_id}>'s Discord linked in Hypixel social settings.",
            )),
            separator(),
            CreateContainerComponent::ActionRow(CreateActionRow::buttons(vec![
                CreateButton::new(format!("{prefix}_force_add:{target_id}:{uuid}"))
                    .label("Force Link")
                    .style(ButtonStyle::Danger),
                CreateButton::new(format!("{prefix}_cancel_add:{target_id}"))
                    .label("Cancel")
                    .style(ButtonStyle::Secondary),
            ])),
        ])
        .accent_color(channel::COLOR_ERROR),
    ));

    interact::update_modal(ctx, modal, components).await
}

pub async fn handle_force_add(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let (target_id, uuid) = interact::parse_ids(&component.data.custom_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid button ID"))?;

    let invoker_id = component.user.id.get();
    let is_self = invoker_id == target_id;
    let (invoker_rank, target, _) = fetch_context(data, invoker_id, target_id).await?;

    if !is_self && invoker_rank < AccessRank::Moderator {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions")
            .await;
    }

    let Some(member) = &target else {
        return interact::send_component_error(ctx, component, "Error", "User is not registered")
            .await;
    };

    crate::accounts::link_alt(ctx, data, target_id, member.id, &uuid).await?;
    refresh_view(ctx, component, data, true, target_id).await
}

pub async fn handle_remove_account(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let (target_id, uuid) = interact::parse_ids(&component.data.custom_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid button ID"))?;

    let invoker_id = component.user.id.get();
    let is_self = invoker_id == target_id;
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !is_self && (invoker_rank < AccessRank::Moderator || invoker_rank <= target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions")
            .await;
    }

    let Some(member) = &target else {
        return interact::send_component_error(ctx, component, "Error", "User is not registered")
            .await;
    };

    if member.uuid.as_deref() == Some(&uuid) {
        MemberRepository::new(data.db.pool())
            .clear_uuid(target_id as i64)
            .await?;
    } else {
        AccountRepository::new(data.db.pool())
            .remove(member.id, &uuid)
            .await?;
    }

    let prefix = context_prefix(&component.data.custom_id);
    let can_modify = resolve_can_modify(prefix, invoker_rank, target_rank, is_self);
    refresh_view(ctx, component, data, can_modify, target_id).await
}

pub async fn handle_swap_primary(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid select ID"))?;

    let Some(new_uuid) = extract_select_value(component) else {
        return Ok(());
    };

    let invoker_id = component.user.id.get();
    let is_self = invoker_id == target_id;
    let (invoker_rank, target, target_rank) = fetch_context(data, invoker_id, target_id).await?;

    if !is_self && (invoker_rank < AccessRank::Moderator || invoker_rank <= target_rank) {
        return interact::send_component_error(ctx, component, "Error", "Insufficient permissions")
            .await;
    }

    let Some(member) = &target else {
        return interact::send_component_error(ctx, component, "Error", "User is not registered")
            .await;
    };

    let old_primary = member.uuid.as_deref().unwrap_or("");

    if new_uuid != old_primary {
        let accounts = AccountRepository::new(data.db.pool());
        let repo = MemberRepository::new(data.db.pool());

        if !old_primary.is_empty() {
            accounts.add(member.id, old_primary).await?;
        }
        accounts.remove(member.id, new_uuid).await?;
        repo.set_uuid(target_id as i64, new_uuid).await?;

        tokio::spawn(crate::sync::sync_user(
            ctx.clone(),
            data.clone(),
            serenity::all::UserId::new(target_id),
        ));
    }

    let prefix = context_prefix(&component.data.custom_id);
    let can_modify = resolve_can_modify(prefix, invoker_rank, target_rank, is_self);
    refresh_view(ctx, component, data, can_modify, target_id).await
}

pub async fn handle_cancel_add(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let target_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow::anyhow!("Invalid button ID"))?;
    refresh_view(ctx, component, data, true, target_id).await
}
