use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serenity::all::{
    ChannelId, Command, ComponentInteraction, Context, CreateCommand, CreateComponent,
    CreateContainer, CreateInteractionResponse, CreateInteractionResponseMessage, EventHandler,
    FullEvent, InstallationContext, Interaction, InteractionContext, MessageFlags,
    ModalInteraction, UserId,
};
use serenity::async_trait;

use clients::SkinProvider;
use coral_redis::EventPublisher;
use database::{Database, Member};

use mc_verify::VerifyHandle;

use crate::api::CoralApiClient;
use crate::commands;
use crate::commands::blacklist::tag::PendingOverwrite;
use crate::commands::stats::bedwars::BedwarsCache;
use crate::commands::stats::session::SessionCache;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccessRank {
    Default = 0,
    Member = 1,
    Helper = 2,
    Moderator = 3,
    Admin = 4,
    Owner = 5,
}

impl AccessRank {
    pub fn of(data: &Data, user_id: u64, member: Option<&Member>) -> Self {
        if data.is_owner(user_id) {
            return Self::Owner;
        }
        match member.map(|m| m.access_level) {
            Some(4..) => Self::Admin,
            Some(3) => Self::Moderator,
            Some(2) => Self::Helper,
            Some(1) => Self::Member,
            _ => Self::Default,
        }
    }

    pub fn from_level(level: i16) -> Self {
        match level {
            4.. => Self::Admin,
            3 => Self::Moderator,
            2 => Self::Helper,
            1 => Self::Member,
            _ => Self::Default,
        }
    }

    pub fn to_level(self) -> i16 {
        match self {
            Self::Admin => 4,
            Self::Moderator => 3,
            Self::Helper => 2,
            Self::Member => 1,
            _ => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Owner => "Owner",
            Self::Admin => "Admin",
            Self::Moderator => "Moderator",
            Self::Helper => "Helper",
            Self::Member => "Member",
            Self::Default => "Default",
        }
    }
}

#[derive(Clone)]
pub struct Data {
    pub db: Arc<Database>,
    pub api: Arc<CoralApiClient>,
    pub skin_provider: Arc<dyn SkinProvider>,
    pub owner_ids: Vec<u64>,
    pub blacklist_channel_id: Option<ChannelId>,
    pub mod_channel_id: Option<ChannelId>,
    pub review_forum_id: Option<ChannelId>,
    pub evidence_forum_id: Option<ChannelId>,
    pub redis_url: String,
    pub event_publisher: EventPublisher,
    pub bedwars_images: Arc<Mutex<HashMap<String, BedwarsCache>>>,
    pub session_images: Arc<Mutex<HashMap<String, SessionCache>>>,
    pub pending_overwrites: Arc<Mutex<HashMap<String, PendingOverwrite>>>,
    pub verify_handle: VerifyHandle,
    pub sync_cooldowns: Arc<Mutex<HashMap<UserId, Instant>>>,
}

impl Data {
    pub fn is_owner(&self, user_id: u64) -> bool {
        self.owner_ids.contains(&user_id)
    }
}

fn strip_panel_prefix(id: &str) -> Option<&str> {
    id.strip_prefix("manage_")
        .or_else(|| id.strip_prefix("dashboard_"))
        .or_else(|| id.strip_prefix("link_"))
}

pub struct Handler {
    data: Data,
}

impl Handler {
    pub fn new(data: Data) -> Self {
        Self { data }
    }

    fn commands() -> Vec<CreateCommand<'static>> {
        let commands = vec![
            commands::blacklist::tag::register(),
            commands::stats::bedwars::register(),
            commands::stats::prestiges::register(),
            commands::stats::session::register(),
            commands::user::link::register(),
            commands::user::unlink::register(),
            commands::user::dashboard::register(),
            commands::admin::info::register(),
            commands::admin::ban::register(),
            commands::admin::manage::register(),
            commands::admin::setup::register(),
            commands::admin::strike::register(),
            commands::blacklist::evidence::register(),
        ];

        commands
            .into_iter()
            .map(|cmd| {
                cmd.integration_types(vec![InstallationContext::Guild, InstallationContext::User])
                    .contexts(vec![
                        InteractionContext::Guild,
                        InteractionContext::BotDm,
                        InteractionContext::PrivateChannel,
                    ])
            })
            .collect()
    }

    async fn handle_command(
        &self,
        ctx: &Context,
        command: &serenity::all::CommandInteraction,
    ) -> anyhow::Result<()> {
        match command.data.name.as_str() {
            "tag" => commands::blacklist::tag::run(ctx, command, &self.data).await,
            "bedwars" => commands::stats::bedwars::run(ctx, command, &self.data).await,
            "prestiges" => commands::stats::prestiges::run(ctx, command, &self.data).await,
            "session" => commands::stats::session::run(ctx, command, &self.data).await,
            "link" => commands::user::link::run(ctx, command, &self.data).await,
            "unlink" => commands::user::unlink::run(ctx, command, &self.data).await,
            "dashboard" => commands::user::dashboard::run(ctx, command, &self.data).await,
            "info" => commands::admin::info::run(ctx, command, &self.data).await,
            "ban" => commands::admin::ban::run(ctx, command, &self.data).await,
            "manage" => commands::admin::manage::run(ctx, command, &self.data).await,
            "setup" => commands::admin::setup::run(ctx, command, &self.data).await,
            "strike" => commands::admin::strike::run(ctx, command, &self.data).await,
            "confirm" => commands::blacklist::evidence::run(ctx, command, &self.data).await,
            _ => Ok(()),
        }
    }

    async fn handle_component(
        &self,
        ctx: &Context,
        component: &ComponentInteraction,
    ) -> anyhow::Result<()> {
        let id = component.data.custom_id.as_str();
        tracing::debug!("component interaction: {id}");

        if let Some(action) = strip_panel_prefix(id) {
            return match action {
                _ if action.starts_with("swap_primary:") => {
                    commands::admin::accounts_panel::handle_swap_primary(ctx, component, &self.data)
                        .await
                }
                _ if action.starts_with("link_new:") => {
                    commands::admin::accounts_panel::handle_link_new(ctx, component, &self.data)
                        .await
                }
                _ if action.starts_with("link_pick:") => {
                    commands::admin::accounts_panel::handle_link_pick(ctx, component, &self.data)
                        .await
                }
                _ if action.starts_with("add_account:") => {
                    commands::admin::accounts_panel::handle_add_account_button(ctx, component).await
                }
                _ if action.starts_with("add_code:") => {
                    commands::admin::accounts_panel::handle_add_code_button(ctx, component).await
                }
                _ if action.starts_with("remove_account:") => {
                    commands::admin::accounts_panel::handle_remove_account(
                        ctx, component, &self.data,
                    )
                    .await
                }
                _ if action.starts_with("force_add:") => {
                    commands::admin::accounts_panel::handle_force_add(ctx, component, &self.data)
                        .await
                }
                _ if action.starts_with("cancel_add:") => {
                    commands::admin::accounts_panel::handle_cancel_add(ctx, component, &self.data)
                        .await
                }
                _ if action.starts_with("accounts_back:") => {
                    commands::admin::accounts_panel::handle_accounts_back_generic(
                        ctx, component, &self.data,
                    )
                    .await
                }
                _ => self.handle_component_direct(ctx, component, id).await,
            };
        }

        self.handle_component_direct(ctx, component, id).await
    }

    async fn handle_component_direct(
        &self,
        ctx: &Context,
        component: &ComponentInteraction,
        id: &str,
    ) -> anyhow::Result<()> {
        match id {
            "regenerate_key" => {
                commands::user::dashboard::handle_regenerate_key(ctx, component, &self.data).await
            }
            "confirm_regenerate_key" => {
                commands::user::dashboard::handle_confirm_regenerate_key(ctx, component, &self.data)
                    .await
            }
            "setup_link" => {
                commands::admin::setup::handle_link_button(ctx, component, &self.data).await
            }
            _ if id.starts_with("dashboard_accounts_back:") => {
                commands::admin::accounts_panel::handle_dashboard_accounts_back(
                    ctx, component, &self.data,
                )
                .await
            }
            _ if id.starts_with("dashboard_accounts:") => {
                commands::admin::accounts_panel::handle_dashboard_accounts_button(
                    ctx, component, &self.data,
                )
                .await
            }
            "bedwars_mode" => {
                commands::stats::bedwars::handle_mode_switch(ctx, component, &self.data).await
            }
            "session_mode" => {
                commands::stats::session::handle_mode_switch(ctx, component, &self.data).await
            }
            "session_switch" => {
                commands::stats::session::handle_switch(ctx, component, &self.data).await
            }
            _ if id.starts_with("session_mgmt_rename:") => {
                commands::stats::session::handle_mgmt_rename_button(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("session_mgmt_delete:") => {
                commands::stats::session::handle_mgmt_delete_button(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("tag_overwrite:") => {
                commands::blacklist::tag::handle_overwrite_button(ctx, component, &self.data).await
            }
            _ if id.starts_with("tag_undo:") => {
                commands::blacklist::tag::handle_undo(ctx, component, &self.data).await
            }
            _ if id.starts_with("tag_edit:") => {
                commands::blacklist::tag::handle_edit(ctx, component, &self.data).await
            }
            _ if id.starts_with("tag_edit_type:") => {
                commands::blacklist::tag::handle_edit_type(ctx, component, &self.data).await
            }
            _ if id.starts_with("tag_edit_reason:") => {
                commands::blacklist::tag::handle_edit_reason(ctx, component, &self.data).await
            }
            _ if id.starts_with("evidence_add_media") => {
                commands::blacklist::evidence::handle_add_media(ctx, component, &self.data).await
            }
            _ if id.starts_with("evidence_cancel_upload") => {
                commands::blacklist::evidence::handle_cancel_upload(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("evidence_remove") => {
                commands::blacklist::evidence::handle_remove(ctx, component, &self.data).await
            }
            _ if id.starts_with("evidence_archive") => {
                commands::blacklist::evidence::handle_archive(ctx, component, &self.data).await
            }
            _ if id.starts_with("manage_access_select:") => {
                commands::admin::manage::handle_access_select(ctx, component, &self.data).await
            }
            _ if id.starts_with("manage_lock:") => {
                commands::admin::manage::handle_lock_button(ctx, component, &self.data).await
            }
            _ if id.starts_with("manage_unlock:") => {
                commands::admin::manage::handle_unlock_button(ctx, component, &self.data).await
            }
            _ if id.starts_with("manage_accounts_back:") => {
                commands::admin::accounts_panel::handle_manage_accounts_back(
                    ctx, component, &self.data,
                )
                .await
            }
            _ if id.starts_with("manage_accounts:") => {
                commands::admin::accounts_panel::handle_accounts_button(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("manage_force_link:") => {
                commands::admin::manage::handle_force_link(ctx, component, &self.data).await
            }
            _ if id.starts_with("manage_toggle_tagging:") => {
                commands::admin::manage::handle_toggle_tagging(ctx, component, &self.data).await
            }
            _ if id.starts_with("manage_remove_strike:") => {
                commands::admin::manage::handle_remove_strike(ctx, component, &self.data).await
            }
            _ if id.starts_with("manage_register:") => {
                commands::admin::manage::handle_register_button(ctx, component, &self.data).await
            }
            _ if id.starts_with("review_add_replay:") => {
                commands::blacklist::reviews::handle_add_replay(ctx, component, &self.data).await
            }
            _ if id.starts_with("review_add_attachment:") => {
                commands::blacklist::reviews::handle_add_attachment(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("review_cancel_attachment:") => {
                commands::blacklist::reviews::handle_cancel_attachment(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("review_remove_player:") => {
                commands::blacklist::reviews::handle_remove_player(ctx, component, &self.data).await
            }
            _ if id.starts_with("review_remove_evidence:") => {
                commands::blacklist::reviews::handle_remove_evidence(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("review_tag_select_add:") => {
                commands::blacklist::reviews::handle_tag_select_add(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("review_tag_select_edit:") => {
                commands::blacklist::reviews::handle_tag_select_edit(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("review_edit_submitted:") => {
                commands::blacklist::reviews::handle_edit_submitted(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("review_submit:") => {
                commands::blacklist::reviews::handle_submit(ctx, component, &self.data).await
            }
            _ if id.starts_with("review_approve:") => {
                commands::blacklist::reviews::handle_approve(ctx, component, &self.data).await
            }
            _ if id.starts_with("review_reject:") => {
                commands::blacklist::reviews::handle_reject(ctx, component, &self.data).await
            }
            _ if id.starts_with("review_confirm:") => {
                commands::blacklist::reviews::handle_confirm(ctx, component, &self.data).await
            }
            _ if id.starts_with("review_cancel_thread:") => {
                commands::blacklist::reviews::handle_cancel_thread(ctx, component, &self.data).await
            }
            _ if id.starts_with("review_abort_delete:") => {
                commands::blacklist::reviews::handle_abort_delete(ctx, component, &self.data).await
            }
            _ if id.starts_with("review_cancel:") => {
                commands::blacklist::reviews::handle_cancel(ctx, component, &self.data).await
            }
            _ if id.starts_with("setup_link_role_select:") => {
                commands::admin::setup::handle_link_role_select(ctx, component, &self.data).await
            }
            _ if id.starts_with("setup_unlinked_role_select:") => {
                commands::admin::setup::handle_unlinked_role_select(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("setup_nickname_edit:") => {
                commands::admin::setup::handle_nickname_edit_button(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("setup_nickname_clear:") => {
                commands::admin::setup::handle_nickname_clear_button(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("setup_nickname:") => {
                commands::admin::setup::handle_nickname_button(ctx, component, &self.data).await
            }
            _ if id.starts_with("setup_link_channel_select:") => {
                commands::admin::setup::handle_link_channel_select(ctx, component, &self.data).await
            }
            _ if id.starts_with("setup_autorole:") => {
                commands::admin::setup::handle_autorole_button(ctx, component, &self.data).await
            }
            _ if id.starts_with("setup_role_config:") => {
                commands::admin::setup::handle_role_config_select(ctx, component, &self.data).await
            }
            _ if id.starts_with("setup_condition_edit:") => {
                commands::admin::setup::handle_condition_edit_button(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("setup_rule_edit:") => {
                commands::admin::setup::handle_rule_edit_button(ctx, component, &self.data).await
            }
            _ if id.starts_with("setup_rule_remove:") => {
                commands::admin::setup::handle_rule_remove_button(ctx, component, &self.data).await
            }
            _ if id.starts_with("setup_role_strip:") => {
                commands::admin::setup::handle_role_strip_button(ctx, component, &self.data).await
            }
            _ if id.starts_with("setup_nickname_reset:") => {
                commands::admin::setup::handle_nickname_reset_button(ctx, component, &self.data)
                    .await
            }
            _ if id.starts_with("setup_autorole_back:") => {
                commands::admin::setup::handle_cancel_button(ctx, component, &self.data).await
            }
            _ if id.starts_with("setup_autorole_cancel:") => {
                commands::admin::setup::handle_autorole_button(ctx, component, &self.data).await
            }
            _ if id.starts_with("setup_cancel:") => {
                commands::admin::setup::handle_cancel_button(ctx, component, &self.data).await
            }
            _ => {
                tracing::warn!("unhandled component interaction: {id}");
                Ok(())
            }
        }
    }

    async fn handle_modal(&self, ctx: &Context, modal: &ModalInteraction) -> anyhow::Result<()> {
        let id = modal.data.custom_id.as_str();

        match id {
            _ if id.starts_with("session_rename_modal:") => {
                commands::stats::session::handle_rename_modal(ctx, modal, &self.data).await
            }
            _ if id.starts_with("review_player_modal:") => {
                commands::blacklist::reviews::handle_player_modal(ctx, modal, &self.data).await
            }
            _ if id.starts_with("review_replay_modal:") => {
                commands::blacklist::reviews::handle_replay_modal(ctx, modal, &self.data).await
            }
            _ if id.starts_with("review_reject_modal:") => {
                commands::blacklist::reviews::handle_reject_modal(ctx, modal, &self.data).await
            }
            _ if id.starts_with("review_edit_player_modal:") => {
                commands::blacklist::reviews::handle_edit_player_modal(ctx, modal, &self.data).await
            }
            _ if id.starts_with("setup_nickname_modal:") => {
                commands::admin::setup::handle_nickname_modal(ctx, modal, &self.data).await
            }
            _ if id.starts_with("setup_add_rule_modal:") => {
                commands::admin::setup::handle_add_rule_modal(ctx, modal, &self.data).await
            }
            _ if id.starts_with("setup_rule_edit_modal:") => {
                commands::admin::setup::handle_rule_edit_modal(ctx, modal, &self.data).await
            }
            _ if id.starts_with("manage_register_modal:") => {
                commands::admin::manage::handle_register_modal(ctx, modal, &self.data).await
            }
            _ if strip_panel_prefix(id).is_some_and(|a| a.starts_with("add_account_modal:")) => {
                commands::admin::accounts_panel::handle_add_account_modal(ctx, modal, &self.data)
                    .await
            }
            _ if strip_panel_prefix(id).is_some_and(|a| a.starts_with("add_code_modal:")) => {
                commands::admin::accounts_panel::handle_add_code_modal(ctx, modal, &self.data).await
            }
            _ if id.starts_with("tag_edit_reason_modal:") => {
                commands::blacklist::tag::handle_edit_reason_modal(ctx, modal, &self.data).await
            }
            _ => Ok(()),
        }
    }

    async fn handle_interaction(&self, ctx: &Context, interaction: Interaction) {
        let result = match &interaction {
            Interaction::Command(command) => self.handle_command(ctx, command).await,
            Interaction::Component(component) => self.handle_component(ctx, component).await,
            Interaction::Modal(modal) => self.handle_modal(ctx, modal).await,
            _ => return,
        };

        if let Err(e) = result {
            tracing::error!("Interaction error: {e}");

            let container = CreateComponent::Container(
                CreateContainer::new(vec![crate::utils::text(
                    "## Something went wrong\nAn unexpected error occurred. Please try again later.",
                )])
                .accent_color(crate::commands::blacklist::channel::COLOR_ERROR),
            );

            let response = CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .flags(MessageFlags::IS_COMPONENTS_V2 | MessageFlags::EPHEMERAL)
                    .components(vec![container]),
            );

            let _ = match interaction {
                Interaction::Command(cmd) => cmd.create_response(&ctx.http, response).await,
                Interaction::Component(cmp) => cmp.create_response(&ctx.http, response).await,
                Interaction::Modal(modal) => modal.create_response(&ctx.http, response).await,
                _ => Ok(()),
            };
        }
    }
}

#[async_trait]
impl EventHandler for Handler {
    async fn dispatch(&self, ctx: &Context, event: &FullEvent) {
        match event {
            FullEvent::Ready { data_about_bot, .. } => {
                tracing::info!("Bot connected as {}", data_about_bot.user.name);

                match Command::set_global_commands(&ctx.http, &Self::commands()).await {
                    Ok(cmds) => tracing::info!("Registered {} global commands", cmds.len()),
                    Err(e) => tracing::error!("Failed to register global commands: {}", e),
                }

                crate::events::spawn_subscriber(ctx.clone(), self.data.clone());
            }
            FullEvent::InteractionCreate { interaction, .. } => {
                self.handle_interaction(ctx, interaction.clone()).await;
            }
            FullEvent::GuildMemberAddition { new_member, .. } => {
                if let Err(e) =
                    commands::user::link::handle_guild_join(ctx, new_member, &self.data).await
                {
                    tracing::error!("Guild join handler error: {}", e);
                }
            }
            FullEvent::GuildMemberUpdate {
                new: Some(member), ..
            } => {
                if member.user.id != ctx.cache.current_user().id {
                    let ctx = ctx.clone();
                    let data = self.data.clone();
                    let member = member.clone();
                    tokio::spawn(async move {
                        crate::sync::handle_member_update(&ctx, &data, &member).await;
                    });
                }
            }
            FullEvent::Message { new_message, .. } => {
                crate::sync::handle_message_activity(ctx, &self.data, new_message);

                let has_attachments = !new_message.attachments.is_empty()
                    || new_message
                        .message_snapshots
                        .iter()
                        .any(|s| !s.attachments.is_empty());

                if !new_message.author.bot() && has_attachments {
                    let ctx2 = ctx.clone();
                    let msg = new_message.clone();
                    let data = self.data.clone();
                    tokio::spawn(async move {
                        if let Err(e) = commands::blacklist::reviews::handle_attachment_message(
                            &ctx2, &msg, &data,
                        )
                        .await
                        {
                            tracing::error!("Review attachment capture error: {}", e);
                        }

                        if let Err(e) = commands::blacklist::evidence::handle_attachment_message(
                            &ctx2, &msg, &data,
                        )
                        .await
                        {
                            tracing::error!("Evidence attachment capture error: {}", e);
                        }
                    });
                }
            }
            _ => {}
        }
    }
}
