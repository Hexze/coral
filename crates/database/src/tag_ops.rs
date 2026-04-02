use chrono::{DateTime, Utc};
use sqlx::PgPool;

use blacklist::permissions;

use crate::blacklist::{BlacklistRepository, PlayerTagRow};


#[derive(Debug)]
pub enum TagOpError {
    TaggingDisabled,
    PlayerLocked,
    InsufficientPermissions,
    InvalidTagType,
    TagAlreadyExists,
    PriorityConflict(PlayerTagRow),
    TagNotFound,
    Database(sqlx::Error),
}


impl From<sqlx::Error> for TagOpError {
    fn from(e: sqlx::Error) -> Self { Self::Database(e) }
}


pub struct TagOp<'a> {
    repo: BlacklistRepository<'a>,
}


impl<'a> TagOp<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { repo: BlacklistRepository::new(pool) }
    }

    pub async fn add(
        &self,
        uuid: &str,
        tag_type: &str,
        reason: &str,
        actor_id: i64,
        actor_level: i16,
        hide_username: bool,
        reviewed_by: Option<&[i64]>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<PlayerTagRow, TagOpError> {
        if !blacklist::lookup(tag_type).is_some() {
            return Err(TagOpError::InvalidTagType);
        }
        if !permissions::can_add(tag_type, actor_level) {
            return Err(TagOpError::InsufficientPermissions);
        }
        self.check_lock(uuid).await?;

        let hide = if permissions::can_set_hide(actor_level) { hide_username } else { false };

        if let Some(conflict) = self.find_priority_conflict(uuid, tag_type).await? {
            if conflict.tag_type == tag_type {
                return Err(TagOpError::TagAlreadyExists);
            }
            return Err(TagOpError::PriorityConflict(conflict));
        }

        self.repo.add_tag_with_expiry(uuid, tag_type, reason, actor_id, hide, reviewed_by, expires_at).await?;

        self.repo.get_tag_by_type(uuid, tag_type).await?
            .ok_or(TagOpError::Database(sqlx::Error::RowNotFound))
    }

    pub async fn remove(
        &self,
        uuid: &str,
        tag_type: &str,
        actor_id: i64,
        actor_level: i16,
    ) -> Result<PlayerTagRow, TagOpError> {
        let tag = self.require_tag(uuid, tag_type).await?;
        self.check_lock(uuid).await?;

        let is_own = tag.added_by == actor_id;
        let age = Utc::now().signed_duration_since(tag.added_on).num_minutes();

        if !permissions::can_remove(&tag.tag_type, actor_level, is_own, age) {
            return Err(TagOpError::InsufficientPermissions);
        }

        self.repo.remove_tag(tag.id, actor_id).await?;
        Ok(tag)
    }

    pub async fn modify(
        &self,
        uuid: &str,
        tag_type: &str,
        actor_id: i64,
        actor_level: i16,
        new_reason: Option<&str>,
        new_hide: Option<bool>,
    ) -> Result<PlayerTagRow, TagOpError> {
        let tag = self.require_tag(uuid, tag_type).await?;
        self.check_lock(uuid).await?;

        let is_own = tag.added_by == actor_id;
        let age = Utc::now().signed_duration_since(tag.added_on).num_minutes();

        if !permissions::can_modify(&tag.tag_type, actor_level, is_own, age) {
            return Err(TagOpError::InsufficientPermissions);
        }

        let hide = if permissions::can_set_hide(actor_level) { new_hide } else { None };

        self.repo.update_tag(uuid, tag_type, new_reason, hide).await?
            .ok_or(TagOpError::TagNotFound)
    }

    pub async fn change_type(
        &self,
        uuid: &str,
        old_type: &str,
        new_type: &str,
        actor_id: i64,
        actor_level: i16,
    ) -> Result<PlayerTagRow, TagOpError> {
        let tag = self.require_tag(uuid, old_type).await?;
        self.check_lock(uuid).await?;

        let is_own = tag.added_by == actor_id;
        let age = Utc::now().signed_duration_since(tag.added_on).num_minutes();

        if !permissions::can_modify(old_type, actor_level, is_own, age) {
            return Err(TagOpError::InsufficientPermissions);
        }
        if !permissions::can_change_to(new_type, actor_level) {
            return Err(TagOpError::InsufficientPermissions);
        }
        if blacklist::lookup(new_type).is_none() {
            return Err(TagOpError::InvalidTagType);
        }

        let new_priority = blacklist::lookup(new_type).unwrap().priority;
        let old_priority = blacklist::lookup(old_type).map(|d| d.priority).unwrap_or(0);
        if new_priority != old_priority {
            if let Some(conflict) = self.find_priority_conflict(uuid, new_type).await? {
                return Err(TagOpError::PriorityConflict(conflict));
            }
        }

        self.repo.modify_tag(tag.id, Some(new_type), None).await?;

        self.repo.get_tag_by_id(tag.id).await?
            .ok_or(TagOpError::TagNotFound)
    }

    pub async fn overwrite(
        &self,
        uuid: &str,
        old_tag_id: i64,
        new_type: &str,
        reason: &str,
        actor_id: i64,
        actor_level: i16,
        hide_username: bool,
    ) -> Result<(PlayerTagRow, PlayerTagRow), TagOpError> {
        if !permissions::can_add(new_type, actor_level) {
            return Err(TagOpError::InsufficientPermissions);
        }
        self.check_lock(uuid).await?;

        let old_tag = self.repo.get_tag_by_id(old_tag_id).await?
            .ok_or(TagOpError::TagNotFound)?;

        let is_own = old_tag.added_by == actor_id;
        let age = Utc::now().signed_duration_since(old_tag.added_on).num_minutes();

        if !permissions::can_remove(&old_tag.tag_type, actor_level, is_own, age) {
            return Err(TagOpError::InsufficientPermissions);
        }

        let hide = if permissions::can_set_hide(actor_level) { hide_username } else { false };

        self.repo.remove_tag(old_tag_id, actor_id).await?;
        self.repo.add_tag(uuid, new_type, reason, actor_id, hide, None).await?;

        let new_tag = self.repo.get_tag_by_type(uuid, new_type).await?
            .ok_or(TagOpError::Database(sqlx::Error::RowNotFound))?;

        Ok((old_tag, new_tag))
    }

    pub async fn remove_by_id(
        &self,
        tag_id: i64,
        actor_id: i64,
        actor_level: i16,
    ) -> Result<(String, PlayerTagRow), TagOpError> {
        let tag = self.repo.get_tag_by_id(tag_id).await?
            .ok_or(TagOpError::TagNotFound)?;

        let uuid = self.repo.get_uuid_by_player_id(tag.player_id).await?
            .ok_or(TagOpError::TagNotFound)?;

        self.check_lock(&uuid).await?;

        let is_own = tag.added_by == actor_id;
        let age = Utc::now().signed_duration_since(tag.added_on).num_minutes();

        if !permissions::can_remove(&tag.tag_type, actor_level, is_own, age) {
            return Err(TagOpError::InsufficientPermissions);
        }

        self.repo.remove_tag(tag_id, actor_id).await?;
        Ok((uuid, tag))
    }

    pub fn repo(&self) -> &BlacklistRepository<'a> { &self.repo }


    async fn check_lock(&self, uuid: &str) -> Result<(), TagOpError> {
        if let Some(player) = self.repo.get_player(uuid).await? {
            if player.is_locked { return Err(TagOpError::PlayerLocked); }
        }
        Ok(())
    }

    async fn require_tag(&self, uuid: &str, tag_type: &str) -> Result<PlayerTagRow, TagOpError> {
        self.repo.get_tag_by_type(uuid, tag_type).await?
            .ok_or(TagOpError::TagNotFound)
    }

    async fn find_priority_conflict(&self, uuid: &str, tag_type: &str) -> Result<Option<PlayerTagRow>, TagOpError> {
        let new_priority = blacklist::lookup(tag_type).map(|d| d.priority).unwrap_or(0);
        let tags = self.repo.get_tags(uuid).await?;
        Ok(tags.into_iter().find(|t| {
            blacklist::lookup(&t.tag_type).map(|d| d.priority).unwrap_or(255) == new_priority
        }))
    }
}
