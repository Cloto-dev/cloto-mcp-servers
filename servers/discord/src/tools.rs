//! Discord MCP tool definitions and execution.
//!
//! Each tool interacts with Discord via Serenity's `Http` client.
//! Follows avatar server pattern: execute() returns (result, notifications).

use crate::protocol::{JsonRpcNotification, McpTool};
use crate::rate_limiter::{RateLimiter, Route};
use crate::utils;
use serde_json::{json, Value};
use serenity::all as serenity;
use std::sync::Arc;

/// Build tool schemas for `tools/list`.
pub fn tool_list() -> Vec<McpTool> {
    vec![
        send_message_schema(),
        send_buttons_schema(),
        send_file_schema(),
        add_reaction_schema(),
        list_channels_schema(),
        list_threads_schema(),
        create_thread_schema(),
        get_history_schema(),
        search_messages_schema(),
        edit_message_schema(),
        delete_message_schema(),
        set_presence_schema(),
    ]
}

/// All bridge-native tool names (used for direct command routing).
pub const BRIDGE_TOOL_NAMES: &[&str] = &[
    "send_message",
    "send_buttons",
    "send_file",
    "add_reaction",
    "list_channels",
    "list_threads",
    "create_thread",
    "get_history",
    "search_messages",
    "edit_message",
    "delete_message",
    "set_presence",
    "bridge_status",
];

/// Execute a tool call. Returns `(result_value, notifications_to_emit)`.
pub async fn execute(
    tool_name: &str,
    args: &Value,
    http: &Arc<serenity::Http>,
    config: &crate::config::DiscordConfig,
    bot_context: &Arc<std::sync::Mutex<Option<serenity::Context>>>,
    rate_limiter: &Arc<RateLimiter>,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    match tool_name {
        "send_message" => execute_send_message(args, http, config, rate_limiter).await,
        "send_buttons" => execute_send_buttons(args, http, config, rate_limiter).await,
        "send_file" => execute_send_file(args, http, rate_limiter).await,
        "add_reaction" => execute_add_reaction(args, http, rate_limiter).await,
        "list_channels" => execute_list_channels(args, http, config).await,
        "list_threads" => execute_list_threads(args, http, config).await,
        "create_thread" => execute_create_thread(args, http, rate_limiter).await,
        "get_history" => execute_get_history(args, http).await,
        "search_messages" => execute_search_messages(args, http).await,
        "edit_message" => execute_edit_message(args, http, config, rate_limiter).await,
        "delete_message" => execute_delete_message(args, http, rate_limiter).await,
        "set_presence" => execute_set_presence(args, bot_context).await,
        _ => Err(format!("Unknown tool: {tool_name}")),
    }
}

// ── Tool Schemas ──

fn send_message_schema() -> McpTool {
    McpTool {
        name: "send_message".into(),
        description: "Send a message to a Discord channel. Supports reply, rich embeds, auto-splitting. Set embed=true for structured embed formatting with author/footer/fields.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "channel_id": {
                    "type": "string",
                    "description": "Discord channel ID"
                },
                "content": {
                    "type": "string",
                    "description": "Message content to send (used as embed description when embed=true)"
                },
                "reply_to": {
                    "type": "string",
                    "description": "Message ID to reply to (optional, sends as Discord Reply)"
                },
                "embed": {
                    "type": "boolean",
                    "description": "Force rich embed format (default: auto-detect based on length)"
                },
                "embed_title": {
                    "type": "string",
                    "description": "Embed title (optional, only used when embed=true or auto-embed)"
                },
                "embed_author": {
                    "type": "string",
                    "description": "Embed author name (optional)"
                },
                "embed_footer": {
                    "type": "string",
                    "description": "Embed footer text (optional)"
                },
                "embed_color": {
                    "type": "string",
                    "description": "Embed color as hex string e.g. '#FF5733' (optional, uses default)"
                },
                "embed_fields": {
                    "type": "array",
                    "description": "Embed fields array (optional)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "value": { "type": "string" },
                            "inline": { "type": "boolean" }
                        },
                        "required": ["name", "value"]
                    }
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
    rate_limiter: &Arc<RateLimiter>,
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

    let route = Route::ChannelMessage(channel_id);

    // Rich embed options
    let force_embed = args.get("embed").and_then(|v| v.as_bool()).unwrap_or(false);
    let embed_title = args.get("embed_title").and_then(|v| v.as_str());
    let embed_author = args.get("embed_author").and_then(|v| v.as_str());
    let embed_footer = args.get("embed_footer").and_then(|v| v.as_str());
    let embed_color_override = args
        .get("embed_color")
        .and_then(|v| v.as_str())
        .and_then(|s| u32::from_str_radix(s.trim_start_matches('#'), 16).ok());
    let embed_fields = args
        .get("embed_fields")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let use_embed = force_embed || content.len() > config.embed_threshold;
    let color = embed_color_override.unwrap_or(config.embed_color);

    if use_embed {
        // Split into embed-sized chunks (4096 char limit per embed description)
        let chunks = utils::split_message(&content, 4096);
        for (i, chunk) in chunks.iter().enumerate() {
            let mut embed = serenity::CreateEmbed::new()
                .description(chunk)
                .color(color);

            // Apply rich formatting on first chunk only
            if i == 0 {
                if let Some(title) = embed_title {
                    embed = embed.title(title);
                }
                if let Some(author) = embed_author {
                    embed = embed.author(serenity::CreateEmbedAuthor::new(author));
                }
                // Add fields to first chunk
                for field in &embed_fields {
                    let name = field.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let value = field.get("value").and_then(|v| v.as_str()).unwrap_or("");
                    let inline = field.get("inline").and_then(|v| v.as_bool()).unwrap_or(false);
                    if !name.is_empty() && !value.is_empty() {
                        embed = embed.field(name, value, inline);
                    }
                }
            }
            // Footer on last chunk
            if i == chunks.len() - 1 {
                if let Some(footer) = embed_footer {
                    embed = embed.footer(serenity::CreateEmbedFooter::new(footer));
                }
                embed = embed.timestamp(serenity::Timestamp::now());
            }

            let mut msg_builder = serenity::CreateMessage::new().embed(embed);
            if i == 0 {
                if let Some(mid) = reply_to {
                    msg_builder = msg_builder.reference_message(serenity::MessageReference::from(
                        (channel, serenity::MessageId::new(mid)),
                    ));
                }
            }
            rate_limiter.acquire(route.clone()).await;
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
                    msg_builder = msg_builder.reference_message(serenity::MessageReference::from(
                        (channel, serenity::MessageId::new(mid)),
                    ));
                }
            }
            rate_limiter.acquire(route.clone()).await;
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

fn send_buttons_schema() -> McpTool {
    McpTool {
        name: "send_buttons".into(),
        description: "Send a message with interactive buttons or a select menu. Each button has a custom_id that is returned when pressed. Use for confirmations, approvals, or choices.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "channel_id": {
                    "type": "string",
                    "description": "Discord channel ID"
                },
                "content": {
                    "type": "string",
                    "description": "Message text displayed above the components"
                },
                "buttons": {
                    "type": "array",
                    "description": "Array of buttons to display",
                    "items": {
                        "type": "object",
                        "properties": {
                            "custom_id": {
                                "type": "string",
                                "description": "Unique identifier returned on click (e.g. 'approve:req-123')"
                            },
                            "label": {
                                "type": "string",
                                "description": "Button text"
                            },
                            "style": {
                                "type": "string",
                                "enum": ["primary", "secondary", "success", "danger"],
                                "description": "Button style (default: primary)"
                            },
                            "emoji": {
                                "type": "string",
                                "description": "Optional emoji to display on the button"
                            }
                        },
                        "required": ["custom_id", "label"]
                    }
                },
                "select_menu": {
                    "type": "object",
                    "description": "A select menu (alternative to buttons, mutually exclusive)",
                    "properties": {
                        "custom_id": {
                            "type": "string",
                            "description": "Unique identifier for the select menu"
                        },
                        "placeholder": {
                            "type": "string",
                            "description": "Placeholder text when nothing is selected"
                        },
                        "options": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": { "type": "string" },
                                    "value": { "type": "string" },
                                    "description": { "type": "string" }
                                },
                                "required": ["label", "value"]
                            }
                        }
                    },
                    "required": ["custom_id", "options"]
                }
            },
            "required": ["channel_id", "content"]
        }),
    }
}

async fn execute_send_buttons(
    args: &Value,
    http: &Arc<serenity::Http>,
    config: &crate::config::DiscordConfig,
    rate_limiter: &Arc<RateLimiter>,
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
    let mut msg_builder = serenity::CreateMessage::new().content(&content);

    // Build components
    let mut action_row_components: Vec<serenity::CreateActionRow> = Vec::new();

    // Buttons
    if let Some(buttons) = args.get("buttons").and_then(|v| v.as_array()) {
        let mut row_buttons: Vec<serenity::CreateButton> = Vec::new();
        for btn in buttons.iter().take(5) {
            // Max 5 buttons per row
            let custom_id = btn.get("custom_id").and_then(|v| v.as_str()).unwrap_or("");
            let label = btn.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let style = match btn.get("style").and_then(|v| v.as_str()).unwrap_or("primary") {
                "secondary" => serenity::ButtonStyle::Secondary,
                "success" => serenity::ButtonStyle::Success,
                "danger" => serenity::ButtonStyle::Danger,
                _ => serenity::ButtonStyle::Primary,
            };

            let mut button = serenity::CreateButton::new(custom_id)
                .label(label)
                .style(style);

            if let Some(emoji_str) = btn.get("emoji").and_then(|v| v.as_str()) {
                button = button.emoji(serenity::ReactionType::Unicode(emoji_str.to_string()));
            }
            row_buttons.push(button);
        }
        if !row_buttons.is_empty() {
            action_row_components.push(serenity::CreateActionRow::Buttons(row_buttons));
        }
    }

    // Select menu
    if let Some(menu) = args.get("select_menu") {
        let custom_id = menu
            .get("custom_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let placeholder = menu.get("placeholder").and_then(|v| v.as_str());
        let options = menu
            .get("options")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let menu_options: Vec<serenity::CreateSelectMenuOption> = options
            .iter()
            .take(25) // Discord max
            .filter_map(|opt| {
                let label = opt.get("label").and_then(|v| v.as_str())?;
                let value = opt.get("value").and_then(|v| v.as_str())?;
                let mut option = serenity::CreateSelectMenuOption::new(label, value);
                if let Some(desc) = opt.get("description").and_then(|v| v.as_str()) {
                    option = option.description(desc);
                }
                Some(option)
            })
            .collect();

        if !menu_options.is_empty() {
            let mut select = serenity::CreateSelectMenu::new(
                custom_id,
                serenity::CreateSelectMenuKind::String {
                    options: menu_options,
                },
            );
            if let Some(ph) = placeholder {
                select = select.placeholder(ph);
            }
            action_row_components.push(serenity::CreateActionRow::SelectMenu(select));
        }
    }

    for row in &action_row_components {
        msg_builder = msg_builder.components(vec![row.clone()]);
    }

    rate_limiter
        .acquire(Route::ChannelMessage(channel_id))
        .await;
    let msg = channel
        .send_message(http, msg_builder)
        .await
        .map_err(|e| format!("Failed to send buttons: {e}"))?;

    Ok((
        text_result(format!("Buttons sent: {}", msg.id)),
        vec![],
    ))
}

async fn execute_send_file(
    args: &Value,
    http: &Arc<serenity::Http>,
    rate_limiter: &Arc<RateLimiter>,
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

    rate_limiter.acquire(Route::ChannelMessage(channel_id)).await;
    let msg = channel
        .send_message(http, msg_builder)
        .await
        .map_err(|e| format!("Failed to send file: {e}"))?;

    Ok((text_result(format!("File sent: {}", msg.id)), vec![]))
}

async fn execute_add_reaction(
    args: &Value,
    http: &Arc<serenity::Http>,
    rate_limiter: &Arc<RateLimiter>,
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

    rate_limiter.acquire(Route::ChannelReaction(channel_id)).await;
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
        .filter(|c| {
            matches!(
                c.kind,
                serenity::ChannelType::Text
                    | serenity::ChannelType::News
                    | serenity::ChannelType::Forum
            )
        })
        .map(|c| {
            let type_str = match c.kind {
                serenity::ChannelType::News => "news",
                serenity::ChannelType::Forum => "forum",
                _ => "text",
            };
            let mut entry = json!({
                "id": c.id.to_string(),
                "name": c.name,
                "topic": c.topic,
                "type": type_str,
            });
            if let Some(parent) = c.parent_id {
                entry
                    .as_object_mut()
                    .unwrap()
                    .insert("parent_id".into(), json!(parent.to_string()));
            }
            entry
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
    let sort_asc = args.get("sort").and_then(|v| v.as_str()) == Some("asc");

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
                    .messages(
                        http,
                        serenity::GetMessages::new()
                            .before(before_cursor)
                            .limit(batch_size),
                    )
                    .await
                    .unwrap_or_default();
                if !older.is_empty() {
                    before_cursor = older.iter().map(|m| m.id).min().unwrap_or(before_cursor);
                    collected.extend(older);
                }

                if collected.len() < scan_limit {
                    let newer = channel
                        .messages(
                            http,
                            serenity::GetMessages::new()
                                .after(after_cursor)
                                .limit(batch_size),
                        )
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
                serenity::GetMessages::new()
                    .before(before)
                    .limit(batch_size)
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
        .filter(|m| {
            m.content.to_lowercase().contains(&query)
                || m.author.name.to_lowercase().contains(&query)
        })
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
        format!(
            "No messages matching '{}' found (scanned {} messages)",
            query,
            collected.len()
        )
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
    rate_limiter: &Arc<RateLimiter>,
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

    rate_limiter.acquire(Route::ChannelMessage(channel_id)).await;
    channel
        .edit_message(http, message, edit)
        .await
        .map_err(|e| format!("Failed to edit message: {e}"))?;

    Ok((text_result("Message edited"), vec![]))
}

async fn execute_delete_message(
    args: &Value,
    http: &Arc<serenity::Http>,
    rate_limiter: &Arc<RateLimiter>,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    let channel_id = parse_id(args, "channel_id")?;
    let message_id = parse_id(args, "message_id")?;

    let channel = serenity::ChannelId::new(channel_id);
    let message = serenity::MessageId::new(message_id);

    rate_limiter.acquire(Route::ChannelMessage(channel_id)).await;
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

// ── Thread Tools ──

fn list_threads_schema() -> McpTool {
    McpTool {
        name: "list_threads".into(),
        description: "List active threads in a Discord guild.".into(),
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

async fn execute_list_threads(
    args: &Value,
    http: &Arc<serenity::Http>,
    config: &crate::config::DiscordConfig,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    let guild_id = parse_id(args, "guild_id")?;

    if !config.is_guild_allowed(guild_id) {
        return Err(format!("Guild {guild_id} is not in allowed list"));
    }

    let guild = serenity::GuildId::new(guild_id);
    let threads_data = guild
        .get_active_threads(http)
        .await
        .map_err(|e| format!("Failed to fetch threads: {e}"))?;

    let threads: Vec<Value> = threads_data
        .threads
        .iter()
        .map(|t| {
            json!({
                "id": t.id.to_string(),
                "name": t.name,
                "parent_id": t.parent_id.map(|p| p.to_string()),
                "type": match t.kind {
                    serenity::ChannelType::PublicThread => "public_thread",
                    serenity::ChannelType::PrivateThread => "private_thread",
                    serenity::ChannelType::NewsThread => "news_thread",
                    _ => "thread",
                },
                "archived": t.thread_metadata.as_ref().is_some_and(|tm| tm.archived),
                "message_count": t.message_count,
            })
        })
        .collect();

    let result = json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(&threads).unwrap_or_default()
        }]
    });
    Ok((result, vec![]))
}

fn create_thread_schema() -> McpTool {
    McpTool {
        name: "create_thread".into(),
        description:
            "Create a new thread in a Discord channel, optionally from an existing message.".into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "channel_id": {
                    "type": "string",
                    "description": "Channel ID to create thread in"
                },
                "name": {
                    "type": "string",
                    "description": "Thread name (2-100 characters)"
                },
                "message_id": {
                    "type": "string",
                    "description": "Message ID to create thread from (optional)"
                },
                "auto_archive_minutes": {
                    "type": "integer",
                    "enum": [60, 1440, 4320, 10080],
                    "description": "Minutes until auto-archive (default: 1440 = 24h)"
                }
            },
            "required": ["channel_id", "name"]
        }),
    }
}

async fn execute_create_thread(
    args: &Value,
    http: &Arc<serenity::Http>,
    rate_limiter: &Arc<RateLimiter>,
) -> Result<(Value, Vec<JsonRpcNotification>), String> {
    let channel_id = parse_id(args, "channel_id")?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("name is required")?;
    let message_id = args
        .get("message_id")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<u64>().ok());
    let archive_minutes = args
        .get("auto_archive_minutes")
        .and_then(|v| v.as_u64())
        .unwrap_or(1440) as u16;

    let archive_duration = match archive_minutes {
        60 => serenity::AutoArchiveDuration::OneHour,
        4320 => serenity::AutoArchiveDuration::ThreeDays,
        10080 => serenity::AutoArchiveDuration::OneWeek,
        _ => serenity::AutoArchiveDuration::OneDay,
    };

    let cid = serenity::ChannelId::new(channel_id);

    rate_limiter.acquire(Route::ChannelMessage(channel_id)).await;
    let thread = if let Some(mid) = message_id {
        let builder = serenity::CreateThread::new(name).auto_archive_duration(archive_duration);
        cid.create_thread_from_message(http, serenity::MessageId::new(mid), builder)
            .await
            .map_err(|e| format!("Failed to create thread from message: {e}"))?
    } else {
        let builder = serenity::CreateThread::new(name)
            .kind(serenity::ChannelType::PublicThread)
            .auto_archive_duration(archive_duration);
        cid.create_thread(http, builder)
            .await
            .map_err(|e| format!("Failed to create thread: {e}"))?
    };

    let result = json!({
        "id": thread.id.to_string(),
        "name": thread.name,
        "parent_id": thread.parent_id.map(|p| p.to_string()),
    });
    Ok((
        text_result(serde_json::to_string_pretty(&result).unwrap_or_default()),
        vec![],
    ))
}
