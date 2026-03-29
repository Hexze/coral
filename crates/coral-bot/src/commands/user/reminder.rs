use anyhow::{Result, bail};
use chrono::{Duration, Utc};
use serenity::all::{
    CommandDataOptionValue, CommandInteraction, CommandOptionType, ComponentInteraction, Context,
    CreateActionRow, CreateButton, CreateCommand, CreateCommandOption, CreateComponent,
    CreateContainer, CreateContainerComponent, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, CreateSelectMenu, CreateSelectMenuKind,
    CreateSelectMenuOption, MessageFlags, UserId,
};

use database::ReminderRepository;

use crate::events::wake_poller;
use crate::framework::Data;
use crate::utils::{separator, text};

const SNOOZE_SECONDS: i64 = 600; // 10 minutes

pub fn register() -> CreateCommand<'static> {
    CreateCommand::new("reminder")
        .description("Set a reminder")
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "set", "Set a new reminder")
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "time", "When to remind (e.g. '30s', '5m', '2h', '1d', '1w', '1h30m')")
                        .required(true),
                )
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "message", "Reminder message")
                        .required(true),
                )
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::Boolean, "repeating", "Repeat this reminder on the same interval")
                        .required(false),
                ),
        )
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "list", "List your active reminders"),
        )
        .add_option(
            CreateCommandOption::new(
                CommandOptionType::SubCommand,
                "cancel",
                "Cancel a reminder",
            ),
        )
}

pub async fn run(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    let sub = command.data.options.first().map(|o| o.name.as_str());

    match sub {
        Some("set") => handle_set(ctx, command, data).await,
        Some("list") => handle_list(ctx, command, data).await,
        Some("cancel") => handle_cancel(ctx, command, data).await,
        _ => Ok(()),
    }
}

async fn handle_set(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    let sub_options = command
        .data
        .options
        .first()
        .and_then(|o| match &o.value {
            CommandDataOptionValue::SubCommand(opts) => Some(opts),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("Missing subcommand options"))?;

    let time_str = sub_options
        .iter()
        .find(|o| o.name == "time")
        .and_then(|o| o.value.as_str())
        .unwrap_or("");

    let message = sub_options
        .iter()
        .find(|o| o.name == "message")
        .and_then(|o| o.value.as_str())
        .unwrap_or("Reminder!");

    let repeating = sub_options
        .iter()
        .find(|o| o.name == "repeating")
        .and_then(|o| match &o.value {
            CommandDataOptionValue::Boolean(b) => Some(*b),
            _ => None,
        })
        .unwrap_or(false);

    let duration = match parse_duration(time_str) {
        Some(d) if d.num_seconds() < 10 => {
            return send_reply(ctx, command, "Reminder must be at least 10 seconds in the future.", true).await;
        }
        Some(d) if d.num_days() > 365 => {
            return send_reply(ctx, command, "Reminder cannot be more than 1 year in the future.", true).await;
        }
        Some(d) => d,
        None => {
            return send_reply(
                ctx,
                command,
                "Could not parse time. Examples: `30m`, `2h`, `1d`, `1h30m`",
                true,
            )
            .await;
        }
    };

    let remind_at = Utc::now() + duration;
    let repeat_interval = if repeating {
        Some(duration.num_seconds())
    } else {
        None
    };

    let channel_id = command.channel_id.get() as i64;
    let guild_id = command.guild_id.map(|g| g.get() as i64);
    let discord_id = command.user.id.get() as i64;

    let repo = ReminderRepository::new(data.db.pool());
    let reminder = repo
        .create(discord_id, channel_id, guild_id, message, remind_at, repeat_interval)
        .await?;
    wake_poller();

    let timestamp = reminder.remind_at.timestamp();
    let mut details = format!(
        "**Message** — {message}\n\
         **When** — <t:{timestamp}:R> (<t:{timestamp}:f>)"
    );
    if repeating {
        details.push_str(&format!("\n**Repeating** — every {}", format_duration(&duration)));
    }

    let container = CreateComponent::Container(
        CreateContainer::new(vec![
            text("## Reminder Set"),
            separator(),
            text(details),
        ])
,
    );

    command
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

async fn handle_list(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    let discord_id = command.user.id.get() as i64;
    let repo = ReminderRepository::new(data.db.pool());
    let reminders = repo.list_by_user(discord_id).await?;

    if reminders.is_empty() {
        return send_reply(ctx, command, "You have no active reminders.", true).await;
    }

    let mut parts: Vec<CreateContainerComponent> = vec![
        text("## Your Reminders"),
        separator(),
    ];
    for r in &reminders {
        let ts = r.remind_at.timestamp();
        let repeat = if r.repeat_interval.is_some() {
            " · repeating"
        } else {
            ""
        };
        parts.push(text(format!(
            "**#{}** — {}\n-# <t:{ts}:R>{repeat}",
            r.id, r.message
        )));
    }

    let container = CreateComponent::Container(
        CreateContainer::new(parts),
    );

    command
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

async fn handle_cancel(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    let discord_id = command.user.id.get() as i64;
    let repo = ReminderRepository::new(data.db.pool());
    let reminders = repo.list_by_user(discord_id).await?;

    if reminders.is_empty() {
        return send_reply(ctx, command, "You have no active reminders to cancel.", true).await;
    }

    let options: Vec<CreateSelectMenuOption> = reminders
        .iter()
        .take(25)
        .map(|r| {
            let ts = r.remind_at.timestamp();
            let repeat_label = if let Some(secs) = r.repeat_interval {
                format!(" (repeats every {})", format_duration(&Duration::seconds(secs)))
            } else {
                String::new()
            };
            // Label: truncated message, description: time info
            let label = if r.message.len() > 80 {
                format!("{}...", &r.message[..77])
            } else {
                r.message.clone()
            };
            let description = format!("Due <t:{ts}:R>{repeat_label}");
            CreateSelectMenuOption::new(label, r.id.to_string())
                .description(description)
        })
        .collect();

    let mut parts: Vec<CreateContainerComponent> = Vec::new();
    parts.push(text("## Cancel a Reminder\n-# Select a reminder to cancel"));
    parts.push(separator());
    parts.push(CreateContainerComponent::ActionRow(
        CreateActionRow::SelectMenu(
            CreateSelectMenu::new(
                format!("reminder_cancel_select:{discord_id}"),
                CreateSelectMenuKind::String {
                    options: options.into(),
                },
            )
            .placeholder("Select a reminder to cancel"),
        ),
    ));

    let container = CreateComponent::Container(
        CreateContainer::new(parts),
    );

    command
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

pub async fn handle_cancel_select(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    // custom_id: reminder_cancel_select:{discord_id}
    let id = component.data.custom_id.as_str();
    let owner_id: i64 = id
        .strip_prefix("reminder_cancel_select:")
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);

    if component.user.id.get() as i64 != owner_id {
        component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("This isn't your menu!")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    let selected = match &component.data.kind {
        serenity::all::ComponentInteractionDataKind::StringSelect { values } => {
            values.first().and_then(|v| v.parse::<i64>().ok())
        }
        _ => None,
    }
    .ok_or_else(|| anyhow::anyhow!("No selection"))?;

    let repo = ReminderRepository::new(data.db.pool());
    let deleted = repo.delete(selected, owner_id).await?;

    let (title, msg) = if deleted {
        ("Reminder Cancelled", format!("Reminder **#{selected}** has been cancelled."))
    } else {
        ("Not Found", "Reminder not found or already fired.".to_string())
    };

    let container = CreateComponent::Container(
        CreateContainer::new(vec![
            text(format!("## {title}")),
            separator(),
            text(msg),
        ])
,
    );

    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(vec![container]),
            ),
        )
        .await?;

    Ok(())
}

pub async fn handle_snooze(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let id = component.data.custom_id.as_str();
    let parts: Vec<&str> = id.strip_prefix("reminder_snooze:").unwrap_or("").split(':').collect();

    if parts.len() < 4 {
        bail!("Invalid snooze ID");
    }

    let discord_id: i64 = parts[1].parse()?;
    let channel_id: i64 = parts[2].parse()?;
    let guild_id: Option<i64> = {
        let v: i64 = parts[3].parse()?;
        if v == 0 { None } else { Some(v) }
    };

    if component.user.id.get() as i64 != discord_id {
        component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("This isn't your reminder!")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    let original_message = component
        .message
        .components
        .iter()
        .find_map(|c| match c {
            serenity::all::Component::Container(container) => Some(container),
            _ => None,
        })
        .and_then(|container| {
            container.components.iter().find_map(|c| match c {
                serenity::all::ContainerComponent::TextDisplay(td) => td.content.clone(),
                _ => None,
            })
        })
        .and_then(|body| {
            let stripped = body.strip_prefix("## Reminder\n").unwrap_or(&body);
            stripped
                .lines()
                .find(|l| !l.is_empty() && !l.starts_with('[') && !l.starts_with('*'))
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "Reminder!".to_string());

    let repo = ReminderRepository::new(data.db.pool());
    let snoozed = repo
        .snooze_once(discord_id, channel_id, guild_id, &original_message, SNOOZE_SECONDS)
        .await?;

    if snoozed.is_some() {
        wake_poller();
    }

    if snoozed.is_none() {
        component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("This reminder was already snoozed.")
                        .ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    let snooze_ts = (Utc::now() + Duration::seconds(SNOOZE_SECONDS)).timestamp();

    let container = CreateComponent::Container(
        CreateContainer::new(vec![
            text("## Snoozed"),
            separator(),
            text(format!(
                "{original_message}\n-# I'll remind you again <t:{snooze_ts}:R>"
            )),
        ])
,
    );

    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .components(vec![container]),
            ),
        )
        .await?;

    Ok(())
}

/// Send a DM for a due reminder
pub async fn send_reminder_dm(
    ctx: &Context,
    reminder: &database::Reminder,
) -> Result<()> {
    let user_id = UserId::new(reminder.discord_id as u64);

    let channel_link = if let Some(guild_id) = reminder.guild_id {
        format!(
            "[Jump to channel](https://discord.com/channels/{}/{})",
            guild_id, reminder.channel_id
        )
    } else {
        format!(
            "[Jump to channel](https://discord.com/channels/@me/{})",
            reminder.channel_id
        )
    };

    let repeat_note = if reminder.repeat_interval.is_some() {
        " · repeating"
    } else {
        ""
    };

    let guild_id_val = reminder.guild_id.unwrap_or(0);
    let snooze_id = format!(
        "reminder_snooze:{}:{}:{}:{guild_id_val}",
        reminder.id, reminder.discord_id, reminder.channel_id
    );

    let snooze_button = CreateButton::new(snooze_id).label("Snooze 10 min");

    let mut parts: Vec<CreateContainerComponent> = Vec::new();
    parts.push(text("## Reminder"));
    parts.push(separator());
    parts.push(text(format!(
        "{}\n-# {}{repeat_note}",
        reminder.message, channel_link
    )));
    parts.push(CreateContainerComponent::ActionRow(
        CreateActionRow::Buttons(vec![snooze_button].into()),
    ));

    let container = CreateComponent::Container(
        CreateContainer::new(parts),
    );

    user_id
        .dm(
            &ctx.http,
            CreateMessage::new()
                .flags(MessageFlags::IS_COMPONENTS_V2)
                .components(vec![container]),
        )
        .await?;

    Ok(())
}

fn parse_duration(input: &str) -> Option<Duration> {
    let input = input.trim().to_lowercase();

    parse_relative_duration(&input)
}

fn parse_relative_duration(input: &str) -> Option<Duration> {
    let mut total_seconds: i64 = 0;
    let mut current_num = String::new();
    let mut found_any = false;

    for c in input.chars() {
        if c.is_ascii_digit() || c == '.' {
            current_num.push(c);
        } else if !current_num.is_empty() {
            let num: f64 = current_num.parse().ok()?;
            current_num.clear();
            found_any = true;

            match c {
                's' => total_seconds += num as i64,
                'm' => total_seconds += (num * 60.0) as i64,
                'h' => total_seconds += (num * 3600.0) as i64,
                'd' => total_seconds += (num * 86400.0) as i64,
                'w' => total_seconds += (num * 604800.0) as i64,
                _ => return None,
            }
        }
    }

    if !found_any {
        return None;
    }

    if total_seconds <= 0 {
        return None;
    }

    Some(Duration::seconds(total_seconds))
}

fn format_duration(d: &Duration) -> String {
    let total = d.num_seconds();
    let days = total / 86400;
    let hours = (total % 86400) / 3600;
    let minutes = (total % 3600) / 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if minutes > 0 {
        parts.push(format!("{minutes}m"));
    }
    if parts.is_empty() {
        parts.push(format!("{total}s"));
    }
    parts.join(" ")
}

async fn send_reply(
    ctx: &Context,
    command: &CommandInteraction,
    message: &str,
    ephemeral: bool,
) -> Result<()> {
    let mut msg = CreateInteractionResponseMessage::new().content(message);
    if ephemeral {
        msg = msg.ephemeral(true);
    }
    command
        .create_response(&ctx.http, CreateInteractionResponse::Message(msg))
        .await?;
    Ok(())
}
