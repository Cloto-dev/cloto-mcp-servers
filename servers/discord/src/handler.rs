//! Serenity EventHandler → tokio mpsc bridge.
//!
//! Converts Discord Gateway events into `DiscordEvent` structs and sends them
//! to the main loop via an unbounded channel. The main loop then emits MGP
//! notifications on stdout.

use crate::utils;
use serenity::all as serenity;
use serenity::async_trait;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Events forwarded from Discord Gateway to the main loop.
#[derive(Debug)]
pub enum DiscordEvent {
    MessageCreate(Box<MessageData>),
    InteractionCreate(Box<InteractionData>),
    Ready(ReadyData),
    Resumed,
    ShardStageUpdate { old: String, new: String },
}

/// Data extracted from a slash command interaction.
#[derive(Debug)]
pub struct InteractionData {
    pub interaction_id: String,
    pub interaction_token: String,
    pub guild_id: Option<String>,
    pub guild_name: Option<String>,
    pub channel_id: String,
    pub author_id: String,
    pub author_name: String,
    pub author_roles: Vec<String>,
    pub command_name: String,
    pub message: String,
    pub timestamp: String,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct MessageData {
    pub guild_id: Option<String>,
    pub guild_name: Option<String>,
    pub channel_id: String,
    pub message_id: String,
    pub author_id: String,
    pub author_name: String,
    pub author_bot: bool,
    pub author_roles: Vec<String>,
    pub content: String,
    pub timestamp: String,
    pub attachments: Vec<AttachmentData>,
    pub reference: Option<ReferenceData>,
    pub thread_info: Option<ThreadInfo>,
}

#[derive(Debug)]
pub struct AttachmentData {
    pub url: String,
    pub filename: String,
    pub size: u64,
    pub content_type: Option<String>,
}

#[derive(Debug)]
pub struct ReferenceData {
    pub author_name: String,
    pub content: String,
}

#[derive(Debug)]
pub struct ThreadInfo {
    pub parent_id: String,
    pub thread_name: String,
    pub archived: bool,
}

#[derive(Debug)]
pub struct ReadyData {
    pub username: String,
    pub bot_user_id: u64,
    pub guild_count: usize,
}

pub struct DiscordHandler {
    pub event_tx: mpsc::UnboundedSender<DiscordEvent>,
    pub allowed_channel_ids: Vec<u64>,
    /// Discord user IDs authorized for backtick direct tool commands.
    pub direct_tool_users: Vec<u64>,
    /// Shared slot to store Serenity Context for presence updates.
    pub bot_context: Arc<std::sync::Mutex<Option<serenity::Context>>>,
}

#[async_trait]
impl serenity::EventHandler for DiscordHandler {
    async fn message(&self, ctx: serenity::Context, msg: serenity::Message) {
        // Skip bot messages to prevent loops
        if msg.author.bot {
            return;
        }

        // Resolve thread info from guild cache (before channel filter for parent check)
        let thread_info = msg.guild_id.and_then(|gid| {
            ctx.cache.guild(gid).and_then(|guild| {
                let ch = guild.channels.get(&msg.channel_id)?;
                match ch.kind {
                    serenity::ChannelType::PublicThread
                    | serenity::ChannelType::PrivateThread
                    | serenity::ChannelType::NewsThread => Some(ThreadInfo {
                        parent_id: ch.parent_id.map(|p| p.to_string()).unwrap_or_default(),
                        thread_name: ch.name.clone(),
                        archived: ch.thread_metadata.as_ref().is_some_and(|tm| tm.archived),
                    }),
                    _ => None,
                }
            })
        });

        // Channel filter: check channel_id OR parent_id for threads
        if !self.allowed_channel_ids.is_empty() {
            let channel_allowed = self.allowed_channel_ids.contains(&msg.channel_id.get());
            let parent_allowed = thread_info.as_ref().is_some_and(|ti| {
                ti.parent_id
                    .parse::<u64>()
                    .is_ok_and(|pid| self.allowed_channel_ids.contains(&pid))
            });
            if !channel_allowed && !parent_allowed {
                return;
            }
        }

        // Direct tool commands bypass mention requirement
        let is_direct_command = !self.direct_tool_users.is_empty()
            && self.direct_tool_users.contains(&msg.author.id.get())
            && msg.content.trim().starts_with('`')
            && !msg.content.trim().starts_with("```")
            && msg.content.trim().ends_with('`');

        if !is_direct_command {
            // Only respond to messages that mention the bot
            let bot_id = ctx.cache.current_user().id;
            if !msg.mentions_user_id(bot_id) {
                return;
            }
        }

        // Strip bot mention from content so LLM doesn't see <@ID>
        let bot_id = ctx.cache.current_user().id;
        let clean_content = utils::strip_bot_mention(&msg.content, bot_id.get());

        let reference = msg
            .referenced_message
            .as_ref()
            .map(|referenced| ReferenceData {
                author_name: referenced.author.name.clone(),
                content: referenced.content.clone(),
            });

        // Resolve author roles from cache
        let author_roles = msg.member.as_ref().map_or_else(Vec::new, |member| {
            msg.guild_id
                .and_then(|gid| ctx.cache.guild(gid))
                .map_or_else(Vec::new, |guild| {
                    member
                        .roles
                        .iter()
                        .filter_map(|rid| guild.roles.get(rid).map(|r| r.name.clone()))
                        .collect()
                })
        });

        // Resolve guild name from cache
        let guild_name = msg
            .guild_id
            .and_then(|gid| ctx.cache.guild(gid).map(|g| g.name.clone()));

        let data = MessageData {
            guild_id: msg.guild_id.map(|id| id.to_string()),
            guild_name,
            channel_id: msg.channel_id.to_string(),
            message_id: msg.id.to_string(),
            author_id: msg.author.id.to_string(),
            author_name: msg
                .author_nick(&ctx.http)
                .await
                .unwrap_or_else(|| msg.author.name.clone()),
            author_bot: msg.author.bot,
            author_roles,
            content: clean_content,
            timestamp: msg.timestamp.to_string(),
            attachments: msg
                .attachments
                .iter()
                .map(|a| AttachmentData {
                    url: a.url.clone(),
                    filename: a.filename.clone(),
                    size: a.size as u64,
                    content_type: a.content_type.clone(),
                })
                .collect(),
            reference,
            thread_info,
        };

        let _ = self
            .event_tx
            .send(DiscordEvent::MessageCreate(Box::new(data)));
    }

    async fn interaction_create(&self, ctx: serenity::Context, interaction: serenity::Interaction) {
        let serenity::Interaction::Command(cmd) = interaction else {
            return;
        };

        // Channel filter
        if !self.allowed_channel_ids.is_empty()
            && !self.allowed_channel_ids.contains(&cmd.channel_id.get())
        {
            return;
        }

        // Extract message from command options
        let message = cmd
            .data
            .options
            .iter()
            .find(|o| o.name == "message")
            .and_then(|o| o.value.as_str())
            .unwrap_or("")
            .to_string();

        // Resolve author info
        let author_name = cmd
            .member
            .as_ref()
            .and_then(|m| m.nick.clone())
            .unwrap_or_else(|| cmd.user.name.clone());

        let author_roles = cmd.member.as_ref().map_or_else(Vec::new, |member| {
            cmd.guild_id
                .and_then(|gid| ctx.cache.guild(gid))
                .map_or_else(Vec::new, |guild| {
                    member
                        .roles
                        .iter()
                        .filter_map(|rid| guild.roles.get(rid).map(|r| r.name.clone()))
                        .collect()
                })
        });

        let guild_name = cmd
            .guild_id
            .and_then(|gid| ctx.cache.guild(gid).map(|g| g.name.clone()));

        let data = InteractionData {
            interaction_id: cmd.id.to_string(),
            interaction_token: cmd.token.clone(),
            guild_id: cmd.guild_id.map(|id| id.to_string()),
            guild_name,
            channel_id: cmd.channel_id.to_string(),
            author_id: cmd.user.id.to_string(),
            author_name,
            author_roles,
            command_name: cmd.data.name.clone(),
            message,
            timestamp: cmd.id.created_at().to_string(),
        };

        // Defer the response (shows "thinking..." to the user)
        let builder = serenity::CreateInteractionResponse::Defer(
            serenity::CreateInteractionResponseMessage::new(),
        );
        if let Err(e) = cmd.create_response(&ctx.http, builder).await {
            tracing::error!("Failed to defer interaction: {e}");
            return;
        }

        let _ = self
            .event_tx
            .send(DiscordEvent::InteractionCreate(Box::new(data)));
    }

    async fn ready(&self, ctx: serenity::Context, ready: serenity::Ready) {
        tracing::info!(
            "Discord connected as {} ({})",
            ready.user.name,
            ready.user.id
        );

        // Register slash commands
        let commands = vec![
            serenity::CreateCommand::new("chat")
                .description("Talk to the bot")
                .add_option(
                    serenity::CreateCommandOption::new(
                        serenity::CommandOptionType::String,
                        "message",
                        "Your message",
                    )
                    .required(true),
                ),
            serenity::CreateCommand::new("status")
                .description("Show bridge status"),
        ];

        if let Err(e) = serenity::Command::set_global_commands(&ctx.http, commands).await {
            tracing::error!("Failed to register slash commands: {e}");
        } else {
            tracing::info!("Slash commands registered globally");
        }

        // Store context for presence management tool
        if let Ok(mut slot) = self.bot_context.lock() {
            *slot = Some(ctx);
        }

        let data = ReadyData {
            username: ready.user.name.clone(),
            bot_user_id: ready.user.id.get(),
            guild_count: ready.guilds.len(),
        };
        let _ = self.event_tx.send(DiscordEvent::Ready(data));
    }

    async fn resume(&self, _ctx: serenity::Context, _: serenity::ResumedEvent) {
        tracing::info!("Discord Gateway resumed");
        let _ = self.event_tx.send(DiscordEvent::Resumed);
    }

    async fn shard_stage_update(
        &self,
        _ctx: serenity::Context,
        event: serenity::ShardStageUpdateEvent,
    ) {
        let _ = self.event_tx.send(DiscordEvent::ShardStageUpdate {
            old: format!("{:?}", event.old),
            new: format!("{:?}", event.new),
        });
    }
}
