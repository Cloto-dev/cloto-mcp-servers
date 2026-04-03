//! Environment-based configuration for the Discord MGP server.

use std::env;

#[derive(Debug, Clone)]
pub struct DiscordConfig {
    /// Discord bot token (required).
    pub bot_token: String,
    /// Channel IDs to monitor. Empty = all channels.
    pub allowed_channel_ids: Vec<u64>,
    /// Guild IDs to allow. Empty = all guilds.
    pub allowed_guild_ids: Vec<u64>,
    /// Block @everyone and @here in outgoing messages.
    pub block_everyone: bool,
    /// Number of recent messages to include as conversation context (0 = disabled).
    pub context_history_limit: u8,
    /// Reaction emoji for processing start.
    pub reaction_processing: String,
    /// Reaction emoji for successful completion.
    pub reaction_done: String,
    /// Reaction emoji for errors.
    pub reaction_error: String,
    /// Embed color (Discord blurple by default).
    pub embed_color: u32,
    /// Character threshold for switching from plain text to embed.
    pub embed_threshold: usize,
    /// Discord user IDs authorized for backtick direct tool commands. Empty = disabled.
    pub direct_tool_users: Vec<u64>,
    /// Ecosystem tool names available via backtick commands (routed via kernel tool_hint).
    pub direct_tool_ecosystem: Vec<String>,
    /// Maximum number of messages in the queue (default: 5).
    pub queue_max_size: usize,
    /// Timeout in seconds for queued messages (default: 180 = 3 minutes).
    pub queue_timeout_secs: u64,
    /// Reaction emoji for queue waiting state.
    pub reaction_queued: String,
}

impl DiscordConfig {
    pub fn from_env() -> Self {
        let bot_token = env::var("DISCORD_BOT_TOKEN").unwrap_or_else(|_| {
            tracing::error!("DISCORD_BOT_TOKEN is required");
            String::new()
        });

        let allowed_channel_ids = parse_id_list("ALLOWED_CHANNEL_IDS");
        let allowed_guild_ids = parse_id_list("ALLOWED_GUILD_IDS");

        let block_everyone = env::var("BLOCK_EVERYONE")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        let context_history_limit = env::var("DISCORD_CONTEXT_HISTORY_LIMIT")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(15)
            .min(50);

        let reaction_processing =
            env::var("DISCORD_REACTION_PROCESSING").unwrap_or_else(|_| "👀".into());
        let reaction_done = env::var("DISCORD_REACTION_DONE").unwrap_or_else(|_| "✅".into());
        let reaction_error = env::var("DISCORD_REACTION_ERROR").unwrap_or_else(|_| "⚠️".into());
        let embed_color = env::var("DISCORD_EMBED_COLOR")
            .ok()
            .and_then(|v| u32::from_str_radix(v.trim_start_matches('#'), 16).ok())
            .unwrap_or(0x5865F2);
        let embed_threshold = env::var("DISCORD_EMBED_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1500);

        let direct_tool_users = parse_id_list("DISCORD_DIRECT_TOOL_USERS");
        let direct_tool_ecosystem: Vec<String> = env::var("DISCORD_DIRECT_TOOL_ECOSYSTEM")
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let queue_max_size = env::var("DISCORD_QUEUE_MAX")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5)
            .min(10);
        let queue_timeout_secs = env::var("DISCORD_QUEUE_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(180);
        let reaction_queued = env::var("DISCORD_REACTION_QUEUED").unwrap_or_else(|_| "⏳".into());

        Self {
            bot_token,
            allowed_channel_ids,
            allowed_guild_ids,
            block_everyone,
            context_history_limit,
            reaction_processing,
            reaction_done,
            reaction_error,
            embed_color,
            embed_threshold,
            direct_tool_users,
            direct_tool_ecosystem,
            queue_max_size,
            queue_timeout_secs,
            reaction_queued,
        }
    }

    /// Check if a guild is allowed (empty list = all allowed).
    pub fn is_guild_allowed(&self, guild_id: u64) -> bool {
        self.allowed_guild_ids.is_empty() || self.allowed_guild_ids.contains(&guild_id)
    }
}

fn parse_id_list(var_name: &str) -> Vec<u64> {
    env::var(var_name)
        .unwrap_or_default()
        .split(',')
        .filter_map(|s| s.trim().parse::<u64>().ok())
        .collect()
}
