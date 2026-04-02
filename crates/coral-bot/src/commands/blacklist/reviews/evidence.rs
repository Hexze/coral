use anyhow::Result;
use blacklist::parse_replay;
use serenity::all::*;

use crate::framework::Data;
use super::{*, builder::*, state::*};


pub async fn handle_add_replay(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let (player_idx, submitter_id) = parse_component_ids(&component.data.custom_id);

    let replay_input = CreateInputText::new(InputTextStyle::Short, "replay")
        .placeholder("/replay 9f2fa87d-ed0b-471b-a2e6-cb42777beec8 #9d303f9d")
        .min_length(1)
        .max_length(200);
    let note_input = CreateInputText::new(InputTextStyle::Short, "note")
        .placeholder("Optional note about this replay")
        .required(false)
        .max_length(75);

    let modal = CreateModal::new(
        format!("review_replay_modal:{player_idx}:{submitter_id}"),
        "Add Replay Evidence",
    )
    .components(vec![
        CreateModalComponent::Label(CreateLabel::input_text("Replay Command or ID", replay_input)),
        CreateModalComponent::Label(CreateLabel::input_text("Note (optional)", note_input)),
    ]);

    component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}


pub async fn handle_replay_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    _data: &Data,
) -> Result<()> {
    modal.defer_ephemeral(&ctx.http).await?;

    let custom_id = modal.data.custom_id.strip_prefix("review_replay_modal:").unwrap_or("");
    let player_idx: usize = custom_id.split(':').next().and_then(|s| s.parse().ok()).unwrap_or(0);

    let replay_input = extract_modal_value(modal, "replay");
    let note = extract_modal_value(modal, "note");

    let Some(replay) = parse_replay(&replay_input) else {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Could not parse replay. Provide a valid replay UUID or `/replay` command")).await?;
        return Ok(());
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
    let Some(player) = state.players.get_mut(player_idx) else {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Player not found")).await?;
        return Ok(());
    };

    let duplicate = player.evidence.iter().any(|e| match e {
        Evidence::Replay { replay: r, .. } => r.id == replay.id && r.timestamp == replay.timestamp,
        _ => false,
    });
    if duplicate {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content("This replay has already been added")).await?;
        return Ok(());
    }

    player.evidence.push(Evidence::Replay {
        replay,
        note: if note.is_empty() { None } else { Some(note) },
    });

    update_builder(ctx, channel_id, &builder_msg, &state).await?;
    let _ = modal.delete_response(&ctx.http).await;
    Ok(())
}


pub async fn handle_attach_media(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let (player_idx, submitter_id) = parse_component_ids(&component.data.custom_id);
    let upload = CreateFileUpload::new("evidence")
        .max_values(MAX_MEDIA_PER_PLAYER as u8)
        .required(true);

    let modal = CreateModal::new(
        format!("review_media_modal:{player_idx}:{submitter_id}"),
        "Upload Evidence",
    )
    .components(vec![CreateModalComponent::Label(
        CreateLabel::file_upload("Evidence screenshots or clips", upload),
    )]);

    component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}


pub async fn handle_media_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    _data: &Data,
) -> Result<()> {
    modal.defer_ephemeral(&ctx.http).await?;

    let custom_id = modal.data.custom_id.strip_prefix("review_media_modal:").unwrap_or("");
    let player_idx: usize = custom_id.split(':').next().and_then(|s| s.parse().ok()).unwrap_or(0);

    let upload_ids: Vec<AttachmentId> = modal
        .data
        .components
        .iter()
        .filter_map(|c| match c {
            Component::Label(label) => match &label.component {
                LabelComponent::FileUpload(fu) => Some(fu.values.iter().copied()),
                _ => None,
            },
            _ => None,
        })
        .flatten()
        .collect();

    if upload_ids.is_empty() {
        let _ = modal.delete_response(&ctx.http).await;
        return Ok(());
    }

    modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Downloading files...")).await?;

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

    let existing_count = player.evidence.iter().filter(|e| matches!(e, Evidence::Attachment { .. })).count();
    let remaining = MAX_MEDIA_PER_PLAYER.saturating_sub(existing_count);

    let mut files = Vec::new();
    let mut rejected = 0usize;
    for (i, att_id) in upload_ids.iter().take(remaining).enumerate() {
        let Some(attachment) = modal.data.resolved.attachments.get(att_id) else { continue };
        let ext = attachment.filename.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        if !ALLOWED_MEDIA_EXTENSIONS.contains(&ext.as_str()) {
            rejected += 1;
            continue;
        }
        let filename = format!("{}_{}.{}", player.username, existing_count + i + 1, ext);
        match CreateAttachment::url(&ctx.http, attachment.url.as_str(), filename.clone()).await {
            Ok(file) => {
                files.push(file);
                player.evidence.push(Evidence::Attachment { filename });
            }
            Err(e) => {
                tracing::warn!("Failed to download attachment: {e}");
                rejected += 1;
            }
        }
    }

    if files.is_empty() && rejected > 0 {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Only images and videos are accepted (png, jpg, gif, webp, mp4, webm, mov)")).await?;
        return Ok(());
    }

    modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Uploading evidence...")).await?;

    match update_builder_with_files(ctx, channel_id, &builder_msg, &state, files).await {
        Ok(()) => { let _ = modal.delete_response(&ctx.http).await; }
        Err(e) => {
            let msg = if e.to_string().contains("too large") || e.to_string().contains("413") {
                "File too large. Try compressing or using a smaller file."
            } else {
                "Failed to upload evidence. Please try again."
            };
            modal.edit_response(&ctx.http, EditInteractionResponse::new().content(msg)).await?;
        }
    }
    Ok(())
}


pub async fn handle_edit_evidence(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let (player_idx, _) = parse_component_ids(&component.data.custom_id);
    let message = *component.message.clone();
    let Some(state) = parse_state_from_message(&message) else { return Ok(()) };
    let Some(player) = state.players.get(player_idx) else { return Ok(()) };

    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::EPHEMERAL | MessageFlags::IS_COMPONENTS_V2)
                    .components(build_evidence_panel(player, player_idx, state.submitter_id)),
            ),
        )
        .await?;
    Ok(())
}


pub async fn handle_remove_evidence(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let (player_idx, _) = parse_component_ids(&component.data.custom_id);
    let ev_idx: usize = match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.first().and_then(|v| v.parse().ok()).unwrap_or(0),
        _ => return Ok(()),
    };

    let channel_id = component.channel_id;
    let Some(builder_msg) = find_builder_message(ctx, channel_id).await else { return Ok(()) };
    let Some(mut state) = parse_state_from_message(&builder_msg) else { return Ok(()) };

    if let Some(player) = state.players.get_mut(player_idx) {
        if ev_idx < player.evidence.len() {
            player.evidence.remove(ev_idx);
        }
    }

    let panel = state
        .players
        .get(player_idx)
        .map(|p| build_evidence_panel(p, player_idx, state.submitter_id))
        .unwrap_or_default();

    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::EPHEMERAL | MessageFlags::IS_COMPONENTS_V2)
                    .components(panel),
            ),
        )
        .await?;

    update_builder(ctx, channel_id, &builder_msg, &state).await
}
