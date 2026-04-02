use std::collections::HashMap;

use anyhow::Result;
use blacklist::{EMOTE_ADDTAG, EMOTE_EDITTAG, EMOTE_REMOVETAG, EMOTE_TAG, lookup as lookup_tag};
use coral_redis::BlacklistEvent;
use database::{BlacklistRepository, CacheRepository, MemberRepository};
use serenity::all::*;

use super::channel::{self, COLOR_DANGER, format_added_line};
use crate::framework::{AccessRank, AccessRankExt, Data};
use crate::interact;
use crate::interact::send_deferred_error;
use crate::utils::{format_tag_detail, format_uuid_dashed, sanitize_reason};

const FACE_SIZE: u32 = 128;
const FACE_FILENAME: &str = "face.png";
const EMOTE_EVIDENCE: &str = "<:evidencefound:1482666860225888346>";
const EMOTE_NO_EVIDENCE: &str = "<:noevidence:1482666258938990696>";


fn face_thumbnail() -> CreateThumbnail<'static> {
    CreateThumbnail::new(CreateUnfurledMediaItem::new(format!("attachment://{FACE_FILENAME}")))
}


async fn face_attachment(data: &Data, uuid: &str) -> CreateAttachment<'static> {
    let png = data.skin_provider.fetch_face(uuid, FACE_SIZE).await
        .unwrap_or_else(default_face);
    CreateAttachment::bytes(png, FACE_FILENAME)
}

fn default_face() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(FACE_SIZE, FACE_SIZE, image::Rgba([0, 0, 0, 0]));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    buf.into_inner()
}


fn face_section(parts: Vec<String>) -> CreateContainerComponent<'static> {
    CreateContainerComponent::Section(CreateSection::new(
        vec![CreateSectionComponent::TextDisplay(CreateTextDisplay::new(parts.join("\n")))],
        CreateSectionAccessory::Thumbnail(face_thumbnail()),
    ))
}


fn container_response(container: CreateContainer<'static>) -> Vec<CreateComponent<'static>> {
    vec![CreateComponent::Container(container)]
}


fn simple_result(emote: &str, msg: &str) -> CreateContainer<'static> {
    CreateContainer::new(vec![CreateContainerComponent::TextDisplay(
        CreateTextDisplay::new(format!("## {emote} {msg}")),
    )])
}


pub struct PendingOverwrite {
    pub uuid: String,
    pub old_tag_id: i64,
    pub tag_type: String,
    pub reason: String,
    pub hide: bool,
}


pub struct PendingTagChanges {
    pub uuid: String,
    pub username: String,
    pub owner_id: u64,
    pub owner_name: String,
    pub is_staff: bool,
    pub rank: AccessRank,
    pub entries: Vec<TagChangeEntry>,
    pub resolved_names: HashMap<i64, String>,
    pub face_url: Option<String>,
}


pub struct TagChangeEntry {
    pub tag_id: i64,
    pub original: database::PlayerTagRow,
    pub new_type: Option<String>,
    pub new_reason: Option<String>,
    pub claimed: bool,
    pub hide: Option<bool>,
    pub removed: bool,
    pub editable: bool,
}


impl TagChangeEntry {
    fn effective_type(&self) -> &str {
        self.new_type.as_deref().unwrap_or(&self.original.tag_type)
    }

    fn effective_reason(&self) -> &str {
        self.new_reason.as_deref().unwrap_or(&self.original.reason)
    }

    fn effective_added_by(&self, owner_id: u64) -> i64 {
        if self.claimed { owner_id as i64 } else { self.original.added_by }
    }

    fn effective_hide(&self) -> bool {
        self.hide.unwrap_or(self.original.hide_username)
    }

    fn has_changes(&self, owner_id: u64) -> bool {
        self.new_type.is_some()
            || self.new_reason.is_some()
            || (self.claimed && self.original.added_by != owner_id as i64)
            || self.hide.map(|h| h != self.original.hide_username).unwrap_or(false)
    }
}


fn tag_choices(option: CreateCommandOption<'static>) -> CreateCommandOption<'static> {
    blacklist::user_addable()
        .iter()
        .filter(|tag| tag.name != "replays_needed")
        .fold(option, |opt, tag| opt.add_string_choice(tag.display_name, tag.name))
}


fn remove_tag_choices(option: CreateCommandOption<'static>) -> CreateCommandOption<'static> {
    blacklist::all()
        .iter()
        .fold(option, |opt, tag| opt.add_string_choice(tag.display_name, tag.name))
}


pub fn register() -> CreateCommand<'static> {
    CreateCommand::new("tag")
        .description("Manage player tags")
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "view", "View a player's tags")
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "player", "Player name or UUID")
                        .required(true),
                ),
        )
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "add", "Add a tag to a player")
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "player", "Player name or UUID")
                        .required(true),
                )
                .add_sub_option(tag_choices(
                    CreateCommandOption::new(CommandOptionType::String, "type", "Tag type").required(true),
                ))
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "reason", "Reason for the tag")
                        .required(true)
                        .max_length(120),
                )
                .add_sub_option(CreateCommandOption::new(
                    CommandOptionType::Boolean, "hide", "Hide your username",
                )),
        )
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "remove", "Remove a tag from a player")
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "player", "Player name or UUID")
                        .required(true),
                )
                .add_sub_option(remove_tag_choices(
                    CreateCommandOption::new(CommandOptionType::String, "type", "Tag type to remove")
                        .required(true),
                )),
        )
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "change", "Manage a player's existing tags")
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "player", "Player name or UUID")
                        .required(true),
                ),
        )
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "lock", "Lock a player's tags from modification")
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "player", "Player name or UUID")
                        .required(true),
                )
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "reason", "Reason for locking")
                        .required(true)
                        .max_length(120),
                ),
        )
        .add_option(
            CreateCommandOption::new(CommandOptionType::SubCommand, "unlock", "Unlock a player's tags")
                .add_sub_option(
                    CreateCommandOption::new(CommandOptionType::String, "player", "Player name or UUID")
                        .required(true),
                ),
        )
}


pub async fn run(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    match command.data.options.first().map(|o| o.name.as_str()) {
        Some("view") => run_view(ctx, command, data).await,
        Some("add") => run_add(ctx, command, data).await,
        Some("remove") => run_remove(ctx, command, data).await,
        Some("change") => run_change(ctx, command, data).await,
        Some("lock") => run_lock(ctx, command, data).await,
        Some("unlock") => run_unlock(ctx, command, data).await,
        _ => Ok(()),
    }
}


fn get_sub_options(command: &CommandInteraction) -> Vec<ResolvedOption<'_>> {
    command
        .data
        .options()
        .first()
        .map(|o| match &o.value {
            ResolvedValue::SubCommand(opts) => opts.to_vec(),
            _ => vec![],
        })
        .unwrap_or_default()
}


fn get_string<'a>(options: &'a [ResolvedOption<'a>], name: &str) -> &'a str {
    options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| match o.value {
            ResolvedValue::String(s) => Some(s),
            _ => None,
        })
        .unwrap_or("")
}


fn get_bool(options: &[ResolvedOption<'_>], name: &str) -> bool {
    options
        .iter()
        .find(|o| o.name == name)
        .and_then(|o| match o.value {
            ResolvedValue::Boolean(b) => Some(b),
            _ => None,
        })
        .unwrap_or(false)
}


fn is_valid_minecraft_name(name: &str) -> bool {
    (3..=16).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}


pub(super) async fn get_rank(data: &Data, discord_id: u64) -> Result<AccessRank> {
    let member_repo = MemberRepository::new(data.db.pool());
    let member = member_repo.get_by_discord_id(discord_id as i64).await?;
    Ok(AccessRank::of(data, discord_id, member.as_ref()))
}


async fn get_rank_and_member(
    data: &Data,
    discord_id: u64,
) -> Result<(AccessRank, Option<database::Member>)> {
    let member_repo = MemberRepository::new(data.db.pool());
    let member = member_repo.get_by_discord_id(discord_id as i64).await?;
    let rank = AccessRank::of(data, discord_id, member.as_ref());
    Ok((rank, member))
}


pub(super) enum MemberCheck {
    Ok(AccessRank, database::Member),
    NotLinked,
    NotInGuild,
}


pub(super) async fn require_linked_member(ctx: &Context, data: &Data, discord_id: u64) -> Result<MemberCheck> {
    let (rank, member) = get_rank_and_member(data, discord_id).await?;
    let Some(member) = member.filter(|m| m.uuid.is_some()) else {
        return Ok(MemberCheck::NotLinked);
    };
    if let Some(guild_id) = data.home_guild_id {
        if guild_id.member(&ctx.http, UserId::new(discord_id)).await.is_err() {
            return Ok(MemberCheck::NotInGuild);
        }
    }
    Ok(MemberCheck::Ok(rank, member))
}


async fn send_tag_response(
    ctx: &Context,
    command: &CommandInteraction,
    data: &Data,
    uuid: &str,
    container: CreateContainer<'static>,
) -> Result<()> {
    let mut resp = EditInteractionResponse::new()
        .flags(MessageFlags::IS_COMPONENTS_V2)
        .components(container_response(container));
    resp = resp.new_attachment(face_attachment(data, uuid).await);
    command.edit_response(&ctx.http, resp).await?;
    Ok(())
}


async fn run_view(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    command.defer(&ctx.http).await?;

    let options = get_sub_options(command);
    let player = get_string(&options, "player");

    let player_info = match data.api.resolve(player).await {
        Ok(info) => info,
        Err(_) => return send_deferred_error(ctx, command, "Error", "Player not found").await,
    };

    let repo = BlacklistRepository::new(data.db.pool());
    let (player_data, player_tags, face) = tokio::join!(
        repo.get_player(&player_info.uuid),
        repo.get_tags(&player_info.uuid),
        face_attachment(data, &player_info.uuid),
    );
    let player_data = player_data?;
    let player_tags = player_tags?;

    let is_locked = player_data.as_ref().map(|p| p.is_locked).unwrap_or(false);
    let dashed_uuid = format_uuid_dashed(&player_info.uuid);

    if player_tags.is_empty() {
        let container = CreateContainer::new(vec![
            face_section(vec![
                format!("## No Tags\n`{}` is not tagged.", player_info.username),
                format!("-# UUID: {dashed_uuid}"),
            ]),
            CreateContainerComponent::Separator(CreateSeparator::new(true)),
        ]);
        let mut resp = EditInteractionResponse::new()
            .flags(MessageFlags::IS_COMPONENTS_V2)
            .components(container_response(container));
        resp = resp.new_attachment(face);
        command.edit_response(&ctx.http, resp).await?;
        return Ok(());
    }

    let evidence_thread = player_data.as_ref().and_then(|p| p.evidence_thread.as_ref());
    let lock_indicator = if is_locked { " \u{1F512}" } else { "" };

    let unique_ids: Vec<i64> = {
        let mut seen = std::collections::HashSet::new();
        let mut ids = Vec::new();
        for tag in &player_tags {
            if !tag.hide_username && seen.insert(tag.added_by) {
                ids.push(tag.added_by);
            }
            if let Some(reviewers) = &tag.reviewed_by {
                for &reviewer_id in reviewers {
                    if seen.insert(reviewer_id) {
                        ids.push(reviewer_id);
                    }
                }
            }
        }
        ids
    };

    let mut join_set = tokio::task::JoinSet::new();
    let http = ctx.http.clone();
    for id in unique_ids {
        let http = http.clone();
        join_set.spawn(async move {
            let name = http
                .get_user(UserId::new(id as u64))
                .await
                .map(|u| u.name.to_string())
                .unwrap_or_else(|_| id.to_string());
            (id, name)
        });
    }

    let mut resolved_names = std::collections::HashMap::new();
    while let Some(Ok((id, name))) = join_set.join_next().await {
        resolved_names.insert(id, name);
    }

    let mut parts = vec![format!(
        "## {} Tagged User{}\nIGN - `{}`\n", EMOTE_TAG, lock_indicator, player_info.username
    )];

    for tag in &player_tags {
        let def = lookup_tag(&tag.tag_type);
        let emote = def.map(|d| d.emote).unwrap_or("");
        let display_name = def.map(|d| d.display_name).unwrap_or(&tag.tag_type);

        let added_line = if tag.hide_username {
            format!("> -# **\\- <t:{}:R>**", tag.added_on.timestamp())
        } else {
            let fallback = tag.added_by.to_string();
            let username = resolved_names.get(&tag.added_by).map(|s| s.as_str()).unwrap_or(&fallback);
            format!("> -# **\\- Added by `@{}` <t:{}:R>**", username, tag.added_on.timestamp())
        };

        let reviewed_line = tag.reviewed_by.as_ref().map(|ids| {
            let formatted: Vec<String> = ids
                .iter()
                .map(|id| {
                    let name = resolved_names.get(id).cloned().unwrap_or_else(|| id.to_string());
                    format!("`@{name}`")
                })
                .collect();
            format!("> -# **\\- Reviewed by {}**", formatted.join(", "))
        });

        let evidence_indicator = if tag.tag_type == "confirmed_cheater" {
            if evidence_thread.is_some() {
                format!(" {EMOTE_EVIDENCE}")
            } else {
                format!(" {EMOTE_NO_EVIDENCE}")
            }
        } else {
            String::new()
        };

        let mut display = format!(
            "**{} {}**{}\n> {}\n{}",
            emote, display_name, evidence_indicator, format_tag_detail(tag), added_line
        );
        if let Some(line) = reviewed_line {
            display.push('\n');
            display.push_str(&line);
        }

        parts.push(display);
    }

    let mut footer = format!("-# UUID: {dashed_uuid}");
    if let Some(ref evidence_url) = player_data.as_ref().and_then(|p| p.evidence_thread.as_ref()) {
        footer.push_str(&format!(" | [Evidence]({evidence_url})"));
    }
    parts.push(footer);

    let components: Vec<CreateContainerComponent> = vec![
        face_section(parts),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ];

    let mut resp = EditInteractionResponse::new()
        .flags(MessageFlags::IS_COMPONENTS_V2)
        .components(container_response(CreateContainer::new(components)));
    resp = resp.new_attachment(face);
    command.edit_response(&ctx.http, resp).await?;
    Ok(())
}


async fn run_add(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    command.defer_ephemeral(&ctx.http).await?;

    let discord_id = command.user.id.get();
    let (rank, member) = match require_linked_member(ctx, data, discord_id).await? {
        MemberCheck::Ok(r, m) => (r, m),
        MemberCheck::NotInGuild =>
            return send_deferred_error(ctx, command, "Error", "You must be in the Urchin server to use this command").await,
        MemberCheck::NotLinked =>
            return send_deferred_error(ctx, command, "Error", "You must link your account to add tags").await,
    };
    if rank < AccessRank::Helper && member.tagging_disabled {
        return send_deferred_error(ctx, command, "Error", "Your tagging ability has been disabled").await;
    }

    let options = get_sub_options(command);
    let player = get_string(&options, "player");
    let tag_type = get_string(&options, "type");
    let reason = get_string(&options, "reason");
    let hide = get_bool(&options, "hide") && blacklist::permissions::can_set_hide(rank.to_level());

    if tag_type == "confirmed_cheater" {
        return send_deferred_error(
            ctx, command, "Error",
            "Confirmed cheater tags can only be applied through the review system",
        ).await;
    }
    if tag_type == "caution" && !blacklist::permissions::can_add("caution", rank.to_level()) {
        return send_deferred_error(ctx, command, "Error", "Only moderators and above can add caution tags").await;
    }
    if tag_type == "replays_needed" {
        return send_deferred_error(ctx, command, "Error", "Use the /watch command to add replays needed tags").await;
    }

    if reason.is_empty() {
        return send_deferred_error(ctx, command, "Error", "A reason is required for this tag type").await;
    }

    let mut needs_review = match rank {
        AccessRank::Default => tag_type != "sniper",
        _ => false,
    };

    let player_info = match data.api.resolve(player).await {
        Ok(info) => info,
        Err(_) => return send_deferred_error(ctx, command, "Error", "Player not found").await,
    };
    let player_name = player_info.username;
    let player_uuid = player_info.uuid;

    if needs_review {
        let components = super::reviews::build_confirmation_message(
            discord_id, &player_name, &player_uuid, tag_type, reason, false,
        );
        command
            .edit_response(
                &ctx.http,
                EditInteractionResponse::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(components),
            )
            .await?;
        return Ok(());
    }

    let player_info = crate::api::ResolveResponse {
        username: player_name,
        uuid: player_uuid,
    };

    let repo = BlacklistRepository::new(data.db.pool());

    if let Some(player_data) = repo.get_player(&player_info.uuid).await? {
        if player_data.is_locked {
            return send_deferred_error(ctx, command, "Error", "This player's tags are locked").await;
        }
    }

    let existing_tags = repo.get_tags(&player_info.uuid).await?;
    let new_priority = lookup_tag(tag_type).map(|d| d.priority).unwrap_or(0);
    let conflicting_tag = existing_tags
        .iter()
        .find(|t| lookup_tag(&t.tag_type).map(|d| d.priority).unwrap_or(0) == new_priority);

    if let Some(conflict) = conflicting_tag {
        if rank < AccessRank::Member {
            return send_deferred_error(ctx, command, "Error", "You need member access to overwrite existing tags").await;
        }
        if conflict.tag_type == "confirmed_cheater" && rank < AccessRank::Helper {
            return send_deferred_error(ctx, command, "Error", "Only helpers and above can overwrite confirmed cheater tags").await;
        }

        let old_def = lookup_tag(&conflict.tag_type);
        let old_emote = old_def.map(|d| d.emote).unwrap_or("");
        let old_display = old_def.map(|d| d.display_name).unwrap_or(&conflict.tag_type);

        let new_def = lookup_tag(tag_type);
        let new_emote = new_def.map(|d| d.emote).unwrap_or("");
        let new_display = new_def.map(|d| d.display_name).unwrap_or(tag_type);

        let dashed_uuid = format_uuid_dashed(&player_info.uuid);
        let overwrite_key = command.id.to_string();
        data.pending_overwrites.lock().unwrap().insert(
            overwrite_key.clone(),
            PendingOverwrite {
                uuid: player_info.uuid.clone(),
                old_tag_id: conflict.id,
                tag_type: tag_type.to_string(),
                reason: reason.to_string(),
                hide,
            },
        );

        let button = CreateButton::new(format!("tag_overwrite:{overwrite_key}"))
            .label("Overwrite Tag")
            .style(ButtonStyle::Danger);

        let old_tag_added = format_added_line(ctx, conflict).await;
        let new_tag_added = if hide {
            String::new()
        } else {
            format!("\n> -# **\\- Added by `@{}`**", command.user.name)
        };

        let container = CreateContainer::new(vec![
            face_section(vec![
                format!("## {} Tag Overwrite\nIGN - `{}`", EMOTE_EDITTAG, player_info.username),
                format!("**{} {}**\n> {}\n{}", old_emote, old_display, format_tag_detail(conflict), old_tag_added),
                format!("-# UUID: {dashed_uuid}"),
            ]),
            CreateContainerComponent::Separator(CreateSeparator::new(true)),
            CreateContainerComponent::Section(CreateSection::new(
                vec![CreateSectionComponent::TextDisplay(CreateTextDisplay::new(format!(
                    "**{} {}**\n> {}{}", new_emote, new_display, sanitize_reason(reason), new_tag_added
                )))],
                CreateSectionAccessory::Button(button),
            )),
        ])
;

        let mut resp = EditInteractionResponse::new()
            .flags(MessageFlags::IS_COMPONENTS_V2)
            .components(vec![
                CreateComponent::TextDisplay(CreateTextDisplay::new(
                    "This user already has an incompatible tag! Would you like to overwrite?",
                )),
                CreateComponent::Container(container),
            ]);
        resp = resp.new_attachment(face_attachment(data, &player_info.uuid).await);
        command.edit_response(&ctx.http, resp).await?;
        return Ok(());
    }

    repo.add_tag(&player_info.uuid, tag_type, reason, discord_id as i64, hide, None).await?;

    let player_tags = repo.get_tags(&player_info.uuid).await?;
    let new_tag = player_tags.iter().find(|t| t.tag_type == tag_type);

    let def = lookup_tag(tag_type);
    let emote = def.map(|d| d.emote).unwrap_or("");
    let display_name = def.map(|d| d.display_name).unwrap_or(tag_type);
    let dashed_uuid = format_uuid_dashed(&player_info.uuid);

    let added_line = match &new_tag {
        Some(tag) => format_added_line(ctx, tag).await,
        None if hide => String::new(),
        None => format!("\n> -# **\\- Added by `@{}`**", command.user.name),
    };

    if let Some(tag) = &new_tag {
        data.event_publisher
            .publish(&BlacklistEvent::TagAdded {
                uuid: player_info.uuid.clone(),
                tag_id: tag.id,
                added_by: command.user.id.get() as i64,
            })
            .await;
    }

    let tag_id = new_tag.map(|t| t.id).unwrap_or(0);
    let container = CreateContainer::new(vec![
        face_section(vec![
            format!("## {} New Tag Applied\nIGN - `{}`", EMOTE_ADDTAG, player_info.username),
            format!("**{} {}**\n> {}\n{}", emote, display_name, sanitize_reason(reason), added_line),
            format!("-# UUID: {dashed_uuid}"),
        ]),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
        CreateContainerComponent::ActionRow(CreateActionRow::buttons(vec![
            CreateButton::new(format!("tag_edit:{tag_id}")).label("Edit").style(ButtonStyle::Secondary),
            CreateButton::new(format!("tag_undo:{tag_id}")).label("Undo").style(ButtonStyle::Danger),
        ])),
        CreateContainerComponent::TextDisplay(CreateTextDisplay::new(
            "-# You can also use /tag change within 30 minutes to update this tag",
        )),
    ])
;

    send_tag_response(ctx, command, data, &player_info.uuid, container).await
}


pub async fn handle_overwrite_button(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let key = component.data.custom_id.strip_prefix("tag_overwrite:").unwrap_or_default();
    let overwrite = data.pending_overwrites.lock().unwrap().remove(key);

    let Some(overwrite) = overwrite else {
        return send_component_message(ctx, component, "This overwrite has expired").await;
    };

    let uuid = &overwrite.uuid;
    let discord_id = component.user.id.get();
    let rank = get_rank(data, discord_id).await?;

    let cache = CacheRepository::new(data.db.pool());
    let player_name = cache.get_username(uuid).await.ok().flatten().unwrap_or_else(|| uuid.to_string());

    let ops = database::TagOp::new(data.db.pool());
    let (old_tag_clone, new_tag_row) = match ops.overwrite(
        uuid, overwrite.old_tag_id, &overwrite.tag_type, &overwrite.reason,
        discord_id as i64, rank.to_level(), overwrite.hide,
    ).await {
        Ok(result) => result,
        Err(database::TagOpError::TagNotFound) =>
            return send_component_message(ctx, component, "The original tag no longer exists").await,
        Err(database::TagOpError::InsufficientPermissions) =>
            return send_component_message(ctx, component, "You don't have permission to overwrite this tag").await,
        Err(database::TagOpError::PlayerLocked) =>
            return send_component_message(ctx, component, "This player's tags are locked").await,
        Err(e) => return Err(anyhow::anyhow!("{e:?}")),
    };
    let new_tag = Some(&new_tag_row);

    let def = lookup_tag(&overwrite.tag_type);
    let emote = def.map(|d| d.emote).unwrap_or("");
    let display_name = def.map(|d| d.display_name).unwrap_or(&overwrite.tag_type);
    let dashed_uuid = format_uuid_dashed(uuid);

    let added_line = match &new_tag {
        Some(tag) => format_added_line(ctx, tag).await,
        None if overwrite.hide => String::new(),
        None => format!("\n> -# **\\- Added by `@{}`**", component.user.name),
    };

    let container = CreateContainer::new(vec![
        face_section(vec![
            format!("## {} Tag Overwritten\nIGN - `{}`", EMOTE_EDITTAG, player_name),
            format!("**{} {}**\n> {}\n{}", emote, display_name, sanitize_reason(&overwrite.reason), added_line),
            format!("-# UUID: {dashed_uuid}"),
        ]),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ]);

    let mut msg = CreateInteractionResponseMessage::new()
        .flags(MessageFlags::IS_COMPONENTS_V2)
        .components(container_response(container));
    msg = msg.add_file(face_attachment(data, uuid).await);
    component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(msg)).await?;

    if let Some(new_tag) = &new_tag {
        data.event_publisher
            .publish(&BlacklistEvent::TagOverwritten {
                uuid: uuid.to_string(),
                old_tag_id: old_tag_clone.id,
                old_tag_type: old_tag_clone.tag_type.clone(),
                old_reason: old_tag_clone.reason.clone(),
                new_tag_id: new_tag.id,
                overwritten_by: discord_id as i64,
            })
            .await;
    }

    Ok(())
}


async fn run_remove(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    command.defer_ephemeral(&ctx.http).await?;

    let discord_id = command.user.id.get();
    let (rank, _) = match require_linked_member(ctx, data, discord_id).await? {
        MemberCheck::Ok(r, m) => (r, m),
        MemberCheck::NotInGuild =>
            return send_deferred_error(ctx, command, "Error", "You must be in the Urchin server to use this command").await,
        MemberCheck::NotLinked =>
            return send_deferred_error(ctx, command, "Error", "You must link your account to remove tags").await,
    };
    let options = get_sub_options(command);
    let player = get_string(&options, "player");
    let tag_type = get_string(&options, "type");

    let player_info = match data.api.resolve(player).await {
        Ok(info) => info,
        Err(_) => return send_deferred_error(ctx, command, "Error", "Player not found").await,
    };

    let repo = BlacklistRepository::new(data.db.pool());
    if let Some(player_data) = repo.get_player(&player_info.uuid).await? {
        if player_data.is_locked {
            return send_deferred_error(ctx, command, "Error", "This player's tags are locked").await;
        }
    }

    let player_tags = repo.get_tags(&player_info.uuid).await?;
    let Some(tag) = player_tags.iter().find(|t| t.tag_type == tag_type) else {
        return send_deferred_error(ctx, command, "Error", &format!("Player doesn't have a {} tag", tag_type)).await;
    };

    if !can_remove_tag(rank, tag, discord_id) {
        return send_deferred_error(ctx, command, "Error", "You don't have permission to remove this tag").await;
    }

    let tag_clone = tag.clone();
    if !repo.remove_tag(tag.id, discord_id as i64).await? {
        return send_deferred_error(ctx, command, "Error", "Failed to remove tag").await;
    }

    if tag_type == "confirmed_cheater" {
        if let Some(player_data) = repo.get_player(&player_info.uuid).await? {
            if let Some(thread_url) = &player_data.evidence_thread {
                super::evidence::archive_evidence_by_url(ctx, data, thread_url).await?;
            }
        }
    }

    let def = lookup_tag(tag_type);
    let emote = def.map(|d| d.emote).unwrap_or("");
    let display_name = def.map(|d| d.display_name).unwrap_or(tag_type);
    let dashed_uuid = format_uuid_dashed(&player_info.uuid);
    let added_line = format_added_line(ctx, &tag_clone).await;

    let container = CreateContainer::new(vec![
        face_section(vec![
            format!("## {} Tag Removed\nIGN - `{}`", EMOTE_REMOVETAG, player_info.username),
            format!("**{} {}**\n> {}\n{}", emote, display_name, format_tag_detail(&tag_clone), added_line),
            format!("-# UUID: {dashed_uuid}"),
        ]),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ])
    .accent_color(COLOR_DANGER);

    send_tag_response(ctx, command, data, &player_info.uuid, container).await?;

    data.event_publisher
        .publish(&BlacklistEvent::TagRemoved {
            uuid: player_info.uuid.clone(),
            tag_id: tag_clone.id,
            removed_by: discord_id as i64,
        })
        .await;

    Ok(())
}


async fn run_change(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    command.defer_ephemeral(&ctx.http).await?;

    let discord_id = command.user.id.get();
    let (rank, _) = match require_linked_member(ctx, data, discord_id).await? {
        MemberCheck::Ok(r, m) => (r, m),
        MemberCheck::NotInGuild =>
            return send_deferred_error(ctx, command, "Error", "You must be in the Urchin server to use this command").await,
        MemberCheck::NotLinked =>
            return send_deferred_error(ctx, command, "Error", "You must link your account to modify tags").await,
    };
    let options = get_sub_options(command);
    let player = get_string(&options, "player");

    let player_info = match data.api.resolve(player).await {
        Ok(info) => info,
        Err(_) => return send_deferred_error(ctx, command, "Error", "Player not found").await,
    };

    let repo = BlacklistRepository::new(data.db.pool());
    if let Some(player_data) = repo.get_player(&player_info.uuid).await? {
        if player_data.is_locked {
            return send_deferred_error(ctx, command, "Error", "This player's tags are locked").await;
        }
    }

    let player_tags = repo.get_tags(&player_info.uuid).await?;
    if player_tags.is_empty() {
        return send_deferred_error(ctx, command, "Error", "This player has no tags to modify").await;
    }

    let is_staff = rank >= AccessRank::Helper;
    let now = chrono::Utc::now();

    let entries: Vec<TagChangeEntry> = player_tags
        .iter()
        .map(|tag| {
            let is_own = tag.added_by == discord_id as i64;
            let within_window = now.signed_duration_since(tag.added_on).num_minutes() <= 30;
            let editable = is_staff || (is_own && within_window);
            TagChangeEntry {
                tag_id: tag.id,
                original: tag.clone(),
                new_type: None,
                new_reason: None,
                claimed: false,
                hide: if blacklist::permissions::can_set_hide(rank.to_level()) { None } else { Some(false) },
                removed: false,
                editable,
            }
        })
        .collect();

    if !entries.iter().any(|e| e.editable) {
        return send_deferred_error(ctx, command, "Error", "You don't have permission to modify any of this player's tags").await;
    }

    let mut seen = std::collections::HashSet::new();
    let mut join_set = tokio::task::JoinSet::new();
    for &id in player_tags.iter().map(|t| &t.added_by).filter(|id| seen.insert(**id)) {
        let http = ctx.http.clone();
        join_set.spawn(async move {
            let name = http.get_user(UserId::new(id as u64)).await
                .map(|u| u.name.to_string())
                .unwrap_or_else(|_| id.to_string());
            (id, name)
        });
    }

    let mut resolved_names = HashMap::new();
    while let Some(Ok((id, name))) = join_set.join_next().await {
        resolved_names.insert(id, name);
    }

    let key = command.id.to_string();
    let pending = PendingTagChanges {
        uuid: player_info.uuid.clone(),
        username: player_info.username.clone(),
        owner_id: discord_id,
        owner_name: command.user.name.to_string(),
        is_staff,
        rank,
        entries,
        resolved_names,
        face_url: None,
    };

    let components = build_tag_change_menu(&pending, &key);
    data.pending_tag_changes.lock().unwrap().insert(key.clone(), pending);

    let mut resp = EditInteractionResponse::new()
        .flags(MessageFlags::IS_COMPONENTS_V2)
        .components(components);
    resp = resp.new_attachment(face_attachment(data, &player_info.uuid).await);
    let msg = command.edit_response(&ctx.http, resp).await?;

    let face_url = msg.attachments.iter()
        .find(|a| a.filename.as_str() == FACE_FILENAME)
        .map(|a| a.url.to_string());
    if let Some(url) = face_url {
        if let Some(p) = data.pending_tag_changes.lock().unwrap().get_mut(&key) {
            p.face_url = Some(url);
        }
    }
    Ok(())
}


async fn run_lock(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    command.defer_ephemeral(&ctx.http).await?;

    let discord_id = command.user.id.get();
    let rank = get_rank(data, discord_id).await?;
    if rank < AccessRank::Moderator {
        return send_deferred_error(ctx, command, "Error", "Only moderators can lock players").await;
    }

    let options = get_sub_options(command);
    let player = get_string(&options, "player");
    let reason = get_string(&options, "reason");

    let player_info = match data.api.resolve(player).await {
        Ok(info) => info,
        Err(_) => return send_deferred_error(ctx, command, "Error", "Player not found").await,
    };

    let repo = BlacklistRepository::new(data.db.pool());
    repo.lock_player(&player_info.uuid, reason, discord_id as i64).await?;

    let dashed_uuid = format_uuid_dashed(&player_info.uuid);
    let container = CreateContainer::new(vec![
        face_section(vec![
            format!("## {} Player Locked \u{1F512}\nIGN - `{}`", EMOTE_TAG, player_info.username),
            format!("> {}", sanitize_reason(reason)),
            format!("-# UUID: {dashed_uuid}"),
        ]),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ]);

    send_tag_response(ctx, command, data, &player_info.uuid, container).await?;

    data.event_publisher
        .publish(&BlacklistEvent::PlayerLocked {
            uuid: player_info.uuid.clone(),
            locked_by: discord_id as i64,
            reason: reason.to_string(),
        })
        .await;

    Ok(())
}


async fn run_unlock(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    command.defer_ephemeral(&ctx.http).await?;

    let discord_id = command.user.id.get();
    let rank = get_rank(data, discord_id).await?;
    if rank < AccessRank::Moderator {
        return send_deferred_error(ctx, command, "Error", "Only moderators can unlock players").await;
    }

    let options = get_sub_options(command);
    let player = get_string(&options, "player");

    let player_info = match data.api.resolve(player).await {
        Ok(info) => info,
        Err(_) => return send_deferred_error(ctx, command, "Error", "Player not found").await,
    };

    let repo = BlacklistRepository::new(data.db.pool());
    let unlocked = repo.unlock_player(&player_info.uuid).await?;
    let dashed_uuid = format_uuid_dashed(&player_info.uuid);
    let face = face_attachment(data, &player_info.uuid).await;

    if !unlocked {
        let container = CreateContainer::new(vec![
            face_section(vec![
                format!("## Not Locked\nIGN - `{}`", player_info.username),
                format!("-# UUID: {dashed_uuid}"),
            ]),
            CreateContainerComponent::Separator(CreateSeparator::new(true)),
        ]);
        let mut resp = EditInteractionResponse::new()
            .flags(MessageFlags::IS_COMPONENTS_V2)
            .components(container_response(container));
        resp = resp.new_attachment(face);
        command.edit_response(&ctx.http, resp).await?;
        return Ok(());
    }

    let container = CreateContainer::new(vec![
        face_section(vec![
            format!("## {} Player Unlocked \u{1F513}\nIGN - `{}`", EMOTE_TAG, player_info.username),
            format!("-# UUID: {dashed_uuid}"),
        ]),
        CreateContainerComponent::Separator(CreateSeparator::new(true)),
    ])
;

    let mut resp = EditInteractionResponse::new()
        .flags(MessageFlags::IS_COMPONENTS_V2)
        .components(container_response(container));
    resp = resp.new_attachment(face);
    command.edit_response(&ctx.http, resp).await?;

    data.event_publisher
        .publish(&BlacklistEvent::PlayerUnlocked {
            uuid: player_info.uuid.clone(),
            unlocked_by: discord_id as i64,
        })
        .await;

    Ok(())
}


fn parse_tc_id(custom_id: &str) -> Option<(String, usize)> {
    let parts: Vec<&str> = custom_id.splitn(3, ':').collect();
    if parts.len() != 3 { return None; }
    Some((parts[1].to_string(), parts[2].parse().ok()?))
}


fn build_tag_change_menu(pending: &PendingTagChanges, key: &str) -> Vec<CreateComponent<'static>> {
    let title = format!("## {} Manage Tags\nIGN - `{}`", EMOTE_EDITTAG, pending.username);
    let header = match &pending.face_url {
        Some(url) => CreateContainerComponent::Section(CreateSection::new(
            vec![CreateSectionComponent::TextDisplay(CreateTextDisplay::new(title))],
            CreateSectionAccessory::Thumbnail(CreateThumbnail::new(
                CreateUnfurledMediaItem::new(url.clone()),
            )),
        )),
        None => CreateContainerComponent::TextDisplay(CreateTextDisplay::new(title)),
    };
    let mut parts: Vec<CreateContainerComponent> = vec![header];

    for (idx, entry) in pending.entries.iter().enumerate() {
        parts.push(CreateContainerComponent::Separator(CreateSeparator::new(true)));

        if entry.removed {
            let def = lookup_tag(&entry.original.tag_type);
            let display = def.map(|d| d.display_name).unwrap_or(&entry.original.tag_type);
            parts.push(CreateContainerComponent::TextDisplay(CreateTextDisplay::new(
                format!("~~{}~~ — marked for removal", display),
            )));
            parts.push(CreateContainerComponent::ActionRow(CreateActionRow::buttons(vec![
                CreateButton::new(format!("tc_remove:{key}:{idx}"))
                    .label("Restore")
                    .style(ButtonStyle::Success),
            ])));
            continue;
        }

        let eff_type = entry.effective_type();
        let def = lookup_tag(eff_type);
        let emote = def.map(|d| d.emote).unwrap_or("");
        let display_name = def.map(|d| d.display_name).unwrap_or(eff_type);

        if entry.editable {
            let sibling_types: Vec<&str> = pending.entries.iter()
                .filter(|e| !e.removed)
                .map(|e| e.original.tag_type.as_str())
                .collect();
            let options = tc_type_options(eff_type, pending.rank, &sibling_types);
            if !options.is_empty() {
                let select = CreateSelectMenu::new(
                    format!("tc_type:{key}:{idx}"),
                    CreateSelectMenuKind::String { options: options.into() },
                )
                .placeholder(format!("Current: {display_name}"));
                parts.push(CreateContainerComponent::ActionRow(CreateActionRow::SelectMenu(select)));
            }
        }

        let eff_added_by = entry.effective_added_by(pending.owner_id);
        let added_name = pending.resolved_names.get(&eff_added_by)
            .map(|s| s.as_str())
            .unwrap_or(if entry.claimed { &pending.owner_name } else { "unknown" });

        let hide_label = if entry.effective_hide() { " *(hidden)*" } else { "" };
        let reason_text = if entry.original.tag_type == "replays_needed" {
            format_tag_detail(&entry.original)
        } else {
            sanitize_reason(entry.effective_reason())
        };

        let tag_display = format!(
            "{emote} {display_name}\n> {reason_text}\n> -# **\\- Added by `@{added_name}` <t:{}:R>**{hide_label}",
            entry.original.added_on.timestamp()
        );
        parts.push(CreateContainerComponent::TextDisplay(CreateTextDisplay::new(tag_display)));

        if entry.editable {
            let mut buttons = vec![
                CreateButton::new(format!("tc_reason:{key}:{idx}"))
                    .label("Edit Reason")
                    .style(ButtonStyle::Secondary),
            ];

            if blacklist::permissions::can_set_hide(pending.rank.to_level()) {
                let hide_btn = if entry.effective_hide() {
                    CreateButton::new(format!("tc_hide:{key}:{idx}"))
                        .label("Unhide")
                        .style(ButtonStyle::Secondary)
                } else {
                    CreateButton::new(format!("tc_hide:{key}:{idx}"))
                        .label("Hide")
                        .style(ButtonStyle::Secondary)
                };
                buttons.push(hide_btn);

                let claim_btn = if entry.claimed {
                    CreateButton::new(format!("tc_claim:{key}:{idx}"))
                        .label("Unclaim")
                        .style(ButtonStyle::Secondary)
                } else {
                    CreateButton::new(format!("tc_claim:{key}:{idx}"))
                        .label("Claim")
                        .style(ButtonStyle::Primary)
                };
                buttons.push(claim_btn);
            }

            if can_remove_tag(pending.rank, &entry.original, pending.owner_id) {
                buttons.push(
                    CreateButton::new(format!("tc_remove:{key}:{idx}"))
                        .label("Remove")
                        .style(ButtonStyle::Danger),
                );
            }

            parts.push(CreateContainerComponent::ActionRow(CreateActionRow::buttons(buttons)));
        }
    }

    let dashed_uuid = format_uuid_dashed(&pending.uuid);
    parts.push(CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!("-# UUID: {dashed_uuid}"))));
    parts.push(CreateContainerComponent::Separator(CreateSeparator::new(true)));

    let mut save_buttons = vec![
        CreateButton::new(format!("tc_save:{key}"))
            .label("Save Changes")
            .style(ButtonStyle::Success),
    ];
    if pending.is_staff {
        save_buttons.push(
            CreateButton::new(format!("tc_silent:{key}"))
                .label("Save Silently")
                .style(ButtonStyle::Secondary),
        );
    }
    parts.push(CreateContainerComponent::ActionRow(CreateActionRow::buttons(save_buttons)));

    vec![CreateComponent::Container(CreateContainer::new(parts))]
}


fn tc_type_options(current: &str, rank: AccessRank, sibling_types: &[&str]) -> Vec<CreateSelectMenuOption<'static>> {
    let level = rank.to_level();
    if !blacklist::permissions::can_modify(current, level, true, 0) {
        return vec![];
    }

    let current_pri = blacklist::lookup(current).map(|d| d.priority).unwrap_or(0);
    let blocked_priorities: Vec<u8> = sibling_types.iter()
        .filter(|&&t| t != current)
        .filter_map(|t| blacklist::lookup(t).map(|d| d.priority))
        .filter(|&p| p != current_pri)
        .collect();

    blacklist::user_addable()
        .iter()
        .filter(|tag| {
            tag.name != current
                && blacklist::permissions::can_change_to(tag.name, level)
                && !blocked_priorities.contains(&tag.priority)
        })
        .map(|tag| CreateSelectMenuOption::new(tag.display_name, tag.name))
        .collect()
}


fn can_remove_tag(rank: AccessRank, tag: &database::PlayerTagRow, user_id: u64) -> bool {
    let is_own = tag.added_by == user_id as i64;
    let age = chrono::Utc::now().signed_duration_since(tag.added_on).num_minutes();
    blacklist::permissions::can_remove(&tag.tag_type, rank.to_level(), is_own, age)
}


fn with_pending_entry(
    data: &Data,
    custom_id: &str,
    f: impl FnOnce(&mut TagChangeEntry),
) -> Result<Option<Vec<CreateComponent<'static>>>> {
    let Some((key, idx)) = parse_tc_id(custom_id) else { return Ok(None) };
    let mut map = data.pending_tag_changes.lock().unwrap();
    let Some(pending) = map.get_mut(&key) else { return Ok(None) };
    let Some(entry) = pending.entries.get_mut(idx) else { return Ok(None) };
    f(entry);
    Ok(Some(build_tag_change_menu(pending, &key)))
}


pub async fn handle_tc_type(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let new_type = match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => {
            values.first().map(|s| s.as_str()).unwrap_or("")
        }
        _ => return Ok(()),
    };

    let result = with_pending_entry(data, &component.data.custom_id, |entry| {
        if new_type == entry.original.tag_type {
            entry.new_type = None;
        } else {
            entry.new_type = Some(new_type.to_string());
            entry.claimed = true;
        }
    })?;

    match result {
        Some(components) => interact::update_message(ctx, component, components).await,
        None => send_component_message(ctx, component, "This session has expired").await,
    }
}


pub async fn handle_tc_reason(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    let payload = component.data.custom_id.splitn(2, ':').nth(1).unwrap_or("");

    let input = CreateInputText::new(InputTextStyle::Paragraph, "tc_reason")
        .placeholder("New reason for this tag")
        .max_length(120)
        .required(true);
    let modal = CreateModal::new(format!("tc_reason_modal:{payload}"), "Edit Tag Reason")
        .components(vec![CreateModalComponent::Label(CreateLabel::input_text("Reason", input))]);

    component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}


pub async fn handle_tc_reason_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
) -> Result<()> {
    let new_reason = interact::extract_modal_value(&modal.data.components, "tc_reason");

    let result = with_pending_entry(data, &modal.data.custom_id, |entry| {
        if new_reason == entry.original.reason {
            entry.new_reason = None;
        } else {
            entry.new_reason = Some(new_reason);
            entry.claimed = true;
        }
    })?;

    match result {
        Some(components) => interact::update_modal(ctx, modal, components).await,
        None => {
            modal.create_response(&ctx.http, CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new().content("This session has expired").ephemeral(true),
            )).await?;
            Ok(())
        }
    }
}


pub async fn handle_tc_hide(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let result = with_pending_entry(data, &component.data.custom_id, |entry| {
        let current = entry.effective_hide();
        entry.hide = Some(!current);
    })?;

    match result {
        Some(components) => interact::update_message(ctx, component, components).await,
        None => send_component_message(ctx, component, "This session has expired").await,
    }
}


pub async fn handle_tc_claim(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let result = with_pending_entry(data, &component.data.custom_id, |entry| {
        entry.claimed = !entry.claimed;
    })?;

    match result {
        Some(components) => interact::update_message(ctx, component, components).await,
        None => send_component_message(ctx, component, "This session has expired").await,
    }
}


pub async fn handle_tc_remove(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let result = with_pending_entry(data, &component.data.custom_id, |entry| {
        entry.removed = !entry.removed;
    })?;

    match result {
        Some(components) => interact::update_message(ctx, component, components).await,
        None => send_component_message(ctx, component, "This session has expired").await,
    }
}


pub async fn handle_tc_save(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
    silent: bool,
) -> Result<()> {
    let key = component.data.custom_id.splitn(2, ':').nth(1).unwrap_or("");
    let pending = data.pending_tag_changes.lock().unwrap().remove(key);

    let Some(pending) = pending else {
        return send_component_message(ctx, component, "This session has expired").await;
    };

    let repo = BlacklistRepository::new(data.db.pool());
    let cache = CacheRepository::new(data.db.pool());

    if let Some(player_data) = repo.get_player(&pending.uuid).await? {
        if player_data.is_locked {
            return send_component_message(ctx, component, "This player's tags are now locked").await;
        }
    }

    let mut changes_made = 0u32;
    let mut removals_made = 0u32;

    for entry in &pending.entries {
        if entry.removed && entry.editable {
            if !repo.remove_tag(entry.tag_id, pending.owner_id as i64).await? { continue; }
            removals_made += 1;
            if entry.original.tag_type == "confirmed_cheater" {
                try_archive_evidence(&repo, ctx, data, &pending.uuid).await;
            }
            data.event_publisher.publish(&BlacklistEvent::TagRemoved {
                uuid: pending.uuid.clone(),
                tag_id: entry.tag_id,
                removed_by: pending.owner_id as i64,
            }).await;
            continue;
        }

        if !entry.editable || !entry.has_changes(pending.owner_id) {
            continue;
        }

        let old_type = entry.original.tag_type.clone();
        let old_reason = entry.original.reason.clone();
        let new_type = entry.new_type.as_deref();
        let new_reason = entry.new_reason.as_deref();
        let new_added_by = entry.claimed.then(|| pending.owner_id as i64)
            .filter(|&id| id != entry.original.added_by);
        let new_hide = entry.hide.filter(|&h| h != entry.original.hide_username);

        let level = pending.rank.to_level();
        let is_own = entry.original.added_by == pending.owner_id as i64;
        let age = chrono::Utc::now().signed_duration_since(entry.original.added_on).num_minutes();

        if !blacklist::permissions::can_modify(&old_type, level, is_own, age) { continue; }

        if let Some(t) = new_type {
            if !blacklist::permissions::can_change_to(t, level) { continue; }
            let new_pri = blacklist::lookup(t).map(|d| d.priority).unwrap_or(0);
            let old_pri = blacklist::lookup(&old_type).map(|d| d.priority).unwrap_or(0);
            if new_pri != old_pri {
                let existing = repo.get_tags(&pending.uuid).await.unwrap_or_default();
                let conflict = existing.iter().any(|tag|
                    tag.id != entry.tag_id
                    && blacklist::lookup(&tag.tag_type).map(|d| d.priority).unwrap_or(255) == new_pri
                );
                if conflict { continue; }
            }
            if old_type == "confirmed_cheater" {
                try_archive_evidence(&repo, ctx, data, &pending.uuid).await;
            }
        }

        repo.modify_tag_full(entry.tag_id, new_type, new_reason, new_added_by, new_hide, new_added_by.is_some()).await?;
        changes_made += 1;

        if new_type.is_some() || new_reason.is_some() {
            let name = cache.get_username(&pending.uuid).await.ok().flatten()
                .unwrap_or_else(|| pending.username.clone());
            if silent {
                let new_tag = repo.get_tag_by_id(entry.tag_id).await?.unwrap_or(entry.original.clone());
                let old_tag = mock_change_tag(&entry.original, &old_type, &old_reason);
                channel::post_tag_changed(
                    ctx, data, &pending.uuid, &name, &old_tag, &new_tag,
                    "Tag Modified (Silent)", pending.owner_id as u64,
                ).await;
            } else {
                data.event_publisher.publish(&BlacklistEvent::TagEdited {
                    uuid: pending.uuid.clone(),
                    tag_id: entry.tag_id,
                    old_tag_type: old_type,
                    old_reason,
                    edited_by: pending.owner_id as i64,
                }).await;
            }
        }
    }

    if changes_made == 0 && removals_made == 0 {
        return send_component_message(ctx, component, "No changes to save").await;
    }

    let suffix = if silent { " (silent)" } else { "" };
    let msg = match (changes_made, removals_made) {
        (0, r) => format!("Removed {r} tag(s){suffix}"),
        (c, 0) => format!("Modified {c} tag(s){suffix}"),
        (c, r) => format!("Modified {c} tag(s), removed {r} tag(s){suffix}"),
    };

    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(container_response(simple_result(EMOTE_EDITTAG, &msg))),
            ),
        )
        .await?;

    Ok(())
}


async fn try_archive_evidence(
    repo: &BlacklistRepository<'_>,
    ctx: &Context,
    data: &Data,
    uuid: &str,
) {
    if let Ok(Some(p)) = repo.get_player(uuid).await {
        if let Some(url) = &p.evidence_thread {
            let _ = super::evidence::archive_evidence_by_url(ctx, data, url).await;
        }
    }
}


fn mock_change_tag(original: &database::PlayerTagRow, old_type: &str, old_reason: &str) -> database::PlayerTagRow {
    database::PlayerTagRow {
        id: original.id,
        player_id: original.player_id,
        tag_type: old_type.to_string(),
        reason: old_reason.to_string(),
        added_by: original.added_by,
        added_on: original.added_on,
        hide_username: original.hide_username,
        reviewed_by: original.reviewed_by.clone(),
        removed_by: original.removed_by,
        removed_on: original.removed_on,
        expires_at: original.expires_at,
    }
}


pub async fn handle_undo(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let tag_id = interact::parse_id(&component.data.custom_id).unwrap_or(0) as i64;
    let discord_id = component.user.id.get();
    let (rank, _) = get_rank_and_member(data, discord_id).await?;

    let ops = database::TagOp::new(data.db.pool());
    let (uuid, tag) = match ops.remove_by_id(tag_id, discord_id as i64, rank.to_level()).await {
        Ok(result) => result,
        Err(database::TagOpError::TagNotFound) =>
            return send_component_message(ctx, component, "Tag not found or already removed").await,
        Err(database::TagOpError::InsufficientPermissions) =>
            return send_component_message(ctx, component, "You don't have permission to remove this tag").await,
        Err(database::TagOpError::PlayerLocked) =>
            return send_component_message(ctx, component, "This player's tags are locked").await,
        Err(e) => return Err(anyhow::anyhow!("{e:?}")),
    };

    data.event_publisher
        .publish(&BlacklistEvent::TagRemoved { uuid, tag_id: tag.id, removed_by: discord_id as i64 })
        .await;

    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(container_response(simple_result(EMOTE_REMOVETAG, "Tag Removed"))),
            ),
        )
        .await?;

    Ok(())
}


pub async fn handle_edit(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let tag_id = interact::parse_id(&component.data.custom_id).unwrap_or(0) as i64;
    let discord_id = component.user.id.get();
    let rank = get_rank(data, discord_id).await?;

    let repo = BlacklistRepository::new(data.db.pool());
    let Some(tag) = repo.get_tag_by_id(tag_id).await? else {
        return send_component_message(ctx, component, "Tag not found or already removed").await;
    };
    if tag.added_by != discord_id as i64 && rank < AccessRank::Helper {
        return send_component_message(ctx, component, "You can only edit your own tags").await;
    }
    if rank < AccessRank::Helper {
        let age = chrono::Utc::now().signed_duration_since(tag.added_on);
        if age.num_minutes() > 30 {
            return send_component_message(ctx, component, "The 30-minute edit window has passed").await;
        }
    }

    let uuid = repo.get_uuid_by_player_id(tag.player_id).await.ok().flatten().unwrap_or_default();
    let siblings: Vec<String> = if !uuid.is_empty() {
        repo.get_tags(&uuid).await.unwrap_or_default().iter().map(|t| t.tag_type.clone()).collect()
    } else { vec![] };

    let select = CreateSelectMenu::new(
        format!("tag_edit_type:{tag_id}"),
        CreateSelectMenuKind::String {
            options: tag_choices_for_edit(&tag.tag_type, &siblings).into(),
        },
    )
    .placeholder("Change tag type");

    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(container_response(
                        CreateContainer::new(vec![
                            CreateContainerComponent::TextDisplay(CreateTextDisplay::new(format!(
                                "## {} Edit Tag", EMOTE_EDITTAG
                            ))),
                            CreateContainerComponent::ActionRow(CreateActionRow::SelectMenu(select)),
                            CreateContainerComponent::ActionRow(CreateActionRow::buttons(vec![
                                CreateButton::new(format!("tag_edit_reason:{tag_id}"))
                                    .label("Change Reason")
                                    .style(ButtonStyle::Secondary),
                                CreateButton::new(format!("tag_undo:{tag_id}"))
                                    .label("Remove")
                                    .style(ButtonStyle::Danger),
                            ])),
                        ])
                    )),
            ),
        )
        .await?;

    Ok(())
}


pub async fn handle_edit_type(
    ctx: &Context,
    component: &ComponentInteraction,
    data: &Data,
) -> Result<()> {
    let tag_id = interact::parse_id(&component.data.custom_id).unwrap_or(0) as i64;

    let new_type = match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => {
            values.first().map(|s| s.as_str()).unwrap_or("")
        }
        _ => return Ok(()),
    };

    let discord_id = component.user.id.get();
    let rank = get_rank(data, discord_id).await?;

    let repo = BlacklistRepository::new(data.db.pool());
    let Some(tag) = repo.get_tag_by_id(tag_id).await? else {
        return send_component_message(ctx, component, "Tag not found or already removed").await;
    };

    let level = rank.to_level();
    let is_own = tag.added_by == discord_id as i64;
    let age = chrono::Utc::now().signed_duration_since(tag.added_on).num_minutes();

    if !blacklist::permissions::can_modify(&tag.tag_type, level, is_own, age) {
        return send_component_message(ctx, component, "You don't have permission to modify this tag").await;
    }

    if new_type == "__revert" {
        if rank < AccessRank::Moderator {
            return send_component_message(
                ctx, component, "Only moderators and above can revert confirmed cheater tags",
            ).await;
        }

        let uuid = repo.get_uuid_by_player_id(tag.player_id).await.ok().flatten().unwrap_or_default();
        if !uuid.is_empty() {
            if let Some(player_data) = repo.get_player(&uuid).await? {
                if let Some(thread_url) = &player_data.evidence_thread {
                    super::evidence::archive_evidence_by_url(ctx, data, thread_url).await?;
                }
            }
        }

        let reverted_tag = repo.get_tag_by_id(tag_id).await?.unwrap_or(tag);
        let reverted_display = lookup_tag(&reverted_tag.tag_type)
            .map(|d| d.display_name)
            .unwrap_or(&reverted_tag.tag_type);

        component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .flags(MessageFlags::IS_COMPONENTS_V2)
                        .components(container_response(simple_result(
                            EMOTE_EDITTAG,
                            &format!("Tag Reverted\nReverted to **{reverted_display}**"),
                        ))),
                ),
            )
            .await?;
        return Ok(());
    }

    if !blacklist::permissions::can_change_to(new_type, level) {
        return send_component_message(ctx, component, "You don't have permission to change to this tag type").await;
    }

    let uuid = repo.get_uuid_by_player_id(tag.player_id).await.ok().flatten().unwrap_or_default();
    if !uuid.is_empty() {
        if let Some(player) = repo.get_player(&uuid).await? {
            if player.is_locked {
                return send_component_message(ctx, component, "This player's tags are locked").await;
            }
        }
    }

    let new_pri = blacklist::lookup(new_type).map(|d| d.priority).unwrap_or(0);
    let old_pri = blacklist::lookup(&tag.tag_type).map(|d| d.priority).unwrap_or(0);
    if new_pri != old_pri && !uuid.is_empty() {
        let existing = repo.get_tags(&uuid).await.unwrap_or_default();
        if existing.iter().any(|t| t.id != tag_id && blacklist::lookup(&t.tag_type).map(|d| d.priority).unwrap_or(255) == new_pri) {
            return send_component_message(ctx, component, "Cannot change — conflicts with an existing tag").await;
        }
    }

    let old_tag = tag.clone();
    repo.modify_tag(tag_id, Some(new_type), None).await?;
    if !uuid.is_empty() {
        data.event_publisher
            .publish(&BlacklistEvent::TagEdited {
                uuid,
                tag_id,
                old_tag_type: old_tag.tag_type.clone(),
                old_reason: old_tag.reason.clone(),
                edited_by: discord_id as i64,
            })
            .await;
    }

    let display = lookup_tag(new_type).map(|d| d.display_name).unwrap_or(new_type);
    component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::UpdateMessage(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(container_response(simple_result(
                        EMOTE_EDITTAG,
                        &format!("Tag Updated\nType changed to **{display}**"),
                    ))),
            ),
        )
        .await?;

    Ok(())
}


pub async fn handle_edit_reason(
    ctx: &Context,
    component: &ComponentInteraction,
    _data: &Data,
) -> Result<()> {
    let tag_id = interact::parse_id(&component.data.custom_id).unwrap_or(0);

    let input = CreateInputText::new(InputTextStyle::Paragraph, "tag_reason")
        .placeholder("New reason for this tag")
        .required(true);
    let modal = CreateModal::new(format!("tag_edit_reason_modal:{tag_id}"), "Edit Tag Reason")
        .components(vec![CreateModalComponent::Label(CreateLabel::input_text("Reason", input))]);

    component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await?;
    Ok(())
}


pub async fn handle_edit_reason_modal(
    ctx: &Context,
    modal: &ModalInteraction,
    data: &Data,
) -> Result<()> {
    let tag_id = interact::parse_id(&modal.data.custom_id).unwrap_or(0) as i64;
    let new_reason = crate::interact::extract_modal_value(&modal.data.components, "tag_reason");
    let discord_id = modal.user.id.get();
    let rank = get_rank(data, discord_id).await?;

    let repo = BlacklistRepository::new(data.db.pool());
    let Some(tag) = repo.get_tag_by_id(tag_id).await? else {
        modal
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new().content("Tag not found or already removed").ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    };

    if tag.added_by != discord_id as i64 && rank < AccessRank::Helper {
        modal
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new().content("Insufficient permissions").ephemeral(true),
                ),
            )
            .await?;
        return Ok(());
    }

    let old_tag = tag.clone();
    repo.modify_tag(tag_id, None, Some(&new_reason)).await?;

    let uuid = repo.get_uuid_by_player_id(tag.player_id).await.ok().flatten().unwrap_or_default();
    if !uuid.is_empty() {
        data.event_publisher
            .publish(&BlacklistEvent::TagEdited {
                uuid,
                tag_id,
                old_tag_type: old_tag.tag_type.clone(),
                old_reason: old_tag.reason.clone(),
                edited_by: discord_id as i64,
            })
            .await;
    }

    modal
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2)
                    .components(container_response(simple_result(EMOTE_EDITTAG, "Reason Updated")))
                    .ephemeral(true),
            ),
        )
        .await?;

    Ok(())
}


fn tag_choices_for_edit(current: &str, sibling_types: &[String]) -> Vec<CreateSelectMenuOption<'static>> {
    let current_pri = blacklist::lookup(current).map(|d| d.priority).unwrap_or(0);
    let blocked: Vec<u8> = sibling_types.iter()
        .filter(|t| t.as_str() != current)
        .filter_map(|t| blacklist::lookup(t).map(|d| d.priority))
        .filter(|&p| p != current_pri)
        .collect();

    let mut options: Vec<CreateSelectMenuOption<'static>> = blacklist::user_addable()
        .iter()
        .filter(|tag| tag.name != current && !blocked.contains(&tag.priority))
        .map(|tag| CreateSelectMenuOption::new(tag.display_name, tag.name))
        .collect();

    if current == "confirmed_cheater" {
        options.insert(0, CreateSelectMenuOption::new("Revert to Unconfirmed", "__revert"));
    }

    options
}


async fn send_component_message(
    ctx: &Context,
    component: &ComponentInteraction,
    message: &str,
) -> Result<()> {
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
