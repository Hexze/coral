use anyhow::Result;
use serenity::all::{
    ButtonStyle, CommandInteraction, Context, CreateActionRow, CreateButton, CreateCommand,
    CreateContainerComponent, CreateInteractionResponse, CreateInteractionResponseMessage,
    CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, Member, MessageFlags, RoleId,
};

use database::{GuildConfigRepository, MemberRepository};

use crate::commands::admin::accounts_panel;
use crate::framework::Data;
use crate::utils::{separator, text};

pub fn register() -> CreateCommand<'static> {
    CreateCommand::new("link").description("Link or manage your Minecraft account")
}

pub async fn run(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    let discord_id = command.user.id.get();
    let repo = MemberRepository::new(data.db.pool());

    if repo.get_by_discord_id(discord_id as i64).await?.is_none() {
        repo.create(discord_id as i64).await?;
    }

    let components =
        accounts_panel::build_accounts_for_self(data, discord_id, &command.user.name).await?;

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

pub fn build_link_parts(
    matches: &[(String, String)],
    prefix: &str,
    target_id: u64,
) -> Vec<CreateContainerComponent<'static>> {
    let mut parts: Vec<CreateContainerComponent> = Vec::new();

    if !matches.is_empty() {
        parts.push(text(
            "We found accounts that may be yours. Select one to link instantly:",
        ));

        let options: Vec<CreateSelectMenuOption> = matches
            .iter()
            .take(25)
            .map(|(uuid, username)| CreateSelectMenuOption::new(username.clone(), uuid.clone()))
            .collect();

        parts.push(CreateContainerComponent::ActionRow(
            CreateActionRow::SelectMenu(
                CreateSelectMenu::new(
                    format!("{prefix}_link_pick:{target_id}"),
                    CreateSelectMenuKind::String {
                        options: options.into(),
                    },
                )
                .placeholder("Select an account"),
            ),
        ));

        parts.push(separator());
    }

    parts.push(text(
        "**Verification Server**\nJoin `link.urchin.gg` with the Minecraft account you want to link, and then enter the 4 digit code you receive here.",
    ));
    parts.push(CreateContainerComponent::ActionRow(
        CreateActionRow::buttons(vec![
            CreateButton::new(format!("{prefix}_add_code:{target_id}"))
                .label("Enter Code")
                .style(ButtonStyle::Primary),
        ]),
    ));

    parts.push(separator());

    parts.push(text(
        "**Hypixel Verification**\nSet your Discord username in Hypixel's Social Media settings, then enter your IGN.",
    ));
    parts.push(CreateContainerComponent::ActionRow(
        CreateActionRow::buttons(vec![
            CreateButton::new(format!("{prefix}_add_account:{target_id}"))
                .label("Enter Username")
                .style(ButtonStyle::Primary),
        ]),
    ));

    parts
}

pub async fn handle_guild_join(ctx: &Context, new_member: &Member, data: &Data) -> Result<()> {
    let discord_id = new_member.user.id.get() as i64;
    let members = MemberRepository::new(data.db.pool());

    let uuid = match members
        .get_by_discord_id(discord_id)
        .await?
        .and_then(|m| m.uuid)
    {
        Some(uuid) => uuid,
        None => {
            assign_unlinked_role(ctx, data, new_member).await;
            return Ok(());
        }
    };

    let stats = match data.api.get_player_stats(&uuid).await {
        Ok(s) => s,
        Err(_) => return Ok(()),
    };

    let Some(hypixel_data) = stats.hypixel else {
        return Ok(());
    };

    let config_repo = GuildConfigRepository::new(data.db.pool());
    let config = match config_repo.get(new_member.guild_id.get() as i64).await {
        Ok(Some(c)) => c,
        _ => return Ok(()),
    };
    let rules = config_repo
        .get_role_rules(new_member.guild_id.get() as i64)
        .await
        .unwrap_or_default();

    if let Err(e) = crate::sync::sync_member(
        ctx,
        data,
        new_member.guild_id,
        new_member,
        &uuid,
        &config,
        &rules,
        &hypixel_data,
        true,
    )
    .await
    {
        tracing::warn!(
            "Failed to sync joining member {} in {}: {e}",
            new_member.user.id,
            new_member.guild_id
        );
    }

    Ok(())
}

async fn assign_unlinked_role(ctx: &Context, data: &Data, member: &Member) {
    let config = match GuildConfigRepository::new(data.db.pool())
        .get(member.guild_id.get() as i64)
        .await
    {
        Ok(Some(c)) => c,
        _ => return,
    };

    if let Some(role_id) = config.unlinked_role_id {
        let _ = member
            .add_role(&ctx.http, RoleId::new(role_id as u64), None)
            .await;
    }
}
