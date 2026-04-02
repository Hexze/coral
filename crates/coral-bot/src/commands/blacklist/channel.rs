use serenity::all::*;

use blacklist::{EMOTE_ADDTAG, EMOTE_EDITTAG, EMOTE_REMOVETAG, EMOTE_TAG, lookup as lookup_tag};
use database::{BlacklistRepository, PlayerTagRow};

use crate::framework::{AccessRank, Data};
use crate::utils::{format_tag_detail, format_uuid_dashed, sanitize_reason};

const FACE_SIZE: u32 = 128;
const FACE_FILENAME: &str = "face.png";

pub const COLOR_SUCCESS: u32 = 0x00FF00;
pub const COLOR_DANGER: u32 = 0xFF5555;
pub const COLOR_ERROR: u32 = 0xED4245;
pub const COLOR_INFO: u32 = 0x5865F2;
pub const COLOR_FALLBACK: u32 = 0xFFA500;


fn face_thumbnail() -> CreateThumbnail<'static> {
    CreateThumbnail::new(CreateUnfurledMediaItem::new(format!("attachment://{FACE_FILENAME}")))
}


async fn face_attachment(data: &Data, uuid: &str) -> CreateAttachment<'static> {
    let png = data.skin_provider.fetch_face(uuid, FACE_SIZE).await
        .unwrap_or_else(|| {
            let img = image::RgbaImage::from_pixel(FACE_SIZE, FACE_SIZE, image::Rgba([0, 0, 0, 0]));
            let mut buf = std::io::Cursor::new(Vec::new());
            img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
            buf.into_inner()
        });
    CreateAttachment::bytes(png, FACE_FILENAME)
}


fn face_section(parts: Vec<String>) -> CreateContainerComponent<'static> {
    CreateContainerComponent::Section(CreateSection::new(
        vec![CreateSectionComponent::TextDisplay(CreateTextDisplay::new(parts.join("\n")))],
        CreateSectionAccessory::Thumbnail(face_thumbnail()),
    ))
}


pub async fn get_username(ctx: &Context, user_id: u64) -> String {
    ctx.http
        .get_user(UserId::new(user_id))
        .await
        .map(|u| u.name.to_string())
        .unwrap_or_else(|_| user_id.to_string())
}


pub async fn format_added_line(ctx: &Context, tag: &PlayerTagRow) -> String {
    if tag.hide_username {
        format!("> -# **\\- <t:{}:R>**", tag.added_on.timestamp())
    } else {
        let username = get_username(ctx, tag.added_by as u64).await;
        format!("> -# **\\- Added by `@{}` <t:{}:R>**", username, tag.added_on.timestamp())
    }
}


pub async fn post_new_tag(
    ctx: &Context,
    data: &Data,
    uuid: &str,
    name: &str,
    tag: &PlayerTagRow,
    all_tags: &[PlayerTagRow],
) -> Option<MessageId> {
    post_tag_to_log(ctx, data, uuid, name, tag, "New Tag", EMOTE_ADDTAG).await;
    post_to_blacklist_channel(ctx, data, uuid, name, all_tags, "New Tag", EMOTE_ADDTAG).await
}


pub async fn post_overwritten_tag(
    ctx: &Context,
    data: &Data,
    uuid: &str,
    name: &str,
    tag: &PlayerTagRow,
    all_tags: &[PlayerTagRow],
) -> Option<MessageId> {
    post_tag_to_log(ctx, data, uuid, name, tag, "Tag Overwritten", EMOTE_EDITTAG).await;
    post_to_blacklist_channel(ctx, data, uuid, name, all_tags, "Tag Overwritten", EMOTE_EDITTAG).await
}


pub async fn post_tag_removed(
    ctx: &Context,
    data: &Data,
    uuid: &str,
    name: &str,
    tag: &PlayerTagRow,
    removed_by: u64,
) {
    let def = lookup_tag(&tag.tag_type);
    let emote = def.map(|d| d.emote).unwrap_or("");
    let display_name = def.map(|d| d.display_name).unwrap_or(&tag.tag_type);
    let dashed_uuid = format_uuid_dashed(uuid);
    let username = get_username(ctx, removed_by).await;

    let face = face_attachment(data, uuid).await;
    let container = CreateContainer::new(vec![
        face_section(vec![
            format!("## {} Tag Removed\nIGN - `{}`", EMOTE_REMOVETAG, name),
            format!("**{} {}**\n> {}\n> -# **\\- Removed by `@{}`**", emote, display_name, format_tag_detail(tag), username),
            format!("-# UUID: {dashed_uuid}"),
        ]),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ])
    .accent_color(COLOR_DANGER);

    send_to_mod_channel(ctx, data, container, vec![face]).await;
}


pub async fn post_tag_changed(
    ctx: &Context,
    data: &Data,
    uuid: &str,
    name: &str,
    old_tag: &PlayerTagRow,
    new_tag: &PlayerTagRow,
    title: &str,
    changed_by: u64,
) {
    let dashed_uuid = format_uuid_dashed(uuid);

    let old_def = lookup_tag(&old_tag.tag_type);
    let old_emote = old_def.map(|d| d.emote).unwrap_or("");
    let old_display = old_def.map(|d| d.display_name).unwrap_or(&old_tag.tag_type);

    let new_def = lookup_tag(&new_tag.tag_type);
    let new_emote = new_def.map(|d| d.emote).unwrap_or("");
    let new_display = new_def.map(|d| d.display_name).unwrap_or(&new_tag.tag_type);

    let old_added_line = format_added_line(ctx, old_tag).await;
    let new_added_line = format_added_line(ctx, new_tag).await;
    let username = get_username(ctx, changed_by).await;

    let face = face_attachment(data, uuid).await;
    let container = CreateContainer::new(vec![
        face_section(vec![
            format!("## {} {}\nIGN - `{}`", EMOTE_EDITTAG, title, name),
            format!("Previous: **{} {}**\n> {}\n{}", old_emote, old_display, format_tag_detail(old_tag), old_added_line),
        ]),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "New: **{} {}**\n> {}\n{}", new_emote, new_display, format_tag_detail(new_tag), new_added_line
        ))),
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "-# {} by `@{}`\n-# UUID: {dashed_uuid}", title, username
        ))),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ])
;

    send_to_mod_channel(ctx, data, container, vec![face]).await;
}


pub async fn post_lock_change(
    ctx: &Context,
    data: &Data,
    uuid: &str,
    name: &str,
    locked: bool,
    reason: Option<&str>,
    changed_by: u64,
) {
    let dashed_uuid = format_uuid_dashed(uuid);
    let title = if locked {
        format!("## {} Player Locked \u{1F512}\nIGN - `{}`", EMOTE_TAG, name)
    } else {
        format!("## {} Player Unlocked \u{1F513}\nIGN - `{}`", EMOTE_TAG, name)
    };

    let face = face_attachment(data, uuid).await;
    let username = get_username(ctx, changed_by).await;
    let action = if locked { "Locked" } else { "Unlocked" };

    let mut section_parts = vec![title];
    if let Some(r) = reason {
        section_parts.push(format!("> {}", sanitize_reason(r)));
    }
    section_parts.push(format!("-# {} by `@{}`\n-# UUID: {dashed_uuid}", action, username));

    let parts = vec![
        face_section(section_parts),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ];

    send_to_mod_channel(ctx, data, CreateContainer::new(parts), vec![face]).await;
}


pub async fn post_key_revoked(
    ctx: &Context,
    data: &Data,
    target_id: u64,
    reason: &str,
    invoker_id: u64,
) {
    let invoker = get_username(ctx, invoker_id).await;
    let container = CreateContainer::new(vec![
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "## \u{1F528} User Banned\n<@{target_id}>"
        ))),
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "> {}", sanitize_reason(reason)
        ))),
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "-# Banned by `@{invoker}`"
        ))),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ])
    .accent_color(COLOR_DANGER);

    send_to_mod_channel(ctx, data, container, vec![]).await;
}


pub async fn post_key_locked(ctx: &Context, data: &Data, target_id: u64, invoker_id: u64) {
    post_key_change(ctx, data, target_id, invoker_id, true).await;
}


pub async fn post_key_unlocked(ctx: &Context, data: &Data, target_id: u64, invoker_id: u64) {
    post_key_change(ctx, data, target_id, invoker_id, false).await;
}


pub async fn post_access_changed(
    ctx: &Context,
    data: &Data,
    target_id: u64,
    old_rank: AccessRank,
    new_rank: AccessRank,
    invoker_id: u64,
) {
    let invoker = get_username(ctx, invoker_id).await;
    let container = CreateContainer::new(vec![
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "## Access Level Changed\n<@{target_id}>"
        ))),
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "{} \u{2192} {}", old_rank.label(), new_rank.label()
        ))),
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "-# Changed by `@{invoker}`"
        ))),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ])
;

    send_to_mod_channel(ctx, data, container, vec![]).await;
}


pub async fn post_tagging_toggled(
    ctx: &Context,
    data: &Data,
    target_id: u64,
    disabled: bool,
    invoker_id: u64,
) {
    let title = if disabled { "Tagging Disabled" } else { "Tagging Enabled" };

    let invoker = get_username(ctx, invoker_id).await;
    let container = CreateContainer::new(vec![
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "## {title}\n<@{target_id}>"
        ))),
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "-# Changed by `@{invoker}`"
        ))),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ]);
    let container = if disabled { container.accent_color(COLOR_DANGER) } else { container };

    send_to_mod_channel(ctx, data, container, vec![]).await;
}


async fn post_key_change(
    ctx: &Context,
    data: &Data,
    target_id: u64,
    invoker_id: u64,
    locked: bool,
) {
    let (title, action) = if locked {
        ("API Key Locked \u{1F512}", "Locked")
    } else {
        ("API Key Unlocked \u{1F513}", "Unlocked")
    };

    let invoker = get_username(ctx, invoker_id).await;
    let container = CreateContainer::new(vec![
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "## {title}\n<@{target_id}>"
        ))),
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
            "-# {action} by `@{invoker}`"
        ))),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ]);
    let container = if locked { container.accent_color(COLOR_DANGER) } else { container };

    send_to_mod_channel(ctx, data, container, vec![]).await;
}


async fn post_tag_to_log(
    ctx: &Context,
    data: &Data,
    uuid: &str,
    name: &str,
    tag: &PlayerTagRow,
    title: &str,
    emote: &str,
) {
    let def = lookup_tag(&tag.tag_type);
    let tag_emote = def.map(|d| d.emote).unwrap_or("");
    let display_name = def.map(|d| d.display_name).unwrap_or(&tag.tag_type);
    let dashed_uuid = format_uuid_dashed(uuid);
    let added_line = format_added_line(ctx, tag).await;
    let face = face_attachment(data, uuid).await;

    let container = CreateContainer::new(vec![
        face_section(vec![
            format!("## {} {}\nIGN - `{}`", emote, title, name),
            format!("**{} {}**\n> {}\n{}", tag_emote, display_name, format_tag_detail(tag), added_line),
            format!("-# UUID: {dashed_uuid}"),
        ]),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ]);

    send_to_mod_channel(ctx, data, container, vec![face]).await;
}


async fn send_to_mod_channel(
    ctx: &Context,
    data: &Data,
    container: CreateContainer<'static>,
    files: Vec<CreateAttachment<'static>>,
) {
    let Some(channel_id) = data.mod_channel_id else { return };
    let _ = send_container(ctx, channel_id, container, files).await;
}


async fn post_to_blacklist_channel(
    ctx: &Context,
    data: &Data,
    uuid: &str,
    name: &str,
    all_tags: &[PlayerTagRow],
    title: &str,
    emote: &str,
) -> Option<MessageId> {
    let channel_id = data.blacklist_channel_id?;
    let dashed_uuid = format_uuid_dashed(uuid);

    let evidence_thread = BlacklistRepository::new(data.db.pool())
        .get_player(uuid).await.ok().flatten()
        .and_then(|p| p.evidence_thread);

    let face = face_attachment(data, uuid).await;

    let mut section_parts = vec![format!("## {} {}\nIGN - `{}`", emote, title, name)];

    for tag in all_tags {
        let def = lookup_tag(&tag.tag_type);
        let tag_emote = def.map(|d| d.emote).unwrap_or("");
        let display_name = def.map(|d| d.display_name).unwrap_or(&tag.tag_type);

        let evidence_indicator = if tag.tag_type == "confirmed_cheater" {
            if evidence_thread.is_some() { " <:evidencefound:1482666860225888346>" }
            else { " <:noevidence:1482666258938990696>" }
        } else { "" };

        let added_line = format_added_line(ctx, tag).await;
        let mut tag_text = format!(
            "**{} {}**{}\n> {}\n{}",
            tag_emote, display_name, evidence_indicator, format_tag_detail(tag), added_line
        );

        if let Some(reviewers) = &tag.reviewed_by {
            if !reviewers.is_empty() {
                let names: Vec<String> = futures::future::join_all(
                    reviewers.iter().map(|&id| async move {
                        format!("`@{}`", get_username(ctx, id as u64).await)
                    })
                ).await;
                tag_text.push_str(&format!("\n> -# **\\- Reviewed by {}**", names.join(", ")));
            }
        }

        section_parts.push(tag_text);
    }

    let mut footer = format!("-# UUID: {dashed_uuid}");
    if let Some(ref url) = evidence_thread {
        footer.push_str(&format!(" | [Evidence]({url})"));
    }
    section_parts.push(footer);

    let parts = vec![
        face_section(section_parts),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ];

    send_container(ctx, channel_id, CreateContainer::new(parts), vec![face]).await
}


async fn send_container(
    ctx: &Context,
    channel_id: ChannelId,
    container: CreateContainer<'static>,
    files: Vec<CreateAttachment<'static>>,
) -> Option<MessageId> {
    match ctx
        .http
        .send_message(
            channel_id.into(),
            files,
            &CreateMessage::new()
                .flags(MessageFlags::IS_COMPONENTS_V2)
                .components(vec![CreateComponent::Container(container)]),
        )
        .await
    {
        Ok(msg) => Some(msg.id),
        Err(e) => {
            tracing::error!("Failed to post to channel {}: {}", channel_id, e);
            None
        }
    }
}
