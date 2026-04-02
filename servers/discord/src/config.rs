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

        Self {
            bot_token,
            allowed_channel_ids,
            allowed_guild_ids,
            block_everyone,
            context_history_limit,
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
