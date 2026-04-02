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
        send_file_schema(),
        add_reaction_schema(),
        list_channels_schema(),
        get_history_schema(),
        search_messages_schema(),
        edit_message_schema(),
        delete_message_schema(),
        set_presence_schema(),
    ]
}

/// Execute a tool call. Returns `(result_value, notifications_to_emit)`.
pub async fn execute(
    tool_name: &str,
    args: &Value,
    http: &Arc<serenity::Http>,
    config: &crate::config::DiscordConfig,
    bot_context: &Arc<std::sync::Mutex<Option<serenity::Context>>>,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    match tool_name {
        "send_message" => execute_send_message(args, http, config).await,
        "send_file" => execute_send_file(args, http).await,
        "add_reaction" => execute_add_reaction(args, http).await,
        "list_channels" => execute_list_channels(args, http, config).await,
        "get_history" => execute_get_history(args, http).await,
        "search_messages" => execute_search_messages(args, http).await,
        "edit_message" => execute_edit_message(args, http, config).await,
        "delete_message" => execute_delete_message(args, http).await,
        "set_presence" => execute_set_presence(args, bot_context).await,
        _ => Err(format!("Unknown tool: {tool_name}")),
    }
}

// ── Tool Schemas ──

fn send_message_schema() -> McpTool {
    McpTool {
        name: "send_message".into(),
        description: "Send a message to a Discord channel. Supports reply, embed for long messages, and auto-splitting.".into(),
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
                },
                "reply_to": {
                    "type": "string",
                    "description": "Message ID to reply to (optional, sends as Discord Reply)"
                }
            },
            "required": ["channel_id", "content"]
        }),
    }
}

fn send_file_schema() -> McpTool {
    McpTool {
        name: "send_file".into(),
        description: "Send a file to a Discord channel as an attachment.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "channel_id": {
                    "type": "string",
                    "description": "Discord channel ID"
                },
                "file_path": {
                    "type": "string",
                    "description": "Local file path to upload"
                },
                "filename": {
                    "type": "string",
                    "description": "Override filename for the attachment (optional)"
                },
                "content": {
                    "type": "string",
                    "description": "Optional message text to accompany the file"
                }
            },
            "required": ["channel_id", "file_path"]
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

fn search_messages_schema() -> McpTool {
    McpTool {
        name: "search_messages".into(),
        description: "Search Discord channel message history by keyword. Fetches messages and filters locally. Use for past conversation research.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "channel_id": {
                    "type": "string",
                    "description": "Discord channel ID to search"
                },
                "query": {
                    "type": "string",
                    "description": "Search keyword (case-insensitive substring match)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of messages to scan (default: 200, max: 500)",
                    "minimum": 1,
                    "maximum": 500
                },
                "target_time": {
                    "type": "string",
                    "description": "ISO 8601 timestamp to search around (optional, searches recent messages by default)"
                },
                "sort": {
                    "type": "string",
                    "enum": ["desc", "asc"],
                    "description": "Sort order: 'desc' (newest first, default) or 'asc' (oldest first)"
                }
            },
            "required": ["channel_id", "query"]
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

fn set_presence_schema() -> McpTool {
    McpTool {
        name: "set_presence".into(),
        description: "Set the bot's online status and activity.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["online", "idle", "dnd", "invisible"],
                    "description": "Online status"
                },
                "activity": {
                    "type": "string",
                    "description": "Activity text (optional)"
                },
                "activity_type": {
                    "type": "string",
                    "enum": ["playing", "watching", "listening", "competing"],
                    "description": "Activity type (default: playing)"
                }
            },
            "required": ["status"]
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
    let reply_to = args
        .get("reply_to")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok());

    let content = if config.block_everyone {
        utils::sanitize_mentions(content)
    } else {
        content.to_string()
    };

    let channel = serenity::ChannelId::new(channel_id);
    let mut sent_ids = Vec::new();

    // Use embed for long messages
    if content.len() > config.embed_threshold {
        // Split into embed-sized chunks (4096 char limit per embed description)
        let chunks = utils::split_message(&content, 4096);
        for (i, chunk) in chunks.iter().enumerate() {
            let embed = serenity::CreateEmbed::new()
                .description(chunk)
                .color(config.embed_color);
            let mut msg_builder = serenity::CreateMessage::new().embed(embed);
            // Reply on first chunk only
            if i == 0 {
                if let Some(mid) = reply_to {
                    msg_builder = msg_builder.reference_message(serenity::MessageReference::from((
                        channel,
                        serenity::MessageId::new(mid),
                    )));
                }
            }
            let msg = channel
                .send_message(http, msg_builder)
                .await
                .map_err(|e| format!("Failed to send embed: {e}"))?;
            sent_ids.push(msg.id.to_string());
        }
    } else {
        // Plain text with splitting at 2000 chars
        let chunks = utils::split_message(&content, 2000);
        for (i, chunk) in chunks.iter().enumerate() {
            let mut msg_builder = serenity::CreateMessage::new().content(chunk);
            if i == 0 {
                if let Some(mid) = reply_to {
                    msg_builder = msg_builder.reference_message(serenity::MessageReference::from((
                        channel,
                        serenity::MessageId::new(mid),
                    )));
                }
            }
            let msg = channel
                .send_message(http, msg_builder)
                .await
                .map_err(|e| format!("Failed to send message: {e}"))?;
            sent_ids.push(msg.id.to_string());
        }
    }

    let result = text_result(format!(
        "Sent {} message(s) to channel {channel_id}: [{}]",
        sent_ids.len(),
        sent_ids.join(", ")
    ));
    Ok((result, vec![]))
}

async fn execute_send_file(
    args: &Value,
    http: &Arc<serenity::Http>,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    let channel_id = parse_id(args, "channel_id")?;
    let file_path = args
        .get("file_path")
        .and_then(|v| v.as_str())
        .ok_or("file_path is required")?;
    let filename_override = args.get("filename").and_then(|v| v.as_str());
    let content = args.get("content").and_then(|v| v.as_str());

    let channel = serenity::ChannelId::new(channel_id);

    let mut attachment = serenity::CreateAttachment::path(file_path)
        .await
        .map_err(|e| format!("Failed to read file: {e}"))?;
    if let Some(name) = filename_override {
        attachment.filename = name.to_string();
    }

    let mut msg_builder = serenity::CreateMessage::new().add_file(attachment);
    if let Some(text) = content {
        msg_builder = msg_builder.content(text);
    }

    let msg = channel
        .send_message(http, msg_builder)
        .await
        .map_err(|e| format!("Failed to send file: {e}"))?;

    Ok((text_result(format!("File sent: {}", msg.id)), vec![]))
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

async fn execute_search_messages(
    args: &Value,
    http: &Arc<serenity::Http>,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    let channel_id = parse_id(args, "channel_id")?;
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("query is required")?
        .to_lowercase();
    let scan_limit = args
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(200)
        .min(500) as usize;
    let sort_asc = args
        .get("sort")
        .and_then(|v| v.as_str())
        == Some("asc");

    let channel = serenity::ChannelId::new(channel_id);
    let jst_offset = chrono::FixedOffset::east_opt(9 * 3600).unwrap();

    // Determine starting point
    let target_time = args
        .get("target_time")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok());

    let mut collected: Vec<serenity::Message> = Vec::new();

    if let Some(time) = target_time {
        let snowflake = utils::timestamp_to_snowflake(time);
        let anchor = serenity::MessageId::new(snowflake);
        let initial = channel
            .messages(http, serenity::GetMessages::new().around(anchor).limit(100))
            .await
            .map_err(|e| format!("Failed to fetch messages: {e}"))?;
        collected.extend(initial);

        if !collected.is_empty() && collected.len() < scan_limit {
            collected.sort_by_key(|m| m.id);
            let mut before_cursor = collected.first().unwrap().id;
            let mut after_cursor = collected.last().unwrap().id;

            while collected.len() < scan_limit {
                let pre_len = collected.len();
                let batch_size = (scan_limit - collected.len()).min(100) as u8;

                let older = channel
                    .messages(http, serenity::GetMessages::new().before(before_cursor).limit(batch_size))
                    .await
                    .unwrap_or_default();
                if !older.is_empty() {
                    before_cursor = older.iter().map(|m| m.id).min().unwrap_or(before_cursor);
                    collected.extend(older);
                }

                if collected.len() < scan_limit {
                    let newer = channel
                        .messages(http, serenity::GetMessages::new().after(after_cursor).limit(batch_size))
                        .await
                        .unwrap_or_default();
                    if !newer.is_empty() {
                        after_cursor = newer.iter().map(|m| m.id).max().unwrap_or(after_cursor);
                        collected.extend(newer);
                    }
                }

                if collected.len() == pre_len {
                    break;
                }
            }
        }
    } else {
        let mut cursor: Option<serenity::MessageId> = None;
        while collected.len() < scan_limit {
            let batch_size = (scan_limit - collected.len()).min(100) as u8;
            let builder = if let Some(before) = cursor {
                serenity::GetMessages::new().before(before).limit(batch_size)
            } else {
                serenity::GetMessages::new().limit(batch_size)
            };

            let batch = channel
                .messages(http, builder)
                .await
                .map_err(|e| format!("Failed to fetch messages: {e}"))?;

            if batch.is_empty() {
                break;
            }
            cursor = batch.iter().map(|m| m.id).min();
            collected.extend(batch);
        }
    }

    collected.sort_by_key(|m| m.id);
    collected.dedup_by_key(|m| m.id);
    if !sort_asc {
        collected.reverse();
    }

    let matched: Vec<&serenity::Message> = collected
        .iter()
        .filter(|m| m.content.to_lowercase().contains(&query) || m.author.name.to_lowercase().contains(&query))
        .collect();

    let formatted: Vec<String> = matched
        .iter()
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
        format!("No messages matching '{}' found (scanned {} messages)", query, collected.len())
    } else {
        format!(
            "Found {} matching messages (scanned {}):\n{}",
            formatted.len(),
            collected.len(),
            formatted.join("\n")
        )
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

async fn execute_set_presence(
    args: &Value,
    bot_context: &Arc<std::sync::Mutex<Option<serenity::Context>>>,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    let status_str = args
        .get("status")
        .and_then(|v| v.as_str())
        .ok_or("status is required")?;
    let activity_text = args.get("activity").and_then(|v| v.as_str());
    let activity_type = args
        .get("activity_type")
        .and_then(|v| v.as_str())
        .unwrap_or("playing");

    let status = match status_str {
        "online" => serenity::OnlineStatus::Online,
        "idle" => serenity::OnlineStatus::Idle,
        "dnd" => serenity::OnlineStatus::DoNotDisturb,
        "invisible" => serenity::OnlineStatus::Invisible,
        _ => return Err(format!("Invalid status: {status_str}")),
    };

    let activity = activity_text.map(|text| {
        let kind = match activity_type {
            "watching" => serenity::ActivityType::Watching,
            "listening" => serenity::ActivityType::Listening,
            "competing" => serenity::ActivityType::Competing,
            _ => serenity::ActivityType::Playing,
        };
        serenity::ActivityData {
            name: text.to_string(),
            kind,
            state: None,
            url: None,
        }
    });

    let ctx = bot_context
        .lock()
        .map_err(|_| "Failed to lock context".to_string())?
        .clone()
        .ok_or("Discord context not available (not connected yet)")?;

    ctx.set_presence(activity, status);

    Ok((text_result(format!("Presence set to {status_str}")), vec![]))
}
