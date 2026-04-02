use anyhow::Result;
use serenity::all::*;

use crate::framework::Data;
use super::{*, state::*};


pub async fn handle_add_player(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let submitter_id = parse_submitter_id(&component.data.custom_id).unwrap_or(0);
    let input = CreateInputText::new(InputTextStyle::Short, "player")
        .placeholder("Minecraft username")
        .min_length(1)
        .max_length(16);

    let modal = CreateModal::new(format!("review_addplayer_name:{submitter_id}"), "Add Player")
        .components(vec![CreateModalComponent::Label(CreateLabel::input_text("Player Name", input))]);

    component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}


pub async fn handle_addplayer_name_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
) -> Result<()> {
    modal.defer_ephemeral(&ctx.http).await?;

    let player_name = extract_modal_value(modal, "player");
    let (resolved_name, resolved_uuid, is_nicked) = match data.api.resolve(&player_name).await {
        Ok(info) => (info.username, info.uuid, false),
        Err(_) => (player_name, String::new(), true),
    };

    let channel_id = modal.channel_id;
    let Some(builder_msg) = find_builder_message(ctx, channel_id).await else {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Could not find the submission message")).await?;
        return Ok(());
    };
    let Some(mut state) = parse_state_from_message(&builder_msg) else {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Could not parse submission state")).await?;
        return Ok(());
    };

    let already_added = state.players.iter().any(|p| {
        if is_nicked { p.is_nicked && p.username.eq_ignore_ascii_case(&resolved_name) }
        else { p.uuid == resolved_uuid }
    });
    if already_added {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content(format!("`{resolved_name}` is already in this submission"))).await?;
        return Ok(());
    }

    state.pending_add = Some(PendingAdd {
        identifier: if is_nicked { resolved_name.clone() } else { resolved_uuid.clone() },
        username: resolved_name,
        is_nicked,
    });

    update_builder(ctx, channel_id, &builder_msg, &state).await?;
    let _ = modal.delete_response(&ctx.http).await;
    Ok(())
}


pub async fn handle_pending_tag_select(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let tag_type = match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.first().map(|s| s.as_str()).unwrap_or(""),
        _ => return Ok(()),
    };
    if !super::REVIEW_TAGS.contains(&tag_type) { return Ok(()); }

    let custom_id = component.data.custom_id.strip_prefix("review_pending_tag:").unwrap_or("");
    let parts: Vec<&str> = custom_id.rsplitn(3, ':').collect();
    let submitter_id = parts.first().unwrap_or(&"0");
    let nicked = parts.get(1).unwrap_or(&"0");
    let identifier = parts.get(2).unwrap_or(&"");

    let reason_input = CreateInputText::new(InputTextStyle::Short, "reason")
        .placeholder("Reason for this tag")
        .min_length(1)
        .max_length(120);

    let modal = CreateModal::new(
        format!("review_addplayer_reason:{identifier}:{tag_type}:{nicked}:{submitter_id}"),
        "Add Player \u{2014} Reason",
    )
    .components(vec![CreateModalComponent::Label(CreateLabel::input_text("Reason", reason_input))]);

    component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}


pub async fn handle_addplayer_reason_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
) -> Result<()> {
    modal.defer_ephemeral(&ctx.http).await?;

    let custom_id = modal.data.custom_id.strip_prefix("review_addplayer_reason:").unwrap_or("");
    let parts: Vec<&str> = custom_id.rsplitn(4, ':').collect();
    let is_nicked = parts.get(1).map(|s| *s == "1").unwrap_or(false);
    let tag_type = parts.get(2).unwrap_or(&"").to_string();
    let identifier = parts.get(3).unwrap_or(&"").to_string();

    if !super::REVIEW_TAGS.contains(&tag_type.as_str()) {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content(format!("Invalid tag type: `{tag_type}`"))).await?;
        return Ok(());
    }

    let reason = extract_modal_value(modal, "reason");
    let (username, uuid) = if is_nicked {
        (identifier.clone(), String::new())
    } else {
        let name = data.api.resolve(&identifier).await.map(|r| r.username).unwrap_or_else(|_| identifier.clone());
        (name, identifier.clone())
    };

    let channel_id = modal.channel_id;
    let Some(builder_msg) = find_builder_message(ctx, channel_id).await else {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Could not find the submission message")).await?;
        return Ok(());
    };
    let Some(mut state) = parse_state_from_message(&builder_msg) else {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Could not parse submission state")).await?;
        return Ok(());
    };

    state.players.push(PlayerEntry {
        username, uuid, tag_type, reason, is_nicked,
        status: PlayerStatus::Pending,
        reviewer: None, review_note: None,
        evidence: Vec::new(), conflict_warning: None,
        accept_votes: Vec::new(), reject_votes: Vec::new(),
    });

    update_builder(ctx, channel_id, &builder_msg, &state).await?;

    if is_nicked {
        let tags = resolve_forum_tags(ctx, data).await;
        if let Some(tag_id) = tags.nicked {
            let mut current_tags = Vec::new();
            if let Some(aw) = tags.awaiting_evidence { current_tags.push(aw); }
            current_tags.push(tag_id);
            let _ = set_forum_tags(ctx, modal.channel_id.expect_thread(), &current_tags).await;
        }
    }

    let _ = modal.delete_response(&ctx.http).await;
    Ok(())
}


pub async fn handle_remove_player(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let (player_idx, _) = parse_component_ids(&component.data.custom_id);
    let channel_id = component.channel_id;

    let Some(builder_msg) = find_builder_message(ctx, channel_id).await else { return Ok(()) };
    let Some(mut state) = parse_state_from_message(&builder_msg) else { return Ok(()) };

    if player_idx < state.players.len() {
        state.players.remove(player_idx);
    }

    component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
    update_builder(ctx, channel_id, &builder_msg, &state).await
}


pub async fn handle_edit_tag(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let (player_idx, _) = parse_component_ids(&component.data.custom_id);
    let Some(message) = find_builder_message(ctx, component.channel_id).await else { return Ok(()) };
    let Some(mut state) = parse_state_from_message(&message) else { return Ok(()) };

    state.editing = Some(player_idx);
    component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
    update_builder(ctx, component.channel_id, &message, &state).await
}


pub async fn handle_edit_done(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let Some(message) = find_builder_message(ctx, component.channel_id).await else { return Ok(()) };
    let Some(state) = parse_state_from_message(&message) else { return Ok(()) };

    component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
    update_builder(ctx, component.channel_id, &message, &state).await
}


pub async fn handle_tag_select_edit(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let tag_type = match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.first().map(|s| s.as_str()).unwrap_or(""),
        _ => return Ok(()),
    };
    if !super::REVIEW_TAGS.contains(&tag_type) { return Ok(()); }

    let (player_idx, submitter_id) = parse_component_ids(&component.data.custom_id);
    let message = *component.message.clone();
    let current_reason = parse_state_from_message(&message)
        .and_then(|s| s.players.get(player_idx).map(|p| p.reason.clone()))
        .unwrap_or_default();

    let reason_input = CreateInputText::new(InputTextStyle::Short, "reason")
        .placeholder("Reason for this tag")
        .value(current_reason)
        .min_length(1)
        .max_length(120);

    let modal = CreateModal::new(
        format!("review_edit_player_modal:{player_idx}:{tag_type}:{submitter_id}"),
        "Edit Player",
    )
    .components(vec![CreateModalComponent::Label(CreateLabel::input_text("Reason", reason_input))]);

    component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}


pub async fn handle_edit_player_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    _data: &Data,
) -> Result<()> {
    modal.defer_ephemeral(&ctx.http).await?;

    let custom_id = modal.data.custom_id.strip_prefix("review_edit_player_modal:").unwrap_or("");
    let parts: Vec<&str> = custom_id.split(':').collect();
    let player_idx: usize = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let tag_type = parts.get(1).unwrap_or(&"").to_string();

    if !super::REVIEW_TAGS.contains(&tag_type.as_str()) {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content(format!("Invalid tag type: `{tag_type}`"))).await?;
        return Ok(());
    }

    let reason = extract_modal_value(modal, "reason");
    let channel_id = modal.channel_id;

    let Some(builder_msg) = find_builder_message(ctx, channel_id).await else {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Could not find the submission message")).await?;
        return Ok(());
    };
    let Some(mut state) = parse_state_from_message(&builder_msg) else {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Could not parse submission state")).await?;
        return Ok(());
    };
    let Some(player) = state.players.get_mut(player_idx) else {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Player not found")).await?;
        return Ok(());
    };

    player.tag_type = tag_type;
    player.reason = reason;

    update_builder(ctx, channel_id, &builder_msg, &state).await?;
    let _ = modal.delete_response(&ctx.http).await;
    Ok(())
}


pub async fn handle_edit_submitted(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let Some(message) = find_builder_message(ctx, component.channel_id).await else { return Ok(()) };
    let Some(mut state) = parse_state_from_message(&message) else { return Ok(()) };

    state.submitted = false;
    component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
    update_builder(ctx, component.channel_id, &message, &state).await
}
