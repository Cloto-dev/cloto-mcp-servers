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
    Ready(ReadyData),
    Resumed,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct MessageData {
    pub guild_id: Option<String>,
    pub channel_id: String,
    pub message_id: String,
    pub author_id: String,
    pub author_name: String,
    pub author_bot: bool,
    pub content: String,
    pub timestamp: String,
    pub attachments: Vec<AttachmentData>,
    pub reference: Option<ReferenceData>,
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
pub struct ReadyData {
    pub username: String,
    pub bot_user_id: u64,
    pub guild_count: usize,
}

pub struct DiscordHandler {
    pub event_tx: mpsc::UnboundedSender<DiscordEvent>,
    pub allowed_channel_ids: Vec<u64>,
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

        // Channel filter
        if !self.allowed_channel_ids.is_empty()
            && !self.allowed_channel_ids.contains(&msg.channel_id.get())
        {
            return;
        }

        // Only respond to messages that mention the bot
        let bot_id = ctx.cache.current_user().id;
        if !msg.mentions_user_id(bot_id) {
            return;
        }

        // Strip bot mention from content so LLM doesn't see <@ID>
        let clean_content = utils::strip_bot_mention(&msg.content, bot_id.get());

        let reference = msg
            .referenced_message
            .as_ref()
            .map(|referenced| ReferenceData {
                author_name: referenced.author.name.clone(),
                content: referenced.content.clone(),
            });

        let data = MessageData {
            guild_id: msg.guild_id.map(|id| id.to_string()),
            channel_id: msg.channel_id.to_string(),
            message_id: msg.id.to_string(),
            author_id: msg.author.id.to_string(),
            author_name: msg
                .author_nick(&ctx.http)
                .await
                .unwrap_or_else(|| msg.author.name.clone()),
            author_bot: msg.author.bot,
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
        };

        let _ = self
            .event_tx
            .send(DiscordEvent::MessageCreate(Box::new(data)));
    }

    async fn ready(&self, ctx: serenity::Context, ready: serenity::Ready) {
        tracing::info!(
            "Discord connected as {} ({})",
            ready.user.name,
            ready.user.id
        );

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
}
