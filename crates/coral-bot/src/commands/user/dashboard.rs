use anyhow::Result;
use serenity::all::*;

use database::{BlacklistRepository, DeveloperKeyRepository, MemberRepository};

use crate::{
    framework::{AccessRank, AccessRankExt, Data},
    interact::{self, section_text},
    utils::{format_number, generate_api_key, resolve_username, separator, text},
};


pub fn register() -> CreateCommand<'static> {
    CreateCommand::new("dashboard").description("View your account dashboard and settings")
}


pub async fn run(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    let discord_id = command.user.id.get() as i64;
    let repo = MemberRepository::new(data.db.pool());
    let mut member = match repo.get_by_discord_id(discord_id).await? {
        Some(m) => m,
        None => {
            repo.create(discord_id).await?;
            repo.get_by_discord_id(discord_id).await?
                .ok_or_else(|| anyhow::anyhow!("failed to retrieve member after creation"))?
        }
    };
    if member.api_key.is_none() {
        let key = generate_api_key();
        repo.set_api_key(discord_id, &key).await?;
        member.api_key = Some(key);
    }
    let components = build_dashboard_view(&member, data).await;

    command.create_response(&ctx.http, CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .flags(MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL)
            .components(components),
    )).await?;
    Ok(())
}


pub async fn handle_regenerate_key(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    let view = vec![CreateComponent::Container(CreateContainer::new(vec![
        text("## Regenerate Personal Key"),
        separator(),
        text("Are you sure? Your previous key will stop working."),
        CreateContainerComponent::ActionRow(CreateActionRow::buttons(vec![
            CreateButton::new("confirm_regenerate_key").label("Confirm").style(ButtonStyle::Danger),
        ])),
    ]))];
    component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(
        CreateInteractionResponseMessage::new().flags(MessageFlags::IS_COMPONENTS_V2).components(view),
    )).await?;
    Ok(())
}


pub async fn handle_confirm_regenerate_key(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let discord_id = component.user.id.get() as i64;
    let repo = MemberRepository::new(data.db.pool());
    let Some(mut member) = repo.get_by_discord_id(discord_id).await? else {
        return interact::send_component_error(ctx, component, "Error", "You are not registered.")
            .await;
    };

    if member.key_locked {
        return interact::send_component_error(ctx, component, "Error", "Your API key is locked.").await;
    }
    let new_key = generate_api_key();
    repo.set_api_key(discord_id, &new_key).await?;
    member.api_key = Some(new_key);

    let components = build_dashboard_view(&member, data).await;
    interact::update_message(ctx, component, components).await
}


pub async fn handle_regenerate_dev_key(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    let view = vec![CreateComponent::Container(CreateContainer::new(vec![
        text("## Regenerate Developer Key"),
        separator(),
        text("Are you sure? Your previous developer key will stop working."),
        CreateContainerComponent::ActionRow(CreateActionRow::buttons(vec![
            CreateButton::new("confirm_regenerate_dev_key").label("Confirm").style(ButtonStyle::Danger),
        ])),
    ]))];
    component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(
        CreateInteractionResponseMessage::new().flags(MessageFlags::IS_COMPONENTS_V2).components(view),
    )).await?;
    Ok(())
}


pub async fn handle_confirm_regenerate_dev_key(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let discord_id = component.user.id.get() as i64;
    let member_repo = MemberRepository::new(data.db.pool());
    let dev_repo = DeveloperKeyRepository::new(data.db.pool());
    let Some(member) = member_repo.get_by_discord_id(discord_id).await? else {
        return interact::send_component_error(ctx, component, "Error", "You are not registered.")
            .await;
    };

    let Some(dev_key) = dev_repo.get_by_member_id(member.id).await? else {
        return interact::send_component_error(ctx, component, "Error", "You don't have a developer key.").await;
    };
    if dev_key.locked {
        return interact::send_component_error(ctx, component, "Error", "Your developer key is locked.").await;
    }
    dev_repo.set_api_key(member.id, &generate_api_key()).await?;

    let components = build_dashboard_view(&member, data).await;
    interact::update_message(ctx, component, components).await
}


pub(crate) async fn build_dashboard_view(
    member: &database::Member,
    data: &Data,
) -> Vec<CreateComponent<'static>> {
    let discord_id = member.discord_id as u64;
    let rank = AccessRank::of(data, discord_id, Some(member));
    let mut parts: Vec<CreateContainerComponent> = vec![text("## Dashboard")];
    parts.push(separator());
    match &member.uuid {
        Some(uuid) => {
            let username = resolve_username(uuid, data).await;
            let name = username.as_deref().unwrap_or(uuid);
            parts.push(CreateContainerComponent::Section(CreateSection::new(
                vec![section_text(&format!("### Account\n**{name}**\n-# {uuid}"))],
                CreateSectionAccessory::Button(
                    CreateButton::new(format!("dashboard_accounts:{discord_id}"))
                        .label("Manage")
                        .style(ButtonStyle::Secondary),
                ),
            )));
        }
        None => {
            parts.push(CreateContainerComponent::Section(CreateSection::new(
                vec![section_text("### Account\nNo account linked.")],
                CreateSectionAccessory::Button(
                    CreateButton::new(format!("dashboard_accounts:{discord_id}"))
                        .label("Link Account")
                        .style(ButtonStyle::Primary),
                ),
            )));
        }
    }

    parts.push(separator());
    let personal_key_text = match (&member.key_locked, &member.api_key) {
        (true, _) => "### Personal Key\nLocked".into(),
        (_, Some(key)) => format!("### Personal Key (click to reveal)\n||```{key}```||"),
        _ => "### Personal Key\nNone".into(),
    };

    parts.push(CreateContainerComponent::Section(CreateSection::new(
        vec![section_text(&personal_key_text)],
        CreateSectionAccessory::Button(
            CreateButton::new("regenerate_key")
                .label("Regenerate")
                .style(ButtonStyle::Secondary)
                .disabled(member.key_locked),
        ),
    )));

    let personal_limit = match member.access_level {
        4.. => 3000,
        2..=3 => 1200,
        _ => 600,
    };
    parts.push(text(format!(
        "-# {} · {} requests · {personal_limit} req/5min · Registered <t:{}:D>",
        rank.label(),
        format_number(member.request_count as u64),
        member.join_date.timestamp()
    )));

    parts.push(separator());

    let dev_key = DeveloperKeyRepository::new(data.db.pool())
        .get_by_member_id(member.id)
        .await
        .ok()
        .flatten();

    match dev_key {
        Some(dk) => {
            let dev_key_text = if dk.locked {
                "### Developer Key\nLocked".into()
            } else {
                format!("### Developer Key (click to reveal)\n||```{}```||", dk.api_key)
            };
            parts.push(CreateContainerComponent::Section(CreateSection::new(
                vec![section_text(&dev_key_text)],
                CreateSectionAccessory::Button(
                    CreateButton::new("regenerate_dev_key")
                        .label("Regenerate")
                        .style(ButtonStyle::Secondary)
                        .disabled(dk.locked),
                ),
            )));
            let perm_labels: Vec<&str> = database::permissions::ALL.iter()
                .filter(|&&p| dk.permissions & p != 0)
                .map(|&p| database::permissions::label(p))
                .collect();
            let perm_display = if perm_labels.is_empty() { "None".into() } else { perm_labels.join(", ") };
            parts.push(text(format!(
                "-# {} requests · {} req/5min · {perm_display}",
                format_number(dk.request_count as u64),
                format_number(dk.rate_limit as u64),
            )));
        }
        None => {
            parts.push(text(
                "### Developer Key\n\
                 Need access to extended data or private endpoints?\n\
                 Open a ticket to request a developer key with custom permissions."
            ));
        }
    }

    parts.push(separator());
    let total_tags = BlacklistRepository::new(data.db.pool())
        .count_tags_by_user(member.discord_id)
        .await
        .unwrap_or(0);
    parts.push(text(format!(
        "Added **{}** tags to the blacklist\n\
         **{}** accepted tag reviews · **{}** rejected\n\
         **{}** accurate verdicts",
        total_tags, member.accepted_tags, member.rejected_tags, member.accurate_verdicts,
    )));
    parts.push(separator());
    parts.push(CreateContainerComponent::ActionRow(
        CreateActionRow::buttons(vec![
            CreateButton::new("help_button")
                .label("Help & Setup Guide")
                .style(ButtonStyle::Secondary),
        ]),
    ));

    vec![CreateComponent::Container(CreateContainer::new(parts))]
}
