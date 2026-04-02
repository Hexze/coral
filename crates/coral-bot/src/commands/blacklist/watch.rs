use anyhow::Result;
use chrono::{Duration, Utc};
use coral_redis::BlacklistEvent;
use database::TagOp;
use serenity::all::*;

use super::tag::{MemberCheck, require_linked_member};
use crate::framework::{AccessRank, Data};
use crate::interact::send_deferred_error;
use crate::utils::format_uuid_dashed;


const DEFAULT_EXPIRY_DAYS: i64 = 14;


pub fn register() -> CreateCommand<'static> {
    CreateCommand::new("watch")
        .description("Add a replays needed tag to a player")
        .add_option(
            CreateCommandOption::new(CommandOptionType::String, "player", "Player name or UUID")
                .required(true),
        )
        .add_option(
            CreateCommandOption::new(CommandOptionType::Integer, "days", "Days until expiry (0 = permanent, default: 14)")
                .min_int_value(0)
                .max_int_value(365),
        )
}


pub async fn run(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    command.defer_ephemeral(&ctx.http).await?;

    let discord_id = command.user.id.get();
    let (rank, _member) = match require_linked_member(ctx, data, discord_id).await? {
        MemberCheck::Ok(r, m) => (r, m),
        MemberCheck::NotInGuild =>
            return send_deferred_error(ctx, command, "Error", "You must be in the Urchin server to use this command").await,
        MemberCheck::NotLinked =>
            return send_deferred_error(ctx, command, "Error", "You must link your account to use this command").await,
    };

    if rank < AccessRank::Helper {
        return send_deferred_error(ctx, command, "Error", "Only helpers and above can use /watch").await;
    }

    let opts = command.data.options();
    let player = opts.iter().find_map(|o| match (&*o.name, &o.value) {
        ("player", ResolvedValue::String(s)) => Some(*s),
        _ => None,
    }).unwrap_or("");
    let days = opts.iter().find_map(|o| match (&*o.name, &o.value) {
        ("days", ResolvedValue::Integer(n)) => Some(*n),
        _ => None,
    }).unwrap_or(DEFAULT_EXPIRY_DAYS);

    let player_info = match data.api.resolve(player).await {
        Ok(info) => info,
        Err(_) => return send_deferred_error(ctx, command, "Error", "Player not found").await,
    };

    let expires_at = if days == 0 { None } else { Some(Utc::now() + Duration::days(days)) };
    let ops = TagOp::new(data.db.pool());

    let tag = match ops.add(
        &player_info.uuid, "replays_needed", "", discord_id as i64,
        rank.to_level(), false, None, expires_at,
    ).await {
        Ok(tag) => tag,
        Err(database::TagOpError::PlayerLocked) =>
            return send_deferred_error(ctx, command, "Error", "This player's tags are locked").await,
        Err(database::TagOpError::TagAlreadyExists) | Err(database::TagOpError::PriorityConflict(_)) =>
            return send_deferred_error(ctx, command, "Error", "Player already has a replays needed tag").await,
        Err(e) => return Err(anyhow::anyhow!("{e:?}")),
    };

    data.event_publisher.publish(&BlacklistEvent::TagAdded {
        uuid: player_info.uuid.clone(),
        tag_id: tag.id,
        added_by: discord_id as i64,
    }).await;

    let expiry_text = match expires_at {
        Some(ts) => format!("expires <t:{}:R>", ts.timestamp()),
        None => "no expiration".into(),
    };
    let dashed_uuid = format_uuid_dashed(&player_info.uuid);

    let container = CreateContainer::new(vec![
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "## Replays Needed\n`{}` is now being watched ({})\n-# UUID: {dashed_uuid}",
            player_info.username, expiry_text
        ))),
    ])
;

    command.edit_response(
        &ctx.http,
        EditInteractionResponse::new()
            .flags(MessageFlags::IS_COMPONENTS_V2)
            .components(vec![CreateComponent::Container(container)]),
    ).await?;

    Ok(())
}
