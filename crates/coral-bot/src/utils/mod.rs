use serenity::all::*;

use database::CacheRepository;

use crate::framework::Data;

mod minecraft;

pub use minecraft::{format_number, format_tag_detail, format_uuid_dashed, generate_api_key, sanitize_reason};


pub async fn resolve_username(uuid: &str, data: &Data) -> Option<String> {
    CacheRepository::new(data.db.pool()).get_username(uuid).await.ok().flatten()
}


pub fn text(s: impl Into<String>) -> CreateContainerComponent<'static> {
    CreateContainerComponent::TextDisplay(CreateTextDisplay::new(s.into()))
}


pub fn separator() -> CreateContainerComponent<'static> {
    CreateContainerComponent::Separator(CreateSeparator::new(true))
}
