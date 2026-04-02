mod builder;
mod compose;
mod evidence;
mod state;
mod verdict;

use std::collections::HashMap;

use anyhow::Result;
use blacklist::lookup as lookup_tag;
use database::BlacklistRepository;
use serenity::all::*;

use crate::framework::Data;
use crate::utils::sanitize_reason;

pub use builder::build_confirmation_message;
pub use compose::{
    handle_add_player, handle_addplayer_name_modal, handle_addplayer_reason_modal,
    handle_edit_done, handle_edit_player_modal, handle_edit_submitted, handle_edit_tag,
    handle_pending_tag_select, handle_remove_player, handle_tag_select_edit,
};
pub use evidence::{
    handle_add_replay, handle_attach_media, handle_edit_evidence, handle_media_modal,
    handle_remove_evidence, handle_replay_modal,
};
pub use verdict::{
    handle_abort_delete, handle_approve, handle_cancel, handle_cancel_thread, handle_confirm,
    handle_reject, handle_reject_modal, handle_submit,
};

use state::*;


const TAG_PENDING: &str = "Pending";
const TAG_APPROVED: &str = "Approved";
const TAG_REJECTED: &str = "Rejected";
const TAG_NICKED: &str = "Nicked";
const TAG_AWAITING_EVIDENCE: &str = "Awaiting Evidence";

const MAX_MEDIA_PER_PLAYER: usize = 4;
const ALLOWED_MEDIA_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "mp4", "webm", "mov",
];
const REVIEW_TAGS: &[&str] = &["closet_cheater", "blatant_cheater"];
const CONFIRMABLE_TAGS: &[&str] = &["closet_cheater", "blatant_cheater"];
const SUBMISSION_TIMEOUT_SECS: u64 = 30 * 60;
const SUBMISSION_WARNING_SECS: u64 = 20 * 60;


fn build_tag_select_options(selected: Option<&str>) -> Vec<CreateSelectMenuOption<'static>> {
    blacklist::all()
        .iter()
        .filter(|def| REVIEW_TAGS.contains(&def.name))
        .map(|def| {
            let mut opt = CreateSelectMenuOption::new(def.display_name, def.name);
            if selected == Some(def.name) {
                opt = opt.default_selection(true);
            }
            opt
        })
        .collect()
}


fn extract_modal_value(modal: &ModalInteraction, field_id: &str) -> String {
    crate::interact::extract_modal_value(&modal.data.components, field_id)
}


fn extract_text_displays(message: &Message) -> Vec<String> {
    let Some(container) = message.components.iter().find_map(|c| match c {
        Component::Container(c) => Some(c),
        _ => None,
    }) else {
        return Vec::new();
    };

    container
        .components
        .iter()
        .filter_map(|c| match c {
            ContainerComponent::TextDisplay(td) => td.content.clone(),
            _ => None,
        })
        .collect()
}


fn find_container(message: &Message) -> Option<&serenity::all::Container> {
    message.components.iter().find_map(|c| match c {
        Component::Container(c) => Some(c),
        _ => None,
    })
}


fn parse_component_ids(custom_id: &str) -> (usize, u64) {
    let mut parts = custom_id.split(':');
    let _ = parts.next();
    let player_idx = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let submitter_id = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (player_idx, submitter_id)
}


fn parse_submitter_id(custom_id: &str) -> Option<u64> {
    custom_id.split(':').last()?.parse().ok()
}


fn is_submitter(component: &ComponentInteraction) -> bool {
    parse_submitter_id(&component.data.custom_id).unwrap_or(0) == component.user.id.get()
}


async fn require_submitter(ctx: &Context, component: &ComponentInteraction) -> Result<bool> {
    if is_submitter(component) { return Ok(true); }
    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .content("Only the submission creator can use these buttons")
                    .ephemeral(true),
            ),
        )
        .await?;
    Ok(false)
}


async fn send_vote_error(ctx: &Context, component: &ComponentInteraction, message: &str) -> Result<()> {
    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new().content(message).ephemeral(true),
            ),
        )
        .await?;
    Ok(())
}


fn attachment_id_from_cdn_url(url: &str) -> Option<AttachmentId> {
    let path = url.split("/attachments/").nth(1)?;
    let id_str = path.split('/').nth(1)?;
    id_str.split('?').next().unwrap_or(id_str).parse::<u64>().ok().map(AttachmentId::new)
}


async fn find_builder_message(ctx: &Context, channel_id: GenericChannelId) -> Option<Message> {
    ctx.http.get_message(channel_id, MessageId::new(channel_id.get())).await.ok()
}


async fn send_thread_message(ctx: &Context, channel_id: GenericChannelId, content: &str) -> Result<()> {
    ctx.http
        .send_message(channel_id, Vec::<CreateAttachment>::new(), &CreateMessage::new().content(content))
        .await?;
    Ok(())
}


fn thread_title(state: &SubmissionState) -> String {
    state.players.iter().map(|p| p.username.as_str()).collect::<Vec<_>>().join(", ")
}


async fn update_builder(
    ctx: &Context,
    channel_id: GenericChannelId,
    message: &Message,
    state: &SubmissionState,
) -> Result<()> {
    let edit = EditMessage::new()
        .flags(MessageFlags::IS_COMPONENTS_V2)
        .components(builder::build_review_message(state, &gallery_url_map(message)));
    ctx.http.edit_message(channel_id, message.id, &edit, Vec::new()).await?;
    let thread_id = channel_id.expect_thread();
    let _ = thread_id.edit(&ctx.http, EditThread::new().name(thread_title(state))).await;
    Ok(())
}



fn gallery_url_map(message: &Message) -> HashMap<String, String> {
    let Some(container) = find_container(message) else { return HashMap::new() };

    let mut map = HashMap::new();
    for part in &*container.components {
        if let ContainerComponent::MediaGallery(gallery) = part {
            for item in &*gallery.items {
                let url = item.media.url.to_string();
                if !url.starts_with("attachment://") {
                    map.insert(attachment_filename_from_url(&url), url);
                }
            }
        }
    }
    map
}


async fn update_builder_with_files(
    ctx: &Context,
    channel_id: GenericChannelId,
    message: &Message,
    state: &SubmissionState,
    files: Vec<CreateAttachment<'static>>,
) -> Result<()> {
    let existing_urls = gallery_url_map(message);

    let mut attachments = EditAttachments::new();
    for url in existing_urls.values() {
        if let Some(id) = attachment_id_from_cdn_url(url) {
            attachments = attachments.keep(id);
        }
    }
    for f in files.iter().cloned() {
        attachments = attachments.add(f);
    }

    let edit = EditMessage::new()
        .flags(MessageFlags::IS_COMPONENTS_V2)
        .components(builder::build_review_message(state, &existing_urls))
        .attachments(attachments);
    ctx.http.edit_message(channel_id, message.id, &edit, files).await?;
    Ok(())
}


fn attachment_filename_from_url(url: &str) -> String {
    if let Some(name) = url.strip_prefix("attachment://") {
        return name.to_string();
    }
    url.rsplit('/')
        .next()
        .map(|s| s.split('?').next().unwrap_or(s))
        .unwrap_or("unknown.png")
        .to_string()
}


async fn resolve_forum_tags(ctx: &Context, data: &Data) -> ForumTags {
    let empty = ForumTags {
        pending: None, approved: None, rejected: None, nicked: None, awaiting_evidence: None,
    };

    let Some(forum_id) = data.review_forum_id else { return empty };
    let Ok(channel) = ctx.http.get_channel(forum_id.into()).await else { return empty };
    let Channel::Guild(gc) = channel else { return empty };

    let find = |name: &str| gc.available_tags.iter().find(|t| t.name == name).map(|t| t.id);
    ForumTags {
        pending: find(TAG_PENDING),
        approved: find(TAG_APPROVED),
        rejected: find(TAG_REJECTED),
        nicked: find(TAG_NICKED),
        awaiting_evidence: find(TAG_AWAITING_EVIDENCE),
    }
}


async fn set_forum_tags(ctx: &Context, thread_id: ThreadId, tag_ids: &[ForumTagId]) -> Result<()> {
    thread_id.edit(&ctx.http, EditThread::new().applied_tags(tag_ids.to_vec())).await?;
    Ok(())
}


async fn check_overwrite_conflict(data: &Data, uuid: &str, tag_type: &str) -> Result<Option<String>> {
    let repo = BlacklistRepository::new(data.db.pool());
    let existing_tags = repo.get_tags(uuid).await?;
    let new_priority = lookup_tag(tag_type).map(|d| d.priority).unwrap_or(0);

    let conflict = existing_tags
        .iter()
        .find(|t| lookup_tag(&t.tag_type).map(|d| d.priority).unwrap_or(0) == new_priority);

    match conflict {
        Some(tag) => {
            let def = lookup_tag(&tag.tag_type);
            let emote = def.map(|d| d.emote).unwrap_or("");
            let display = def.map(|d| d.display_name).unwrap_or(&tag.tag_type);
            Ok(Some(format!(
                "\u{26A0} Existing tag: {} {} \u{2014} \"{}\"",
                emote, display, sanitize_reason(&tag.reason)
            )))
        }
        None => Ok(None),
    }
}


pub async fn create_submission(
    ctx: &Context,
    data: &Data,
    submitter_id: u64,
    player_name: &str,
    player_uuid: &str,
    tag_type: &str,
    reason: &str,
    is_nicked: bool,
) -> Result<ThreadId> {
    let Some(forum_id) = data.review_forum_id else {
        anyhow::bail!("Review forum channel not configured");
    };

    if !REVIEW_TAGS.contains(&tag_type) && !CONFIRMABLE_TAGS.contains(&tag_type) {
        anyhow::bail!("Tag type '{}' cannot be submitted for review", tag_type);
    }

    let player = PlayerEntry {
        username: player_name.to_string(),
        uuid: player_uuid.to_string(),
        tag_type: tag_type.to_string(),
        reason: reason.to_string(),
        is_nicked,
        status: PlayerStatus::Pending,
        reviewer: None,
        review_note: None,
        evidence: Vec::new(),
        conflict_warning: None,
        accept_votes: Vec::new(),
        reject_votes: Vec::new(),
    };

    let state = SubmissionState {
        submitter_id,
        players: vec![player],
        submitted: false,
        editing: None,
        pending_add: None,
    };

    let message = CreateMessage::new()
        .flags(MessageFlags::IS_COMPONENTS_V2)
        .components(builder::build_review_message(&state, &HashMap::new()));

    let mut forum_post = CreateForumPost::new(player_name.to_string(), message);
    let tags = resolve_forum_tags(ctx, data).await;
    if let Some(tag_id) = tags.awaiting_evidence {
        forum_post = forum_post.add_applied_tag(tag_id);
    }
    if is_nicked {
        if let Some(tag_id) = tags.nicked {
            forum_post = forum_post.add_applied_tag(tag_id);
        }
    }

    let thread = forum_id.create_forum_post(&ctx.http, forum_post).await?;
    Ok(thread.id)
}


pub fn spawn_submission_timeout(ctx: Context, thread_id: ThreadId) {
    let channel_id: GenericChannelId = thread_id.into();

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(SUBMISSION_WARNING_SECS)).await;

        let Some(msg) = find_builder_message(&ctx, channel_id).await else { return };
        let Some(state) = parse_state_from_message(&msg) else { return };
        if state.submitted { return; }

        let _ = send_thread_message(
            &ctx, channel_id,
            &format!(
                "<@{}> This submission will be automatically cancelled in 10 minutes due to inactivity.",
                state.submitter_id
            ),
        ).await;

        tokio::time::sleep(std::time::Duration::from_secs(
            SUBMISSION_TIMEOUT_SECS - SUBMISSION_WARNING_SECS,
        )).await;

        let Some(msg) = find_builder_message(&ctx, channel_id).await else { return };
        let Some(state) = parse_state_from_message(&msg) else { return };
        if state.submitted { return; }

        let _ = channel_id.delete(&ctx.http, None).await;
    });
}
