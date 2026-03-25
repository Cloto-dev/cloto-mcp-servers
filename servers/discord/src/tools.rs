//! Discord MCP tool definitions and execution.
//!
//! Each tool interacts with Discord via Serenity's `Http` client.
//! Follows avatar server pattern: execute() returns (result, notifications).

use crate::protocol::{JsonRpcNotification, McpTool};
use crate::utils;
use serde_json::{json, Value};
use serenity::all as serenity;
use std::sync::Arc;

/// Build tool schemas for `tools/list`.
pub fn tool_list() -> Vec<McpTool> {
    vec![
        send_message_schema(),
        add_reaction_schema(),
        list_channels_schema(),
        get_history_schema(),
        edit_message_schema(),
        delete_message_schema(),
    ]
}

/// Execute a tool call. Returns `(result_value, notifications_to_emit)`.
pub async fn execute(
    tool_name: &str,
    args: &Value,
    http: &Arc<serenity::Http>,
    config: &crate::config::DiscordConfig,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    match tool_name {
        "send_message" => execute_send_message(args, http, config).await,
        "add_reaction" => execute_add_reaction(args, http).await,
        "list_channels" => execute_list_channels(args, http, config).await,
        "get_history" => execute_get_history(args, http).await,
        "edit_message" => execute_edit_message(args, http, config).await,
        "delete_message" => execute_delete_message(args, http).await,
        _ => Err(format!("Unknown tool: {tool_name}")),
    }
}

// ── Tool Schemas ──

fn send_message_schema() -> McpTool {
    McpTool {
        name: "send_message".into(),
        description: "Send a message to a Discord channel. Long messages are automatically split at 2000 characters.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "channel_id": {
                    "type": "string",
                    "description": "Discord channel ID"
                },
                "content": {
                    "type": "string",
                    "description": "Message content to send"
                }
            },
            "required": ["channel_id", "content"]
        }),
    }
}

fn add_reaction_schema() -> McpTool {
    McpTool {
        name: "add_reaction".into(),
        description: "Add a reaction emoji to a message.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "channel_id": {
                    "type": "string",
                    "description": "Discord channel ID"
                },
                "message_id": {
                    "type": "string",
                    "description": "Discord message ID"
                },
                "emoji": {
                    "type": "string",
                    "description": "Emoji to react with (Unicode emoji or custom format <:name:id>)"
                }
            },
            "required": ["channel_id", "message_id", "emoji"]
        }),
    }
}

fn list_channels_schema() -> McpTool {
    McpTool {
        name: "list_channels".into(),
        description: "List text channels in a Discord guild.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "guild_id": {
                    "type": "string",
                    "description": "Discord guild (server) ID"
                }
            },
            "required": ["guild_id"]
        }),
    }
}

fn get_history_schema() -> McpTool {
    McpTool {
        name: "get_history".into(),
        description: "Get recent message history from a Discord channel.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "channel_id": {
                    "type": "string",
                    "description": "Discord channel ID"
                },
                "limit": {
                    "type": "integer",
                    "description": "Number of messages to fetch (default: 25, max: 100)",
                    "minimum": 1,
                    "maximum": 100
                },
                "around_time": {
                    "type": "string",
                    "description": "ISO 8601 timestamp to search around (optional)"
                }
            },
            "required": ["channel_id"]
        }),
    }
}

fn edit_message_schema() -> McpTool {
    McpTool {
        name: "edit_message".into(),
        description: "Edit a message previously sent by the bot.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "channel_id": {
                    "type": "string",
                    "description": "Discord channel ID"
                },
                "message_id": {
                    "type": "string",
                    "description": "Discord message ID to edit"
                },
                "content": {
                    "type": "string",
                    "description": "New message content"
                }
            },
            "required": ["channel_id", "message_id", "content"]
        }),
    }
}

fn delete_message_schema() -> McpTool {
    McpTool {
        name: "delete_message".into(),
        description: "Delete a message from a Discord channel.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "channel_id": {
                    "type": "string",
                    "description": "Discord channel ID"
                },
                "message_id": {
                    "type": "string",
                    "description": "Discord message ID to delete"
                }
            },
            "required": ["channel_id", "message_id"]
        }),
    }
}

// ── Tool Implementations ──

fn parse_id(val: &Value, field: &str) -> Result<u64, String> {
    val.get(field)
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("{field} is required"))?
        .parse::<u64>()
        .map_err(|_| format!("Invalid {field}"))
}

fn text_result(text: impl Into<String>) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": text.into()
        }]
    })
}

async fn execute_send_message(
    args: &Value,
    http: &Arc<serenity::Http>,
    config: &crate::config::DiscordConfig,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    let channel_id = parse_id(args, "channel_id")?;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("content is required")?;

    let content = if config.block_everyone {
        utils::sanitize_mentions(content)
    } else {
        content.to_string()
    };

    let channel = serenity::ChannelId::new(channel_id);
    let chunks = utils::split_message(&content, 2000);
    let mut sent_ids = Vec::new();

    for chunk in &chunks {
        let msg = channel
            .say(http, chunk)
            .await
            .map_err(|e| format!("Failed to send message: {e}"))?;
        sent_ids.push(msg.id.to_string());
    }

    let result = text_result(format!(
        "Sent {} message(s) to channel {channel_id}: [{}]",
        sent_ids.len(),
        sent_ids.join(", ")
    ));
    Ok((result, vec![]))
}

async fn execute_add_reaction(
    args: &Value,
    http: &Arc<serenity::Http>,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    let channel_id = parse_id(args, "channel_id")?;
    let message_id = parse_id(args, "message_id")?;
    let emoji_str = args
        .get("emoji")
        .and_then(|v| v.as_str())
        .ok_or("emoji is required")?;

    let channel = serenity::ChannelId::new(channel_id);
    let message = serenity::MessageId::new(message_id);
    let emoji = serenity::ReactionType::Unicode(emoji_str.to_string());

    http.create_reaction(channel, message, &emoji)
        .await
        .map_err(|e| format!("Failed to add reaction: {e}"))?;

    Ok((text_result("Reaction added"), vec![]))
}

async fn execute_list_channels(
    args: &Value,
    http: &Arc<serenity::Http>,
    config: &crate::config::DiscordConfig,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    let guild_id = parse_id(args, "guild_id")?;

    if !config.is_guild_allowed(guild_id) {
        return Err(format!("Guild {guild_id} is not in allowed list"));
    }

    let guild = serenity::GuildId::new(guild_id);
    let channels = guild
        .channels(http)
        .await
        .map_err(|e| format!("Failed to fetch channels: {e}"))?;

    let text_channels: Vec<Value> = channels
        .values()
        .filter(|c| c.kind == serenity::ChannelType::Text)
        .map(|c| {
            json!({
                "id": c.id.to_string(),
                "name": c.name,
                "topic": c.topic,
            })
        })
        .collect();

    let result = json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&text_channels).unwrap_or_default()
        }]
    });
    Ok((result, vec![]))
}

async fn execute_get_history(
    args: &Value,
    http: &Arc<serenity::Http>,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    let channel_id = parse_id(args, "channel_id")?;
    let limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(25)
        .min(100) as u8;

    let channel = serenity::ChannelId::new(channel_id);

    let builder = if let Some(around_time_str) = args.get("around_time").and_then(|v| v.as_str()) {
        let time = around_time_str
            .parse::<chrono::DateTime<chrono::Utc>>()
            .map_err(|e| format!("Invalid around_time: {e}"))?;
        let snowflake = utils::timestamp_to_snowflake(time);
        serenity::GetMessages::new()
            .around(serenity::MessageId::new(snowflake))
            .limit(limit)
    } else {
        serenity::GetMessages::new().limit(limit)
    };

    let messages = channel
        .messages(http, builder)
        .await
        .map_err(|e| format!("Failed to fetch history: {e}"))?;

    let jst_offset = chrono::FixedOffset::east_opt(9 * 3600).unwrap();
    let formatted: Vec<String> = messages
        .iter()
        .rev() // oldest first
        .filter(|m| !m.content.is_empty() || !m.attachments.is_empty())
        .map(|m| {
            let ts = chrono::DateTime::from_timestamp(m.timestamp.unix_timestamp(), 0)
                .map(|dt| {
                    dt.with_timezone(&jst_offset)
                        .format("%Y-%m-%d %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|| m.timestamp.to_string());
            let content = if m.content.is_empty() {
                "[Attachment/Embed]"
            } else {
                &m.content
            };
            format!("[{}] {}: {}", ts, m.author.name, content)
        })
        .collect();

    let result = text_result(if formatted.is_empty() {
        "No messages found".to_string()
    } else {
        formatted.join("\n")
    });
    Ok((result, vec![]))
}

async fn execute_edit_message(
    args: &Value,
    http: &Arc<serenity::Http>,
    config: &crate::config::DiscordConfig,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    let channel_id = parse_id(args, "channel_id")?;
    let message_id = parse_id(args, "message_id")?;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("content is required")?;

    let content = if config.block_everyone {
        utils::sanitize_mentions(content)
    } else {
        content.to_string()
    };

    let channel = serenity::ChannelId::new(channel_id);
    let message = serenity::MessageId::new(message_id);
    let edit = serenity::EditMessage::new().content(&content);

    channel
        .edit_message(http, message, edit)
        .await
        .map_err(|e| format!("Failed to edit message: {e}"))?;

    Ok((text_result("Message edited"), vec![]))
}

async fn execute_delete_message(
    args: &Value,
    http: &Arc<serenity::Http>,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    let channel_id = parse_id(args, "channel_id")?;
    let message_id = parse_id(args, "message_id")?;

    let channel = serenity::ChannelId::new(channel_id);
    let message = serenity::MessageId::new(message_id);

    channel
        .delete_message(http, message)
        .await
        .map_err(|e| format!("Failed to delete message: {e}"))?;

    Ok((text_result("Message deleted"), vec![]))
}
