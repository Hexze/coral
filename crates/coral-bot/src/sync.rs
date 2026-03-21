use std::collections::HashMap as StdHashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use serde_json::Value;
use serenity::all::{Context, EditMember, GuildId, Member, Message, RoleId, UserId};
use serenity::nonmax::NonMaxU16;

use database::{
    BlacklistRepository, CacheRepository, GuildConfig, GuildConfigRepository, GuildRoleRule,
    MemberRepository,
};

use crate::expr;
use crate::framework::Data;

pub const NICKNAME_MAX_LEN: usize = 32;
const NICKNAME_SEPARATOR: &str = " | ";

pub fn build_nickname(prefix: &str, current_nick: Option<&str>) -> String {
    if prefix.is_empty() {
        return String::new();
    }

    let Some(current) = current_nick else {
        return prefix.to_string();
    };

    if current.starts_with(prefix) {
        return truncate_nick(current, NICKNAME_MAX_LEN);
    }

    let custom = current
        .strip_prefix(prefix.split(NICKNAME_SEPARATOR).next().unwrap_or(prefix))
        .map(|rest| rest.trim_start_matches(NICKNAME_SEPARATOR).trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(current.trim());

    if custom.is_empty() || prefix.len() + NICKNAME_SEPARATOR.len() >= NICKNAME_MAX_LEN {
        return prefix.to_string();
    }

    let full = format!("{prefix}{NICKNAME_SEPARATOR}{custom}");
    truncate_nick(&full, NICKNAME_MAX_LEN)
}

fn truncate_nick(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    let mut end = max_len;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].trim_end().to_string()
}

pub(crate) fn build_template_context(
    hypixel_data: &Value,
    member: &Member,
    active_tags: &[String],
) -> Value {
    let mut ctx = hypixel_data.clone();

    let display_name = member
        .user
        .global_name
        .as_deref()
        .unwrap_or(&member.user.name);

    ctx["discord"] = serde_json::json!({
        "name": display_name,
    });

    let highest = active_tags
        .iter()
        .filter_map(|t| blacklist::lookup(t).map(|def| (def.priority, t.as_str())))
        .min_by_key(|(p, _)| *p)
        .map(|(_, name)| name);

    let mut bl = serde_json::json!({ "tag": highest });
    for def in blacklist::all() {
        bl[def.name] = Value::Bool(active_tags.iter().any(|t| t == def.name));
    }
    ctx["blacklist"] = bl;

    ctx
}

pub(crate) async fn active_tags(data: &Data, uuid: &str) -> Vec<String> {
    let repo = BlacklistRepository::new(data.db.pool());
    repo.get_tags(uuid)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row| row.tag_type)
        .collect()
}

const REFRESH_THRESHOLD: Duration = Duration::from_secs(4 * 3600);
const MEMBERS_PER_PAGE: u16 = 1000;
const RATE_LIMIT_DELAY: Duration = Duration::from_millis(1100);

pub async fn handle_member_update(ctx: &Context, data: &Data, member: &Member) {
    let guild_id = member.guild_id;
    let discord_id = member.user.id.get() as i64;

    let config_repo = GuildConfigRepository::new(data.db.pool());
    let config = match config_repo.get(guild_id.get() as i64).await {
        Ok(Some(c)) => c,
        _ => return,
    };

    let rules = config_repo
        .get_role_rules(guild_id.get() as i64)
        .await
        .unwrap_or_default();

    if config.nickname_template.is_none() && rules.is_empty() {
        return;
    }

    let members = MemberRepository::new(data.db.pool());
    let uuid = match members
        .get_by_discord_id(discord_id)
        .await
        .ok()
        .flatten()
        .and_then(|m| m.uuid)
    {
        Some(uuid) => uuid,
        None => return,
    };

    let cache = CacheRepository::new(data.db.pool());
    let hypixel_data = match cache.get_latest_snapshot(&uuid).await.ok().flatten() {
        Some(d) => d,
        None => return,
    };

    if let Err(e) = sync_member(
        ctx,
        data,
        guild_id,
        member,
        &uuid,
        &config,
        &rules,
        &hypixel_data,
        true,
    )
    .await
    {
        tracing::debug!(
            "Failed to sync member {} in {guild_id}: {e}",
            member.user.id
        );
    }
}

pub fn handle_message_activity(ctx: &Context, data: &Data, message: &Message) {
    if message.author.bot() {
        return;
    }

    let Some(guild_id) = message.guild_id else {
        return;
    };

    let user_id = message.author.id;

    if is_on_cooldown(data, user_id) {
        return;
    }

    let ctx = ctx.clone();
    let data = data.clone();
    tokio::spawn(async move {
        if let Err(e) = try_sync_from_message(&ctx, &data, guild_id, user_id).await {
            tracing::warn!("Sync from message failed for {user_id} in {guild_id}: {e}");
        }
    });
}

pub async fn sync_user(ctx: Context, data: Data, user_id: UserId) {
    let members = MemberRepository::new(data.db.pool());
    let uuid = match members
        .get_by_discord_id(user_id.get() as i64)
        .await
        .ok()
        .flatten()
        .and_then(|m| m.uuid)
    {
        Some(uuid) => uuid,
        None => return,
    };

    let hypixel_data = match resolve_hypixel_data(&data, &uuid).await {
        Some(hd) => hd,
        None => return,
    };

    let config_repo = GuildConfigRepository::new(data.db.pool());
    let configs = match config_repo.get_all().await {
        Ok(c) => c,
        Err(_) => return,
    };

    for config in configs {
        let guild_id = GuildId::new(config.guild_id as u64);
        let member = match guild_id.member(&ctx.http, user_id).await {
            Ok(m) => m,
            Err(_) => continue,
        };

        let rules = config_repo
            .get_role_rules(config.guild_id)
            .await
            .unwrap_or_default();

        if let Err(e) = sync_member(
            &ctx,
            &data,
            guild_id,
            &member,
            &uuid,
            &config,
            &rules,
            &hypixel_data,
            false,
        )
        .await
        {
            tracing::warn!("User sync failed for {} in {guild_id}: {e}", user_id.get());
        }
    }
}

pub async fn sync_guild(ctx: Context, data: Data, guild_id: GuildId) {
    if let Err(e) = try_sync_guild(&ctx, &data, guild_id).await {
        tracing::warn!("Guild sync failed for {guild_id}: {e}");
    }
}

async fn try_sync_from_message(
    ctx: &Context,
    data: &Data,
    guild_id: GuildId,
    user_id: UserId,
) -> Result<()> {
    set_cooldown(data, user_id);

    let config_repo = GuildConfigRepository::new(data.db.pool());
    let config = match config_repo.get(guild_id.get() as i64).await? {
        Some(config) => config,
        None => return Ok(()),
    };

    let rules = config_repo.get_role_rules(guild_id.get() as i64).await?;

    if config.nickname_template.is_none() && rules.is_empty() {
        return Ok(());
    }

    let members = MemberRepository::new(data.db.pool());
    let uuid = match members
        .get_by_discord_id(user_id.get() as i64)
        .await?
        .and_then(|m| m.uuid)
    {
        Some(uuid) => uuid,
        None => return Ok(()),
    };

    let hypixel_data = match resolve_hypixel_data(data, &uuid).await {
        Some(hd) => hd,
        None => return Ok(()),
    };

    let member = guild_id.member(&ctx.http, user_id).await?;
    sync_member(
        ctx,
        data,
        guild_id,
        &member,
        &uuid,
        &config,
        &rules,
        &hypixel_data,
        true,
    )
    .await?;

    Ok(())
}

async fn try_sync_guild(ctx: &Context, data: &Data, guild_id: GuildId) -> Result<()> {
    let config_repo = GuildConfigRepository::new(data.db.pool());
    let config = match config_repo.get(guild_id.get() as i64).await? {
        Some(config) => config,
        None => return Ok(()),
    };
    let rules = config_repo.get_role_rules(guild_id.get() as i64).await?;

    let mut after = None;
    let mut updates = 0usize;
    let mut total = 0usize;

    loop {
        let page_limit = NonMaxU16::new(MEMBERS_PER_PAGE).unwrap();
        let chunk = guild_id.members(&ctx.http, Some(page_limit), after).await?;
        if chunk.is_empty() {
            break;
        }

        after = chunk.last().map(|m| m.user.id);
        let (page_total, page_updates) =
            sync_member_batch(ctx, data, guild_id, &chunk, &config, &rules).await;

        total += page_total;
        updates += page_updates;
    }

    tracing::info!("Guild sync {guild_id}: {updates}/{total} members updated");
    Ok(())
}

async fn sync_member_batch(
    ctx: &Context,
    data: &Data,
    guild_id: GuildId,
    members: &[Member],
    config: &GuildConfig,
    rules: &[GuildRoleRule],
) -> (usize, usize) {
    let members_repo = MemberRepository::new(data.db.pool());
    let cache = CacheRepository::new(data.db.pool());

    let discord_ids: Vec<i64> = members.iter().map(|m| m.user.id.get() as i64).collect();
    let linked = members_repo
        .get_linked_by_discord_ids(&discord_ids)
        .await
        .unwrap_or_default();

    let uuid_map: StdHashMap<i64, String> = linked
        .into_iter()
        .filter_map(|m| m.uuid.map(|uuid| (m.discord_id, uuid)))
        .collect();

    let mut total = 0;
    let mut updates = 0;

    for member in members {
        if member.user.bot() {
            continue;
        }

        total += 1;

        let result = match uuid_map.get(&(member.user.id.get() as i64)) {
            Some(uuid) => {
                let hypixel_data = cache.get_latest_snapshot(uuid).await.ok().flatten();
                match hypixel_data {
                    Some(data_val) => {
                        sync_member(
                            ctx, data, guild_id, member, uuid, config, rules, &data_val, false,
                        )
                        .await
                    }
                    None => sync_unlinked_member(ctx, guild_id, member, config, rules).await,
                }
            }
            None => sync_unlinked_member(ctx, guild_id, member, config, rules).await,
        };

        match result {
            Ok(true) => {
                updates += 1;
                tokio::time::sleep(RATE_LIMIT_DELAY).await;
            }
            Ok(false) => {}
            Err(e) => tracing::debug!("Sync failed for {} in {guild_id}: {e}", member.user.id),
        }
    }

    (total, updates)
}

pub(crate) async fn sync_member(
    ctx: &Context,
    data: &Data,
    guild_id: GuildId,
    member: &Member,
    uuid: &str,
    config: &GuildConfig,
    rules: &[GuildRoleRule],
    hypixel_data: &Value,
    preserve_custom: bool,
) -> Result<bool> {
    let tags = active_tags(data, uuid).await;
    let template_ctx = build_template_context(hypixel_data, member, &tags);

    let mut roles: Vec<RoleId> = member.roles.iter().copied().collect();
    let original_roles = roles.clone();

    if let Some(id) = config.link_role_id {
        let role = RoleId::new(id as u64);
        if !roles.contains(&role) {
            roles.push(role);
        }
    }

    if let Some(id) = config.unlinked_role_id {
        let role = RoleId::new(id as u64);
        roles.retain(|r| *r != role);
    }

    for rule in rules {
        let role = RoleId::new(rule.role_id as u64);
        let matches = expr::eval_condition(&rule.condition, &template_ctx).unwrap_or(false);
        if matches && !roles.contains(&role) {
            roles.push(role);
        } else if !matches {
            roles.retain(|r| *r != role);
        }
    }

    let roles_changed = roles != original_roles;

    let nickname = config.nickname_template.as_ref().and_then(|template| {
        let prefix = expr::render_template(template, &template_ctx).to_truncated(NICKNAME_MAX_LEN);
        let nick = if preserve_custom {
            build_nickname(&prefix, member.nick.as_deref())
        } else {
            prefix
        };
        if !nick.is_empty() && member.nick.as_deref() != Some(&nick) {
            Some(nick)
        } else {
            None
        }
    });

    if !roles_changed && nickname.is_none() {
        return Ok(false);
    }

    let mut edit = EditMember::new();
    if roles_changed {
        edit = edit.roles(&roles);
    }
    if let Some(ref nick) = nickname {
        edit = edit.nickname(nick);
    }

    guild_id
        .edit_member(&ctx.http, member.user.id, edit)
        .await?;
    Ok(true)
}

async fn sync_unlinked_member(
    ctx: &Context,
    guild_id: GuildId,
    member: &Member,
    config: &GuildConfig,
    rules: &[GuildRoleRule],
) -> Result<bool> {
    let mut roles: Vec<RoleId> = member.roles.iter().copied().collect();
    let original_roles = roles.clone();

    if let Some(id) = config.unlinked_role_id {
        let role = RoleId::new(id as u64);
        if !roles.contains(&role) {
            roles.push(role);
        }
    }

    if let Some(id) = config.link_role_id {
        let role = RoleId::new(id as u64);
        roles.retain(|r| *r != role);
    }

    for rule in rules {
        let role = RoleId::new(rule.role_id as u64);
        roles.retain(|r| *r != role);
    }

    if roles == original_roles {
        return Ok(false);
    }

    guild_id
        .edit_member(&ctx.http, member.user.id, EditMember::new().roles(&roles))
        .await?;
    Ok(true)
}

pub async fn clear_nicknames(ctx: Context, data: Data, guild_id: GuildId) {
    if let Err(e) = try_clear_nicknames(&ctx, &data, guild_id).await {
        tracing::warn!("Failed to clear nicknames for {guild_id}: {e}");
    }
}

async fn try_clear_nicknames(ctx: &Context, data: &Data, guild_id: GuildId) -> Result<()> {
    let mut after = None;
    let mut cleared = 0usize;

    loop {
        let config = GuildConfigRepository::new(data.db.pool())
            .get(guild_id.get() as i64)
            .await?;
        if config
            .as_ref()
            .and_then(|c| c.nickname_template.as_ref())
            .is_some()
        {
            tracing::info!("Nickname reset cancelled — new template set in {guild_id}");
            return Ok(());
        }

        let page_limit = serenity::nonmax::NonMaxU16::new(MEMBERS_PER_PAGE).unwrap();
        let chunk = guild_id.members(&ctx.http, Some(page_limit), after).await?;
        if chunk.is_empty() {
            break;
        }

        after = chunk.last().map(|m| m.user.id);

        for member in &chunk {
            if member.nick.is_some() && !member.user.bot() {
                let _ = guild_id
                    .edit_member(&ctx.http, member.user.id, EditMember::new().nickname(""))
                    .await;
                cleared += 1;
                tokio::time::sleep(RATE_LIMIT_DELAY).await;
            }
        }
    }

    tracing::info!("Cleared {cleared} nicknames in {guild_id}");
    Ok(())
}

pub async fn clear_role(ctx: Context, guild_id: GuildId, role_id: RoleId) {
    if let Err(e) = try_clear_role(&ctx, guild_id, role_id).await {
        tracing::warn!("Failed to clear role {role_id} in {guild_id}: {e}");
    }
}

async fn try_clear_role(ctx: &Context, guild_id: GuildId, role_id: RoleId) -> Result<()> {
    let mut after = None;
    let mut removed = 0usize;

    loop {
        let page_limit = serenity::nonmax::NonMaxU16::new(MEMBERS_PER_PAGE).unwrap();
        let chunk = guild_id.members(&ctx.http, Some(page_limit), after).await?;
        if chunk.is_empty() {
            break;
        }

        after = chunk.last().map(|m| m.user.id);

        for member in &chunk {
            if member.roles.contains(&role_id) {
                let _ = member.remove_role(&ctx.http, role_id, None).await;
                removed += 1;
                tokio::time::sleep(RATE_LIMIT_DELAY).await;
            }
        }
    }

    tracing::info!("Removed role {role_id} from {removed} members in {guild_id}");
    Ok(())
}

pub async fn swap_role(
    ctx: Context,
    data: Data,
    guild_id: GuildId,
    old_role: Option<RoleId>,
    new_role: Option<RoleId>,
    config_field: RoleConfigField,
) {
    if let Err(e) = try_swap_role(&ctx, &data, guild_id, old_role, new_role, config_field).await {
        tracing::warn!("Failed to swap role in {guild_id}: {e}");
    }
}

#[derive(Clone, Copy)]
pub enum RoleConfigField {
    Link,
    Unlinked,
}

async fn try_swap_role(
    ctx: &Context,
    data: &Data,
    guild_id: GuildId,
    old_role: Option<RoleId>,
    new_role: Option<RoleId>,
    config_field: RoleConfigField,
) -> Result<()> {
    if old_role == new_role {
        return Ok(());
    }

    let mut after = None;
    let mut changed = 0usize;

    loop {
        let current_config = GuildConfigRepository::new(data.db.pool())
            .get(guild_id.get() as i64)
            .await?;
        let current_role_id = current_config.as_ref().and_then(|c| match config_field {
            RoleConfigField::Link => c.link_role_id,
            RoleConfigField::Unlinked => c.unlinked_role_id,
        });
        if current_role_id != new_role.map(|r| r.get() as i64) {
            tracing::info!("Role swap cancelled — config changed in {guild_id}");
            return Ok(());
        }

        let page_limit = serenity::nonmax::NonMaxU16::new(MEMBERS_PER_PAGE).unwrap();
        let chunk = guild_id.members(&ctx.http, Some(page_limit), after).await?;
        if chunk.is_empty() {
            break;
        }

        after = chunk.last().map(|m| m.user.id);

        for member in &chunk {
            if member.user.bot() {
                continue;
            }

            if let Some(old) = old_role {
                if member.roles.contains(&old) {
                    let _ = member.remove_role(&ctx.http, old, None).await;
                    changed += 1;
                    tokio::time::sleep(RATE_LIMIT_DELAY).await;
                }
            }
        }
    }

    tracing::info!("Role swap complete in {guild_id}: {changed} members updated");
    Ok(())
}

async fn resolve_hypixel_data(data: &Data, uuid: &str) -> Option<Value> {
    let cache = CacheRepository::new(data.db.pool());

    if is_snapshot_stale(&cache, uuid).await {
        match data.api.get_player_stats(uuid).await {
            Ok(response) => return response.hypixel,
            Err(e) => tracing::debug!("Hypixel refresh failed for {uuid}, using cache: {e}"),
        }
    }

    cache.get_latest_snapshot(uuid).await.ok().flatten()
}

async fn is_snapshot_stale(cache: &CacheRepository<'_>, uuid: &str) -> bool {
    match cache.get_latest_timestamp(uuid).await.ok().flatten() {
        Some(timestamp) => {
            (Utc::now() - timestamp).num_seconds() > REFRESH_THRESHOLD.as_secs() as i64
        }
        None => true,
    }
}

fn is_on_cooldown(data: &Data, user_id: UserId) -> bool {
    let cooldowns = data.sync_cooldowns.lock().unwrap();
    cooldowns
        .get(&user_id)
        .is_some_and(|last| last.elapsed() < REFRESH_THRESHOLD)
}

fn set_cooldown(data: &Data, user_id: UserId) {
    let mut cooldowns = data.sync_cooldowns.lock().unwrap();
    cooldowns.retain(|_, last| last.elapsed() < REFRESH_THRESHOLD);
    cooldowns.insert(user_id, Instant::now());
}
