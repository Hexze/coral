use anyhow::Result;
use blacklist::{lookup as lookup_tag, EMOTE_ADDTAG, EMOTE_TAG};
use coral_redis::BlacklistEvent;
use database::{BlacklistRepository, MemberRepository};
use serenity::all::*;

use crate::{framework::Data, utils::text};
use super::{*, builder::*, state::*};


pub async fn handle_submit(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let Some(message) = find_builder_message(ctx, component.channel_id).await else { return Ok(()) };
    let Some(mut state) = parse_state_from_message(&message) else {
        return send_vote_error(ctx, component, "Could not parse submission state").await;
    };

    if state.players.is_empty() {
        return send_vote_error(ctx, component, "Add at least one player before submitting").await;
    }
    if !state.players.iter().any(|p| !p.evidence.is_empty()) {
        return send_vote_error(ctx, component, "Add at least one piece of evidence (replay or attachment) before submitting").await;
    }

    state.submitted = true;
    for player in &mut state.players {
        if !player.is_nicked {
            if let Ok(warning) = check_overwrite_conflict(data, &player.uuid, &player.tag_type).await {
                player.conflict_warning = warning;
            }
        }
    }

    component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
    update_builder(ctx, component.channel_id, &message, &state).await?;

    let tags = resolve_forum_tags(ctx, data).await;
    let mut tag_ids = Vec::new();
    if let Some(id) = tags.pending { tag_ids.push(id); }
    if state.players.iter().any(|p| p.is_nicked) {
        if let Some(id) = tags.nicked { tag_ids.push(id); }
    }
    let _ = set_forum_tags(ctx, component.channel_id.expect_thread(), &tag_ids).await;
    Ok(())
}


pub async fn handle_approve(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let (player_index, submitter_id) = parse_component_ids(&component.data.custom_id);
    let discord_id = component.user.id.get();
    let rank = super::super::tag::get_rank(data, discord_id).await?;

    if rank < crate::framework::AccessRank::Member {
        return send_vote_error(ctx, component, "Only members and above can review submissions").await;
    }
    if discord_id == submitter_id {
        return send_vote_error(ctx, component, "You cannot review your own submission").await;
    }

    let Some(message) = find_builder_message(ctx, component.channel_id).await else { return Ok(()) };
    let Some(mut state) = parse_state_from_message(&message) else { return Ok(()) };
    let Some(player) = state.players.get(player_index) else { return Ok(()) };

    if player.status != PlayerStatus::Pending {
        return send_vote_error(ctx, component, "This player has already been reviewed").await;
    }
    if player.accept_votes.contains(&discord_id) || player.reject_votes.contains(&discord_id) {
        return send_vote_error(ctx, component, "You have already voted on this player").await;
    }

    let is_staff = rank >= crate::framework::AccessRank::Helper;

    if !is_staff {
        state.players[player_index].accept_votes.push(discord_id);
        let unanimous = state.players[player_index].reject_votes.is_empty()
            && state.players[player_index].accept_votes.len() >= 3;

        if !unanimous {
            let player = &state.players[player_index];
            let vote_msg = build_vote_message(
                discord_id, "accept", &player.tag_type, &player.username,
                false, player.accept_votes.len(), player.reject_votes.len(),
            );
            component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
            update_builder(ctx, component.channel_id, &message, &state).await?;
            let _ = ctx.http.send_message(component.channel_id.into(), Vec::<CreateAttachment>::new(), &vote_msg).await;
            return Ok(());
        }
    }

    let player = &state.players[player_index];
    let player_uuid = player.uuid.clone();
    let player_tag_type = player.tag_type.clone();
    let player_username = player.username.clone();
    let player_reason = player.reason.clone();
    let media_urls = extract_media_urls_from_message(&message, player_index);

    if !super::REVIEW_TAGS.contains(&player_tag_type.as_str()) {
        return send_vote_error(ctx, component, "Invalid tag type in submission").await;
    }

    let repo = BlacklistRepository::new(data.db.pool());
    let reviewed_by: Vec<i64> = if is_staff {
        vec![discord_id as i64]
    } else {
        state.players[player_index].accept_votes.iter().map(|&id| id as i64).collect()
    };

    if !player.is_nicked {
        let existing_tags = repo.get_tags(&player_uuid).await?;
        let new_priority = lookup_tag(&player_tag_type).map(|d| d.priority).unwrap_or(0);
        if let Some(conflict) = existing_tags
            .iter()
            .find(|t| lookup_tag(&t.tag_type).map(|d| d.priority).unwrap_or(0) == new_priority)
        {
            repo.remove_tag(conflict.id, discord_id as i64).await?;
        }

        let reviewed_by_slice = if reviewed_by.is_empty() { None } else { Some(reviewed_by.as_slice()) };
        let will_confirm = !media_urls.is_empty() && CONFIRMABLE_TAGS.contains(&player_tag_type.as_str());
        let stored_type = if will_confirm { "confirmed_cheater" } else { &player_tag_type };

        let tag_id = repo
            .add_tag(&player_uuid, stored_type, &player_reason, submitter_id as i64, false, reviewed_by_slice)
            .await?;

        if will_confirm {
            let guild_id = component.guild_id.map(|g| g.get()).unwrap_or(0);
            let review_thread_url = format!(
                "https://discord.com/channels/{}/{}",
                guild_id, component.channel_id.get(),
            );
            if let Err(e) = super::super::evidence::create_evidence_from_review(
                ctx, data, guild_id, &player_uuid, &player_username, &player_tag_type,
                tag_id, &media_urls, Some(&review_thread_url), discord_id as i64,
            ).await {
                tracing::error!("Failed to create evidence post: {e:#}");
            }
        }

        let tags = repo.get_tags(&player_uuid).await?;
        if tags.iter().any(|t| t.id == tag_id) {
            data.event_publisher
                .publish(&BlacklistEvent::TagAdded {
                    uuid: player_uuid.clone(),
                    tag_id,
                    added_by: submitter_id as i64,
                })
                .await;
        }
    }

    let member_repo = MemberRepository::new(data.db.pool());
    let _ = member_repo.increment_accepted_tags(submitter_id as i64).await;

    let accurate_ids: Vec<i64> = state.players[player_index].accept_votes.iter().map(|&id| id as i64).collect();
    if !accurate_ids.is_empty() {
        let _ = member_repo.increment_accurate_verdicts(&accurate_ids).await;
    }

    state.players[player_index].status = PlayerStatus::Approved;
    state.players[player_index].reviewer = None;
    state.players[player_index].accept_votes.clear();
    state.players[player_index].reject_votes.clear();

    component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
    update_builder(ctx, component.channel_id, &message, &state).await?;

    let channel_id = component.channel_id;
    let msg = build_vote_message(
        discord_id, "accept", &player_tag_type, &player_username,
        is_staff, if is_staff { 1 } else { 3 }, 0,
    );
    let _ = ctx.http.send_message(channel_id.into(), Vec::<CreateAttachment>::new(), &msg).await;

    check_all_resolved(ctx, data, component.channel_id.expect_thread(), &state).await
}


pub async fn handle_reject(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let (player_index, submitter_id) = parse_component_ids(&component.data.custom_id);
    let discord_id = component.user.id.get();
    let rank = super::super::tag::get_rank(data, discord_id).await?;

    if rank < crate::framework::AccessRank::Member {
        return send_vote_error(ctx, component, "Only members and above can review submissions").await;
    }
    if discord_id == submitter_id {
        return send_vote_error(ctx, component, "You cannot review your own submission").await;
    }

    let is_staff = rank >= crate::framework::AccessRank::Helper;
    if is_staff {
        let reason_input = CreateInputText::new(InputTextStyle::Short, "reason")
            .placeholder("Why is this submission being rejected?")
            .min_length(1)
            .max_length(30);

        let modal = CreateModal::new(
            format!("review_reject_modal:{player_index}:{submitter_id}"),
            "Reject Submission",
        )
        .components(vec![CreateModalComponent::Label(
            CreateLabel::input_text("Rejection Reason", reason_input),
        )]);

        component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
        return Ok(());
    }

    let Some(message) = find_builder_message(ctx, component.channel_id).await else { return Ok(()) };
    let Some(mut state) = parse_state_from_message(&message) else { return Ok(()) };
    let Some(player) = state.players.get(player_index) else { return Ok(()) };

    if player.status != PlayerStatus::Pending {
        return send_vote_error(ctx, component, "This player has already been reviewed").await;
    }
    if player.accept_votes.contains(&discord_id) || player.reject_votes.contains(&discord_id) {
        return send_vote_error(ctx, component, "You have already voted on this player").await;
    }

    state.players[player_index].reject_votes.push(discord_id);
    let unanimous = state.players[player_index].accept_votes.is_empty()
        && state.players[player_index].reject_votes.len() >= 3;

    if !unanimous {
        let player = &state.players[player_index];
        let vote_msg = build_vote_message(
            discord_id, "reject", &player.tag_type, &player.username,
            false, player.accept_votes.len(), player.reject_votes.len(),
        );
        component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
        update_builder(ctx, component.channel_id, &message, &state).await?;
        let _ = ctx.http.send_message(component.channel_id.into(), Vec::<CreateAttachment>::new(), &vote_msg).await;
        return Ok(());
    }

    let player_tag_type = state.players[player_index].tag_type.clone();
    let player_username = state.players[player_index].username.clone();

    let member_repo = MemberRepository::new(data.db.pool());
    let _ = member_repo.increment_rejected_tags(submitter_id as i64).await;

    let accurate_ids: Vec<i64> = state.players[player_index].reject_votes.iter().map(|&id| id as i64).collect();
    if !accurate_ids.is_empty() {
        let _ = member_repo.increment_accurate_verdicts(&accurate_ids).await;
    }

    let vote_msg = build_vote_message(discord_id, "reject", &player_tag_type, &player_username, false, 0, 3);

    state.players[player_index].status = PlayerStatus::Rejected;
    state.players[player_index].reviewer = None;
    state.players[player_index].accept_votes.clear();
    state.players[player_index].reject_votes.clear();

    component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
    update_builder(ctx, component.channel_id, &message, &state).await?;
    let _ = ctx.http.send_message(component.channel_id.into(), Vec::<CreateAttachment>::new(), &vote_msg).await;

    check_all_resolved(ctx, data, component.channel_id.expect_thread(), &state).await
}


pub async fn handle_reject_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
) -> Result<()> {
    let custom_id = modal.data.custom_id.strip_prefix("review_reject_modal:").unwrap_or("");
    let parts: Vec<&str> = custom_id.split(':').collect();
    if parts.len() < 2 { return Ok(()); }

    let player_index: usize = parts[0].parse().unwrap_or(0);
    let submitter_id: u64 = parts[1].parse().unwrap_or(0);
    let reason = extract_modal_value(modal, "reason");
    let discord_id = modal.user.id.get();

    modal.defer_ephemeral(&ctx.http).await?;

    let channel_id = modal.channel_id;
    let Some(builder_msg) = find_builder_message(ctx, channel_id).await else { return Ok(()) };
    let Some(mut state) = parse_state_from_message(&builder_msg) else { return Ok(()) };
    let Some(player) = state.players.get(player_index) else { return Ok(()) };

    if player.status != PlayerStatus::Pending {
        modal.edit_response(&ctx.http, EditInteractionResponse::new().content("This player has already been reviewed")).await?;
        return Ok(());
    }

    let player_username = player.username.clone();
    let player_tag_type = player.tag_type.clone();

    let member_repo = MemberRepository::new(data.db.pool());
    let _ = member_repo.increment_rejected_tags(submitter_id as i64).await;

    let accurate_ids: Vec<i64> = state.players[player_index].reject_votes.iter().map(|&id| id as i64).collect();
    if !accurate_ids.is_empty() {
        let _ = member_repo.increment_accurate_verdicts(&accurate_ids).await;
    }

    state.players[player_index].status = PlayerStatus::Rejected;
    state.players[player_index].reviewer = None;
    state.players[player_index].review_note = Some(reason.clone());
    state.players[player_index].accept_votes.clear();
    state.players[player_index].reject_votes.clear();

    update_builder(ctx, channel_id, &builder_msg, &state).await?;

    let vote_msg = build_vote_message(discord_id, "reject", &player_tag_type, &player_username, true, 0, 1);
    let _ = ctx.http.send_message(channel_id.into(), Vec::<CreateAttachment>::new(), &vote_msg).await;

    modal.edit_response(&ctx.http, EditInteractionResponse::new().content("Rejected")).await?;

    check_all_resolved(ctx, data, modal.channel_id.expect_thread(), &state).await
}


async fn check_all_resolved(
    ctx: &Context,
    data: &Data,
    thread_id: ThreadId,
    state: &SubmissionState,
) -> Result<()> {
    if !state.players.iter().all(|p| p.status != PlayerStatus::Pending) {
        return Ok(());
    }

    let all_approved = state.players.iter().all(|p| p.status == PlayerStatus::Approved);
    let all_rejected = state.players.iter().all(|p| p.status == PlayerStatus::Rejected);

    let tags = resolve_forum_tags(ctx, data).await;
    let mut tag_ids = Vec::new();

    if all_approved {
        if let Some(id) = tags.approved { tag_ids.push(id); }
    } else if all_rejected {
        if let Some(id) = tags.rejected { tag_ids.push(id); }
    } else if let Some(id) = tags.pending {
        tag_ids.push(id);
    }

    if state.players.iter().any(|p| p.is_nicked) {
        if let Some(id) = tags.nicked { tag_ids.push(id); }
    }

    let _ = set_forum_tags(ctx, thread_id, &tag_ids).await;

    let channel_id: GenericChannelId = thread_id.into();
    let mut summary = format!("<@{}> All players have been reviewed:\n", state.submitter_id);
    for player in &state.players {
        let emote = lookup_tag(&player.tag_type).map(|d| d.emote).unwrap_or("");
        let verdict = match player.status {
            PlayerStatus::Approved => "approved",
            PlayerStatus::Rejected => "rejected",
            PlayerStatus::Pending => "pending",
        };
        summary.push_str(&format!("- {emote} `{}` \u{2014} **{verdict}**\n", player.username));
    }

    let _ = super::send_thread_message(ctx, channel_id, &summary).await;

    let http = ctx.http.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;
        let _ = thread_id.edit(&http, EditThread::new().archived(true).locked(true)).await;
    });

    Ok(())
}


pub async fn handle_confirm(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let message = component.message.clone();
    let Some(conf) = parse_confirmation_data(&component.data.custom_id, &message) else {
        return send_vote_error(ctx, component, "Could not parse confirmation data").await;
    };

    let submitter_id = component.user.id.get();
    match create_submission(
        ctx, data, submitter_id, &conf.player_name, &conf.player_uuid,
        &conf.tag_type, &conf.reason, conf.is_nicked,
    ).await {
        Ok(thread_id) => {
            spawn_submission_timeout(ctx.clone(), thread_id);
            component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .flags(MessageFlags::IS_COMPONENTS_V2)
                            .components(vec![CreateComponent::Container(CreateContainer::new(vec![text(format!(
                                "## {} Tag Review Created\nYour submission has been created in <#{}>.\nAdd evidence and submit for mod review.",
                                EMOTE_ADDTAG, thread_id
                            ))]))]),
                    ),
                )
                .await?;
        }
        Err(e) => {
            tracing::error!("Failed to create review submission: {}", e);
            component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::UpdateMessage(
                        CreateInteractionResponseMessage::new()
                            .flags(MessageFlags::IS_COMPONENTS_V2)
                            .components(vec![CreateComponent::Container(CreateContainer::new(
                                vec![text("## Error\nFailed to create review submission")],
                            ))]),
                    ),
                )
                .await?;
        }
    }
    Ok(())
}


pub async fn handle_cancel(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(vec![CreateComponent::Container(CreateContainer::new(vec![text(format!(
                        "## {} Submission Cancelled", EMOTE_TAG
                    ))]))]),
            ),
        )
        .await?;
    Ok(())
}


pub async fn handle_cancel_thread(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let submitter_id = component.data.custom_id.split(':').last().unwrap_or("0");
    component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;

    let channel_id: GenericChannelId = component.channel_id.into();
    let delete_msg = CreateMessage::new()
        .content("Deleting post in 30 seconds.")
        .components(vec![CreateComponent::ActionRow(CreateActionRow::Buttons(
            vec![CreateButton::new(format!("review_abort_delete:{submitter_id}"))
                .label("Cancel")
                .style(ButtonStyle::Secondary)]
            .into(),
        ))]);

    let sent = ctx.http.send_message(channel_id, Vec::<CreateAttachment>::new(), &delete_msg).await?;
    let http = ctx.http.clone();
    let msg_id = sent.id;

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        let Ok(msg) = http.get_message(channel_id, msg_id).await else { return };
        if msg.content != "Deleting post in 30 seconds." { return; }
        let _ = channel_id.delete(&http, None).await;
    });

    Ok(())
}


pub async fn handle_abort_delete(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    if !require_submitter(ctx, component).await? { return Ok(()); }

    let channel_id: GenericChannelId = component.channel_id.into();
    let _ = ctx.http.delete_message(channel_id, component.message.id, None).await;
    component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await.ok();
    Ok(())
}
