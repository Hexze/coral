use anyhow::Result;
use serenity::all::{
    ButtonStyle, CommandInteraction, ComponentInteraction, Context, CreateActionRow, CreateButton,
    CreateCommand, CreateComponent, CreateContainer, CreateContainerComponent,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateSection,
    CreateSectionAccessory, MessageFlags,
};

use database::{BlacklistRepository, MemberRepository};

use crate::framework::{AccessRank, Data};
use crate::interact::{self, section_text};
use crate::utils::{format_number, generate_api_key, resolve_username, separator, text};

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
            repo.get_by_discord_id(discord_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("failed to retrieve member after creation"))?
        }
    };

    if member.api_key.is_none() {
        let key = generate_api_key();
        repo.set_api_key(discord_id, &key).await?;
        member.api_key = Some(key);
    }

    let components = build_dashboard_view(&member, data).await;

    command
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL)
                    .components(components),
            ),
        )
        .await?;

    Ok(())
}

pub async fn handle_regenerate_key(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    let view = vec![CreateComponent::Container(CreateContainer::new(vec![
        text("## Regenerate API Key"),
        separator(),
        text("Are you sure? Your previous key will stop working."),
        CreateContainerComponent::ActionRow(CreateActionRow::buttons(vec![
            CreateButton::new("confirm_regenerate_key")
                .label("Confirm")
                .style(ButtonStyle::Danger),
        ])),
    ]))];

    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(view),
            ),
        )
        .await?;

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
        return interact::send_component_error(ctx, component, "Error", "Your API key is locked.")
            .await;
    }

    let new_key = generate_api_key();
    repo.set_api_key(discord_id, &new_key).await?;
    member.api_key = Some(new_key);

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

    // --- Account ---

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

    // --- API Key ---

    parts.push(separator());

    let api_key_text = if member.key_locked {
        "### API Key\nLocked".into()
    } else if let Some(key) = &member.api_key {
        format!("### API Key\n||`{key}`||")
    } else {
        "### API Key\nNone".into()
    };

    parts.push(CreateContainerComponent::Section(CreateSection::new(
        vec![section_text(&api_key_text)],
        CreateSectionAccessory::Button(
            CreateButton::new("regenerate_key")
                .label("Regenerate")
                .style(ButtonStyle::Secondary)
                .disabled(member.key_locked),
        ),
    )));

    parts.push(text(format!(
        "-# {} requests",
        format_number(member.request_count as u64)
    )));

    parts.push(separator());

    let blacklist_repo = BlacklistRepository::new(data.db.pool());
    let total_tags = blacklist_repo
        .count_tags_by_user(member.discord_id)
        .await
        .unwrap_or(0);

    parts.push(text(format!(
        "### {}\n-# Registered since <t:{}:D>",
        rank.label(),
        member.join_date.timestamp()
    )));

    parts.push(separator());

    parts.push(text(format!(
        "Added **{}** tags to the blacklist\n\
         **{}** accepted tag reviews · **{}** rejected\n\
         **{}** accurate verdicts",
        total_tags, member.accepted_tags, member.rejected_tags, member.accurate_verdicts,
    )));

    vec![CreateComponent::Container(CreateContainer::new(parts))]
}
