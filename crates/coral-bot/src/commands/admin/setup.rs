use anyhow::{Result, anyhow};
use serenity::all::*;

use database::{CacheRepository, GuildConfigRepository, GuildRoleRule, MemberRepository};

use crate::expr;
use crate::framework::Data;
use crate::interact;
use crate::utils::{separator, text};


const AUTOROLE_HELP: &str = "\
## Autorole Config

Autoroles assign Discord roles based on Hypixel stats when a user links their account.

Each rule has a **role** and a **condition**. The role is assigned when the condition is true.

**Fields** — `displayname` · `achievements.bedwars_level` · `stats.Bedwars.<stat>`
**Discord** — `discord.name`
**Blacklist** — `blacklist.sniper` · `blacklist.blatant_cheater` · `blacklist.closet_cheater` · `blacklist.confirmed_cheater` · `blacklist.replays_needed`
**Compare** — `>` · `>=` · `<` · `<=` · `==` · `!=`
**Logic** — `and` · `or` · `not`
**Math** — `+` · `-` · `*` · `/` · `%`
**Conditionals** — `if cond: a, else: b`

### Examples

**Minimum Stars**
```py
achievements.bedwars_level >= 500
```
**FKDR Threshold**
```py
stats.Bedwars.final_kills_bedwars
/ stats.Bedwars.final_deaths_bedwars >= 2.0
```
**Any Cheater Tag**
```py
blacklist.blatant_cheater
or blacklist.closet_cheater
or blacklist.confirmed_cheater
```
-# Any raw Hypixel API field path works.";

const NICKNAME_HEADER: &str = "\
## Display Name Format

Wrap expressions in `{}` to insert dynamic values. Everything else is literal text.

**Fields** — `{displayname}` · `{achievements.bedwars_level}` · `{stats.Bedwars.<stat>}`
**Discord** — `{discord.name}`
**Blacklist** — `{blacklist.tag}` · `{blacklist.sniper}` · `{blacklist.blatant_cheater}` · etc.
**Math** — `{a + b}` · `{a / b}` · `{value : .2f}`
**Conditionals** — `{if cond: a, else: b}`
**Truncation** — `{..expr}` marks an expression as truncatable. When the result exceeds 32 characters, this segment is trimmed to fit.";

const NICKNAME_EXAMPLES: &[(&str, &str)] = &[
    (
        "Minecraft Username + Discord Name",
        "{displayname} | {discord.name}",
    ),
    (
        "Minecraft Username + FKDR",
        "{displayname} [{\n  stats.Bedwars.final_kills_bedwars\n  / stats.Bedwars.final_deaths_bedwars : .1f\n}]",
    ),
    (
        "BedWars Star + Minecraft Username + Discord Name",
        "[{achievements.bedwars_level}{\n  if achievements.bedwars_level < 1100: \"✫\",\n  < 2100: \"✪\",\n  < 3100: \"⚝\",\n  else: \"✥\"\n}] {displayname} | {discord.name}",
    ),
];


pub fn register() -> CreateCommand<'static> {
    CreateCommand::new("setup")
        .description("Configure Coral for this server")
        .default_member_permissions(Permissions::MANAGE_GUILD)
}


pub async fn run(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    let Some(guild_id) = command.guild_id else {
        return interact::send_error(ctx, command, "Error", "This command can only be used in a server.").await;
    };

    command
        .create_response(&ctx.http, CreateInteractionResponse::Defer(
            CreateInteractionResponseMessage::new().flags(MessageFlags::EPHEMERAL),
        ))
        .await?;

    let gid = guild_id.get() as i64;
    let uid = command.user.id.get() as i64;
    let repo = GuildConfigRepository::new(data.db.pool());
    let member_repo = MemberRepository::new(data.db.pool());

    let (config, rules, member) = tokio::try_join!(
        repo.upsert(gid, uid),
        repo.get_role_rules(gid),
        async { member_repo.get_by_discord_id(uid).await },
    )?;

    let preview_ctx = build_preview_from_member(ctx, data, guild_id.get(), command.user.id.get(), member).await;
    let components = build_main_view(&config, &rules, guild_id.get(), preview_ctx.as_ref());

    command
        .edit_response(
            &ctx.http,
            EditInteractionResponse::new()
                .flags(MessageFlags::IS_COMPONENTS_V2)
                .components(components),
        )
        .await?;
    Ok(())
}


fn build_main_view(
    config: &database::GuildConfig,
    rules: &[GuildRoleRule],
    guild_id: u64,
    preview_ctx: Option<&serde_json::Value>,
) -> Vec<CreateComponent<'static>> {
    let nickname = match &config.nickname_template {
        Some(t) => match render_with_context(t, preview_ctx) {
            Some(p) => format!("`{p}`\n```py\n{t}\n```"),
            None => format!("```py\n{t}\n```"),
        },
        None => "Not set".into(),
    };

    let rules_text = if rules.is_empty() {
        "None".into()
    } else {
        rules.iter().map(|r| format!("<@&{}>", r.role_id)).collect::<Vec<_>>().join(" ")
    };

    let linked_role_select = CreateSelectMenu::new(
        format!("setup_link_role_select:{guild_id}"),
        CreateSelectMenuKind::Role {
            default_roles: config.link_role_id.map(|id| vec![RoleId::new(id as u64)].into()),
        },
    )
    .placeholder("Select a linked role");

    let unlinked_role_select = CreateSelectMenu::new(
        format!("setup_unlinked_role_select:{guild_id}"),
        CreateSelectMenuKind::Role {
            default_roles: config.unlinked_role_id.map(|id| vec![RoleId::new(id as u64)].into()),
        },
    )
    .placeholder("Select an unlinked role");

    let channel_select = CreateSelectMenu::new(
        format!("setup_link_channel_select:{guild_id}"),
        CreateSelectMenuKind::Channel {
            channel_types: Some(vec![ChannelType::Text].into()),
            default_channels: config.link_channel_id.map(|id| vec![GenericChannelId::new(id as u64)].into()),
        },
    )
    .placeholder("Select a link channel");

    let parts = vec![
        text("## Server Configuration"),
        separator(),
        text("**Linked Role**"),
        CreateContainerComponent::ActionRow(CreateActionRow::SelectMenu(linked_role_select)),
        text("**Unlinked Role**"),
        CreateContainerComponent::ActionRow(CreateActionRow::SelectMenu(unlinked_role_select)),
        text("**Link Channel**"),
        CreateContainerComponent::ActionRow(CreateActionRow::SelectMenu(channel_select)),
        separator(),
        text(format!("**Display Name Format**\n{nickname}")),
        CreateContainerComponent::ActionRow(CreateActionRow::buttons(vec![
            CreateButton::new(format!("setup_nickname:{guild_id}"))
                .label("Display Name Config").style(ButtonStyle::Secondary),
        ])),
        separator(),
        text(format!("**Autoroles**\n{rules_text}")),
        CreateContainerComponent::ActionRow(CreateActionRow::buttons(vec![
            CreateButton::new(format!("setup_autorole:{guild_id}"))
                .label("Autorole Config").style(ButtonStyle::Secondary),
        ])),
    ];

    vec![CreateComponent::Container(CreateContainer::new(parts))]
}


fn build_autorole_view(
    guild_id: u64,
    extra: Vec<CreateContainerComponent<'static>>,
) -> CreateComponent<'static> {
    let select = CreateSelectMenu::new(
        format!("setup_role_config:{guild_id}"),
        CreateSelectMenuKind::Role { default_roles: None },
    )
    .placeholder("Select a role to configure");

    let mut parts: Vec<CreateContainerComponent> = vec![
        text(AUTOROLE_HELP),
        separator(),
        CreateContainerComponent::ActionRow(CreateActionRow::SelectMenu(select)),
    ];
    parts.extend(extra);
    parts.push(separator());
    parts.push(CreateContainerComponent::ActionRow(
        CreateActionRow::buttons(vec![
            CreateButton::new(format!("setup_autorole_back:{guild_id}"))
                .label("Back").style(ButtonStyle::Secondary),
        ]),
    ));

    CreateComponent::Container(CreateContainer::new(parts))
}


fn build_nickname_help(preview_ctx: Option<&serde_json::Value>) -> String {
    let mut help = NICKNAME_HEADER.to_string();
    help.push_str("\n\n### Examples");
    for (name, tmpl) in NICKNAME_EXAMPLES {
        help.push_str(&format!("\n\n**{name}**"));
        if let Some(p) = render_with_context(tmpl, preview_ctx) {
            help.push_str(&format!("\n`{p}`"));
        }
        help.push_str(&format!("\n```py\n{tmpl}\n```"));
    }
    help.push_str("\n\n-# Any raw Hypixel API field path works. Set empty to clear.");
    help
}


fn build_nickname_panel(
    guild_id: u64,
    template: Option<&str>,
    preview_ctx: Option<&serde_json::Value>,
) -> CreateComponent<'static> {
    let mut parts: Vec<CreateContainerComponent> = vec![text(build_nickname_help(preview_ctx))];

    match template {
        Some(tmpl) => {
            let status = match render_with_context(tmpl, preview_ctx) {
                Some(p) => format!("### **{p}**\n```py\n{tmpl}\n```"),
                None => format!("### Current\n```py\n{tmpl}\n```\n-# *Link your account to see a preview*"),
            };
            parts.push(separator());
            parts.push(text(status));
            parts.push(CreateContainerComponent::ActionRow(
                CreateActionRow::buttons(vec![
                    CreateButton::new(format!("setup_nickname_edit:{guild_id}"))
                        .label("Edit Format").style(ButtonStyle::Primary),
                    CreateButton::new(format!("setup_nickname_clear:{guild_id}"))
                        .label("Clear").style(ButtonStyle::Danger),
                ]),
            ));
        }
        None => {
            parts.push(separator());
            parts.push(text("### No format set"));
            parts.push(CreateContainerComponent::ActionRow(
                CreateActionRow::buttons(vec![
                    CreateButton::new(format!("setup_nickname_edit:{guild_id}"))
                        .label("Set Format").style(ButtonStyle::Primary),
                    CreateButton::new(format!("setup_nickname_reset:{guild_id}"))
                        .label("Reset All Nicknames").style(ButtonStyle::Danger),
                ]),
            ));
        }
    }

    parts.push(separator());
    parts.push(CreateContainerComponent::ActionRow(
        CreateActionRow::buttons(vec![
            CreateButton::new(format!("setup_cancel:{guild_id}"))
                .label("Back").style(ButtonStyle::Secondary),
        ]),
    ));

    CreateComponent::Container(CreateContainer::new(parts))
}


fn build_role_section(
    guild_id: u64,
    role_id: u64,
    rule: Option<&GuildRoleRule>,
) -> Vec<CreateContainerComponent<'static>> {
    let mut parts = vec![separator()];

    match rule {
        Some(rule) => {
            parts.push(text(format!("### <@&{}>\n```py\n{}\n```", role_id, rule.condition)));
            parts.push(CreateContainerComponent::ActionRow(
                CreateActionRow::buttons(vec![
                    CreateButton::new(format!("setup_rule_edit:{}:{}", guild_id, rule.id))
                        .label("Edit Condition").style(ButtonStyle::Primary),
                    CreateButton::new(format!("setup_rule_remove:{}:{}", guild_id, rule.id))
                        .label("Remove").style(ButtonStyle::Danger),
                ]),
            ));
        }
        None => {
            parts.push(text(format!("### <@&{role_id}>")));
            parts.push(CreateContainerComponent::ActionRow(
                CreateActionRow::buttons(vec![
                    CreateButton::new(format!("setup_condition_edit:{guild_id}:{role_id}"))
                        .label("Set Condition").style(ButtonStyle::Primary),
                    CreateButton::new(format!("setup_role_strip:{guild_id}:{role_id}"))
                        .label("Strip Role").style(ButtonStyle::Danger),
                ]),
            ));
        }
    }

    parts
}


fn post_link_embed_container() -> CreateComponent<'static> {
    CreateComponent::Container(CreateContainer::new(vec![
        text(
            "## Account Linking\n\n\
             Link your Minecraft account to get roles and a nickname in this server.\n\n\
             Use the `/link` command or the button below to get started.",
        ),
        separator(),
        CreateContainerComponent::ActionRow(CreateActionRow::buttons(vec![
            CreateButton::new("setup_link").label("Link Account").style(ButtonStyle::Primary),
        ])),
    ]))
}


pub async fn handle_link_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let discord_id = component.user.id.get();
    let repo = MemberRepository::new(data.db.pool());
    if repo.get_by_discord_id(discord_id as i64).await?.is_none() {
        repo.create(discord_id as i64).await?;
    }

    let components = crate::commands::admin::accounts_panel::build_accounts_for_self(
        data, discord_id, &component.user.name,
    )
    .await?;

    component
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


pub async fn handle_cancel_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let guild_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid guild ID"))?;
    refresh_main(ctx, component, data, guild_id).await
}


pub async fn handle_link_role_select(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let guild_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid guild ID"))?;

    let role_id = match &component.data.kind {
        ComponentInteractionDataKind::RoleSelect { values } => values.first().copied(),
        _ => None,
    };

    if let Some(rid) = role_id {
        if !can_manage_role(ctx, GuildId::new(guild_id), component.user.id, rid).await {
            return interact::send_component_error(
                ctx, component, "Error", "You can only select roles below your highest role.",
            )
            .await;
        }
    }

    let repo = GuildConfigRepository::new(data.db.pool());
    let old_role = repo.get(guild_id as i64).await?
        .and_then(|c| c.link_role_id)
        .map(|r| RoleId::new(r as u64));

    repo.set_link_role(guild_id as i64, role_id.map(|r| r.get() as i64)).await?;
    refresh_main(ctx, component, data, guild_id).await?;

    if old_role != role_id {
        spawn_role_swap(ctx, &component.token, data, guild_id, old_role, role_id, crate::sync::RoleConfigField::Link);
    }

    Ok(())
}


pub async fn handle_unlinked_role_select(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let guild_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid guild ID"))?;

    let role_id = match &component.data.kind {
        ComponentInteractionDataKind::RoleSelect { values } => values.first().copied(),
        _ => None,
    };

    if let Some(rid) = role_id {
        if !can_manage_role(ctx, GuildId::new(guild_id), component.user.id, rid).await {
            return interact::send_component_error(
                ctx, component, "Error", "You can only select roles below your highest role.",
            )
            .await;
        }
    }

    let repo = GuildConfigRepository::new(data.db.pool());
    let old_role = repo.get(guild_id as i64).await?
        .and_then(|c| c.unlinked_role_id)
        .map(|r| RoleId::new(r as u64));

    repo.set_unlinked_role(guild_id as i64, role_id.map(|r| r.get() as i64)).await?;
    refresh_main(ctx, component, data, guild_id).await?;

    if old_role != role_id {
        spawn_role_swap(ctx, &component.token, data, guild_id, old_role, role_id, crate::sync::RoleConfigField::Unlinked);
    }

    Ok(())
}


pub async fn handle_link_channel_select(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let guild_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid guild ID"))?;

    let channel_id = match &component.data.kind {
        ComponentInteractionDataKind::ChannelSelect { values } => values.first().copied(),
        _ => None,
    };

    let repo = GuildConfigRepository::new(data.db.pool());
    replace_link_embed(ctx, &repo, guild_id, channel_id).await?;
    refresh_main(ctx, component, data, guild_id).await
}


pub async fn handle_nickname_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let guild_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid guild ID"))?;

    let (config, _) = fetch_config(data, guild_id).await?;
    let preview_ctx = build_preview_context(ctx, data, guild_id, component.user.id.get()).await;
    let panel = build_nickname_panel(guild_id, config.nickname_template.as_deref(), preview_ctx.as_ref());
    interact::update_message(ctx, component, vec![panel]).await
}


pub async fn handle_nickname_edit_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let guild_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid guild ID"))?;

    let current = GuildConfigRepository::new(data.db.pool())
        .get(guild_id as i64)
        .await?
        .and_then(|c| c.nickname_template)
        .unwrap_or_default();

    let input = CreateInputText::new(InputTextStyle::Paragraph, "template")
        .placeholder("[{achievements.bedwars_level}] | {displayname}")
        .required(false)
        .value(current);
    let modal = CreateModal::new(format!("setup_nickname_modal:{guild_id}"), "Set Display Name Format")
        .components(vec![CreateModalComponent::Label(CreateLabel::input_text("Format", input))]);

    component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}


pub async fn handle_nickname_clear_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let guild_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid guild ID"))?;
    GuildConfigRepository::new(data.db.pool()).set_nickname_template(guild_id as i64, None).await?;
    interact::update_message(ctx, component, vec![build_nickname_panel(guild_id, None, None)]).await?;
    spawn_guild_sync(ctx, &component.token, data, guild_id);
    Ok(())
}


pub async fn handle_nickname_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
) -> Result<()> {
    let guild_id = interact::parse_id(&modal.data.custom_id)
        .ok_or_else(|| anyhow!("invalid guild ID"))?;
    let value = interact::extract_modal_value(&modal.data.components, "template");
    let template = if value.is_empty() { None } else { Some(value.as_str()) };

    if let Some(t) = template {
        if let Err(e) = expr::validate_template(t) {
            return interact::send_modal_error(ctx, modal, "Error", &format!("Invalid template: {e}")).await;
        }
    }

    GuildConfigRepository::new(data.db.pool()).set_nickname_template(guild_id as i64, template).await?;

    let (config, _) = fetch_config(data, guild_id).await?;
    let preview_ctx = build_preview_context(ctx, data, guild_id, modal.user.id.get()).await;
    let panel = build_nickname_panel(guild_id, config.nickname_template.as_deref(), preview_ctx.as_ref());
    interact::update_modal(ctx, modal, vec![panel]).await?;
    spawn_guild_sync(ctx, &modal.token, data, guild_id);
    Ok(())
}


pub async fn handle_autorole_button(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    let guild_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid guild ID"))?;
    interact::update_message(ctx, component, vec![build_autorole_view(guild_id, vec![])]).await
}


pub async fn handle_role_config_select(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let guild_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid guild ID"))?;

    let role_id = match &component.data.kind {
        ComponentInteractionDataKind::RoleSelect { values } => values.first().copied(),
        _ => None,
    };
    let Some(role_id) = role_id else { return Ok(()) };

    if !can_manage_role(ctx, GuildId::new(guild_id), component.user.id, role_id).await {
        return interact::send_component_error(
            ctx, component, "Error", "You can only configure roles below your highest role.",
        )
        .await;
    }

    let (_, rules) = fetch_config(data, guild_id).await?;
    let existing = rules.iter().find(|r| r.role_id == role_id.get() as i64);
    let section = build_role_section(guild_id, role_id.get(), existing);
    interact::update_message(ctx, component, vec![build_autorole_view(guild_id, section)]).await
}


pub async fn handle_condition_edit_button(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    let (guild_id, role_id) = interact::parse_compound_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid compound ID"))?;

    let input = CreateInputText::new(InputTextStyle::Paragraph, "condition")
        .placeholder("achievements.bedwars_level >= 500")
        .required(true);
    let modal = CreateModal::new(format!("setup_add_rule_modal:{guild_id}:{role_id}"), "Add Role Rule")
        .components(vec![CreateModalComponent::Label(CreateLabel::input_text("Condition", input))]);

    component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}


pub async fn handle_rule_edit_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let (guild_id, rule_id) = interact::parse_compound_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid compound ID"))?;

    let repo = GuildConfigRepository::new(data.db.pool());
    let rules = repo.get_role_rules(guild_id as i64).await?;
    let current = rules.iter()
        .find(|r| r.id == rule_id as i64)
        .map(|r| r.condition.as_str())
        .unwrap_or("");

    let input = CreateInputText::new(InputTextStyle::Paragraph, "condition")
        .placeholder("achievements.bedwars_level >= 500")
        .required(true)
        .value(current);
    let modal = CreateModal::new(format!("setup_rule_edit_modal:{guild_id}:{rule_id}"), "Edit Rule Condition")
        .components(vec![CreateModalComponent::Label(CreateLabel::input_text("Condition", input))]);

    component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}


pub async fn handle_rule_remove_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let (guild_id, rule_id) = interact::parse_compound_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid compound ID"))?;

    let repo = GuildConfigRepository::new(data.db.pool());
    let rules = repo.get_role_rules(guild_id as i64).await?;
    let role_id = rules.iter().find(|r| r.id == rule_id as i64).map(|r| r.role_id as u64);
    repo.remove_role_rule(rule_id as i64).await?;

    refresh_autorole(ctx, component, data, guild_id, role_id).await
}


pub async fn handle_add_rule_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
) -> Result<()> {
    let (guild_id, role_id) = interact::parse_compound_id(&modal.data.custom_id)
        .ok_or_else(|| anyhow!("invalid compound ID"))?;
    let condition = interact::extract_modal_value(&modal.data.components, "condition");

    if !can_manage_role(ctx, GuildId::new(guild_id), modal.user.id, RoleId::new(role_id)).await {
        return interact::send_modal_error(
            ctx, modal, "Error", "You can only configure roles below your highest role.",
        )
        .await;
    }

    if let Err(e) = expr::validate_condition(&condition) {
        return interact::send_modal_error(ctx, modal, "Error", &format!("Invalid condition: {e}")).await;
    }

    let repo = GuildConfigRepository::new(data.db.pool());
    if repo.get_role_rules(guild_id as i64).await?.iter().any(|r| r.role_id == role_id as i64) {
        return interact::send_modal_error(
            ctx, modal, "Error", "A rule already exists for that role. Edit or remove it first.",
        )
        .await;
    }

    repo.add_role_rule(guild_id as i64, role_id as i64, &condition, 0).await?;
    refresh_autorole_from_modal(ctx, modal, data, guild_id, Some(role_id)).await?;
    spawn_guild_sync(ctx, &modal.token, data, guild_id);
    Ok(())
}


pub async fn handle_rule_edit_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
) -> Result<()> {
    let (guild_id, rule_id) = interact::parse_compound_id(&modal.data.custom_id)
        .ok_or_else(|| anyhow!("invalid compound ID"))?;
    let condition = interact::extract_modal_value(&modal.data.components, "condition");

    if let Err(e) = expr::validate_condition(&condition) {
        return interact::send_modal_error(ctx, modal, "Error", &format!("Invalid condition: {e}")).await;
    }

    let repo = GuildConfigRepository::new(data.db.pool());
    let rules = repo.get_role_rules(guild_id as i64).await?;
    let rule = rules.iter().find(|r| r.id == rule_id as i64);

    if let Some(r) = rule {
        if !can_manage_role(ctx, GuildId::new(guild_id), modal.user.id, RoleId::new(r.role_id as u64)).await {
            return interact::send_modal_error(
                ctx, modal, "Error", "You can only configure roles below your highest role.",
            ).await;
        }
    }

    repo.update_role_rule_condition(rule_id as i64, &condition).await?;

    let role_id = rule.map(|r| r.role_id as u64);
    refresh_autorole_from_modal(ctx, modal, data, guild_id, role_id).await?;
    spawn_guild_sync(ctx, &modal.token, data, guild_id);
    Ok(())
}


pub async fn handle_nickname_reset_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let guild_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid guild ID"))?;

    let (config, _) = fetch_config(data, guild_id).await?;
    let preview_ctx = build_preview_context(ctx, data, guild_id, component.user.id.get()).await;
    let panel = build_nickname_panel(guild_id, config.nickname_template.as_deref(), preview_ctx.as_ref());
    interact::update_message(ctx, component, vec![panel]).await?;

    let ctx_clone = ctx.clone();
    let data_clone = data.clone();
    spawn_bulk_update(ctx, &component.token, data, guild_id, "Clearing Display Names", move |cancel| {
        Box::pin(async move { crate::sync::clear_nicknames(ctx_clone, data_clone, GuildId::new(guild_id), cancel).await })
    });
    Ok(())
}


pub async fn handle_role_strip_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let (guild_id, role_id) = interact::parse_compound_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid compound ID"))?;

    let (_, rules) = fetch_config(data, guild_id).await?;
    let existing = rules.iter().find(|r| r.role_id == role_id as i64);
    let section = build_role_section(guild_id, role_id, existing);
    interact::update_message(ctx, component, vec![build_autorole_view(guild_id, section)]).await?;

    let ctx_clone = ctx.clone();
    let data_clone = data.clone();
    spawn_bulk_update(ctx, &component.token, data, guild_id, &format!("Stripping <@&{role_id}>"), move |cancel| {
        Box::pin(async move { crate::sync::clear_role(ctx_clone, data_clone, GuildId::new(guild_id), RoleId::new(role_id), cancel).await })
    });
    Ok(())
}


async fn refresh_main(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
    guild_id: u64,
) -> Result<()> {
    let (config, rules) = fetch_config(data, guild_id).await?;
    let components = build_main_view(&config, &rules, guild_id, None);
    interact::update_message(ctx, component, components).await
}


async fn refresh_autorole(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
    guild_id: u64,
    selected_role: Option<u64>,
) -> Result<()> {
    let (_, rules) = fetch_config(data, guild_id).await?;
    interact::update_message(ctx, component, build_autorole_components(&rules, guild_id, selected_role)).await
}


async fn refresh_autorole_from_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
    guild_id: u64,
    selected_role: Option<u64>,
) -> Result<()> {
    let (_, rules) = fetch_config(data, guild_id).await?;
    interact::update_modal(ctx, modal, build_autorole_components(&rules, guild_id, selected_role)).await
}


fn build_autorole_components(
    rules: &[GuildRoleRule],
    guild_id: u64,
    selected_role: Option<u64>,
) -> Vec<CreateComponent<'static>> {
    let section = selected_role
        .map(|role_id| {
            let existing = rules.iter().find(|r| r.role_id == role_id as i64);
            build_role_section(guild_id, role_id, existing)
        })
        .unwrap_or_default();
    vec![build_autorole_view(guild_id, section)]
}


async fn replace_link_embed(
    ctx: &Context,
    repo: &GuildConfigRepository<'_>,
    guild_id: u64,
    new_channel: Option<ChannelId>,
) -> Result<()> {
    if let Some(config) = repo.get(guild_id as i64).await? {
        if let (Some(ch_id), Some(msg_id)) = (config.link_channel_id, config.link_message_id) {
            let _ = ctx.http.delete_message(
                ChannelId::new(ch_id as u64).into(),
                MessageId::new(msg_id as u64),
                None,
            )
            .await;
        }
    }

    if let Some(ch_id) = new_channel {
        let msg = ctx.http
            .send_message(
                ch_id.into(),
                Vec::<CreateAttachment>::new(),
                &CreateMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(vec![post_link_embed_container()]),
            )
            .await?;
        repo.set_link_channel(guild_id as i64, Some(ch_id.get() as i64), Some(msg.id.get() as i64)).await?;
    } else {
        repo.set_link_channel(guild_id as i64, None, None).await?;
    }

    Ok(())
}


async fn build_preview_context(
    ctx: &Context,
    data: &Data,
    guild_id: u64,
    user_id: u64,
) -> Option<serde_json::Value> {
    let member = MemberRepository::new(data.db.pool())
        .get_by_discord_id(user_id as i64).await.ok()??;
    build_preview_from_member(ctx, data, guild_id, user_id, Some(member)).await
}


async fn build_preview_from_member(
    ctx: &Context,
    data: &Data,
    guild_id: u64,
    user_id: u64,
    member: Option<database::Member>,
) -> Option<serde_json::Value> {
    let uuid = member?.uuid?;

    let cache_repo = CacheRepository::new(data.db.pool());
    let (hypixel_data, discord_member, tags) = tokio::join!(
        cache_repo.get_latest_snapshot(&uuid),
        GuildId::new(guild_id).member(&ctx.http, UserId::new(user_id)),
        crate::sync::active_tags(data, &uuid),
    );

    let hypixel_data = hypixel_data.ok()??;
    let discord_member = discord_member.ok()?;
    Some(crate::sync::build_template_context(&hypixel_data, &discord_member, &tags))
}


fn render_with_context(template: &str, preview_ctx: Option<&serde_json::Value>) -> Option<String> {
    let rendered = expr::render_template(template, preview_ctx?).to_truncated(crate::sync::NICKNAME_MAX_LEN);
    (!rendered.is_empty()).then_some(rendered)
}


async fn fetch_config(
    data: &Data,
    guild_id: u64,
) -> Result<(database::GuildConfig, Vec<GuildRoleRule>)> {
    let repo = GuildConfigRepository::new(data.db.pool());
    let config = repo.get(guild_id as i64).await?
        .ok_or_else(|| anyhow!("guild config not found for {guild_id}"))?;
    let rules = repo.get_role_rules(guild_id as i64).await?;
    Ok((config, rules))
}





async fn can_manage_role(
    ctx: &Context,
    guild_id: GuildId,
    user_id: UserId,
    target_role: RoleId,
) -> bool {
    let Ok(roles) = guild_id.roles(&ctx.http).await else { return false };
    let Some(target) = roles.get(&target_role) else { return false };

    if let Ok(guild) = guild_id.to_partial_guild(&ctx.http).await {
        if guild.owner_id == user_id {
            return true;
        }
    }

    let Ok(member) = guild_id.member(&ctx.http, user_id).await else { return false };
    let user_highest = member.roles.iter()
        .filter_map(|r| roles.get(r).map(|role| role.position))
        .max()
        .unwrap_or(0);
    target.position < user_highest
}


fn spawn_bulk_update(
    ctx: &Context,
    token: &str,
    data: &Data,
    guild_id: u64,
    heading: &str,
    task: impl FnOnce(crate::sync::CancelToken) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + 'static,
) {
    let cancel = crate::sync::cancel_token();
    data.sync_cancel_tokens.lock().unwrap().insert(GuildId::new(guild_id), cancel.clone());

    let http = ctx.http.clone();
    let token = token.to_string();
    let heading = heading.to_string();

    let start_components = vec![CreateComponent::Container(CreateContainer::new(vec![
        text(format!("### {heading}")),
        text("Updating members. This may take a long time if your server has many members."),
        separator(),
        CreateContainerComponent::ActionRow(CreateActionRow::buttons(vec![
            CreateButton::new(format!("setup_sync_cancel:{guild_id}"))
                .label("Cancel").style(ButtonStyle::Danger),
        ])),
    ]))];

    let done_components = vec![CreateComponent::Container(CreateContainer::new(vec![
        text(format!("### {heading}")),
        text("Done."),
    ]))];

    let cancelled_components = vec![CreateComponent::Container(CreateContainer::new(vec![
        text(format!("### {heading}")),
        text("Cancelled."),
    ]))];

    tokio::spawn(async move {
        let followup = http.create_followup_message(
            &token,
            &CreateInteractionResponseFollowup::new()
                .flags(MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL)
                .components(start_components),
            Vec::<CreateAttachment>::new(),
        ).await;

        task(cancel.clone()).await;

        if let Ok(msg) = followup {
            let final_components = if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                cancelled_components
            } else {
                done_components
            };
            let _ = http.edit_followup_message(
                &token, msg.id,
                &EditInteractionResponse::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(final_components),
                Vec::<CreateAttachment>::new(),
            ).await;
        }
    });
}


pub async fn handle_sync_cancel_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let guild_id = interact::parse_id(&component.data.custom_id)
        .ok_or_else(|| anyhow!("invalid guild ID"))?;

    if let Some(token) = data.sync_cancel_tokens.lock().unwrap().remove(&GuildId::new(guild_id)) {
        token.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    component.create_response(
        &ctx.http,
        CreateInteractionResponse::UpdateMessage(
            CreateInteractionResponseMessage::new()
                .flags(MessageFlags::IS_COMPONENTS_V2)
                .components(vec![CreateComponent::Container(CreateContainer::new(vec![
                    text("Cancelling..."),
                ]))]),
        ),
    ).await?;
    Ok(())
}


fn spawn_guild_sync(ctx: &Context, token: &str, data: &Data, guild_id: u64) {
    let ctx_clone = ctx.clone();
    let data_clone = data.clone();
    spawn_bulk_update(ctx, token, data, guild_id, "Changing Display Names", move |cancel| {
        Box::pin(async move {
            crate::sync::sync_guild_fresh(ctx_clone, data_clone, GuildId::new(guild_id), cancel).await;
        })
    });
}


fn spawn_role_swap(
    ctx: &Context,
    token: &str,
    data: &Data,
    guild_id: u64,
    old_role: Option<RoleId>,
    new_role: Option<RoleId>,
    config_field: crate::sync::RoleConfigField,
) {
    let heading = match (old_role, new_role) {
        (Some(old), Some(new)) => format!("<@&{old}> \u{2192} <@&{new}>"),
        (Some(old), None) => format!("Removing <@&{old}>"),
        (None, Some(new)) => format!("Assigning <@&{new}>"),
        (None, None) => return,
    };

    let ctx_clone = ctx.clone();
    let data_clone = data.clone();
    spawn_bulk_update(ctx, token, data, guild_id, &heading, move |cancel| {
        Box::pin(async move {
            crate::sync::swap_role(ctx_clone.clone(), data_clone.clone(), GuildId::new(guild_id), old_role, new_role, config_field, cancel.clone()).await;
            crate::sync::sync_guild_fresh(ctx_clone, data_clone, GuildId::new(guild_id), cancel).await;
        })
    });
}
