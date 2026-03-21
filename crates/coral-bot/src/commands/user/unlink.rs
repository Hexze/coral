use anyhow::Result;
use serenity::all::{CommandInteraction, Context, CreateCommand};

use crate::framework::Data;

pub fn register() -> CreateCommand<'static> {
    CreateCommand::new("unlink").description("Manage or unlink your Minecraft accounts")
}

pub async fn run(ctx: &Context, command: &CommandInteraction, data: &Data) -> Result<()> {
    super::link::run(ctx, command, data).await
}
