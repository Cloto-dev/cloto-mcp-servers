//! MGP Discord Server v0.2.0 — Bidirectional Discord communication.
//!
//! Runs as a single process using stdio JSON-RPC transport + Discord Gateway.
//! Server ID: `io.discord`
//!
//! Communication flow:
//!   Agent → tools/call → Kernel → stdio → this server → Discord API
//!   Discord Gateway → this server → notifications/mgp.callback.request → Kernel → agentic loop
//!   Kernel → mgp/callback/respond → this server → Discord API (auto-reply)
//!
//! Architecture:
//!   - std::thread reads stdin lines → mpsc → main loop
//!   - Serenity Gateway runs in tokio task → mpsc → main loop
//!   - Main loop: tokio::select! dispatches both, writes to stdout via Mutex

mod config;
mod handler;
mod protocol;
mod tools;
mod utils;

use config::DiscordConfig;
use handler::{DiscordEvent, DiscordHandler};
use protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use serde_json::{json, Value};
use serenity::all as serenity;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// Context stored for pending callbacks awaiting kernel response.
struct CallbackContext {
    channel_id: String,
    #[allow(dead_code)]
    guild_id: Option<String>,
    message_id: String,
    #[allow(dead_code)]
    author_name: String,
    /// Typing indicator guard — dropping this stops the typing indicator.
    _typing: Option<serenity::http::Typing>,
}

type PendingCallbacks = Arc<Mutex<HashMap<String, CallbackContext>>>;
type BotContext = Arc<std::sync::Mutex<Option<serenity::Context>>>;

#[tokio::main]
async fn main() {
    // Tracing to stderr (stdout is reserved for JSON-RPC)
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("mgp_discord=info".parse().unwrap()),
        )
        .init();

    tracing::info!("MGP Discord Server v0.2.0 starting");

    let config = DiscordConfig::from_env();

    if config.bot_token.is_empty() {
        tracing::error!("DISCORD_BOT_TOKEN not set — Discord Gateway will not connect");
    }

    // Shared stdout writer
    let stdout = Arc::new(Mutex::new(io::stdout()));

    // Pending callbacks: maps callback_id → CallbackContext for response routing
    let pending_callbacks: PendingCallbacks = Arc::new(Mutex::new(HashMap::new()));

    // Bot user ID (set eagerly via HTTP, updated on Ready)
    let bot_user_id = Arc::new(std::sync::atomic::AtomicU64::new(0));

    // Bot context for presence management (set on Ready)
    let bot_context: BotContext = Arc::new(std::sync::Mutex::new(None));

    // Discord event channel
    let (discord_tx, mut discord_rx) = mpsc::unbounded_channel::<DiscordEvent>();

    // Stdin reader channel
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(100);

    // Start stdin reader thread (blocking I/O, Windows-compatible)
    std::thread::spawn(move || {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) if !l.trim().is_empty() => {
                    if stdin_tx.blocking_send(l).is_err() {
                        break;
                    }
                }
                Err(_) => break,
                _ => continue,
            }
        }
        tracing::info!("stdin closed");
    });

    // Start Serenity Gateway (if token is available)
    let http: Option<Arc<serenity::Http>> = if !config.bot_token.is_empty() {
        let intents = serenity::GatewayIntents::GUILD_MESSAGES
            | serenity::GatewayIntents::MESSAGE_CONTENT
            | serenity::GatewayIntents::GUILDS;

        let handler = DiscordHandler {
            event_tx: discord_tx,
            allowed_channel_ids: config.allowed_channel_ids.clone(),
            bot_context: bot_context.clone(),
        };

        match serenity::Client::builder(&config.bot_token, intents)
            .event_handler(handler)
            .await
        {
            Ok(mut client) => {
                let http_arc = client.http.clone();

                // Eagerly fetch bot user ID before gateway connects
                match http_arc.get_current_user().await {
                    Ok(user) => {
                        bot_user_id.store(user.id.get(), std::sync::atomic::Ordering::Relaxed);
                        tracing::info!("Bot user ID resolved: {} ({})", user.name, user.id);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to fetch bot user eagerly: {e}");
                    }
                }

                tokio::spawn(async move {
                    if let Err(e) = client.start().await {
                        tracing::error!("Serenity client error: {e}");
                    }
                });
                Some(http_arc)
            }
            Err(e) => {
                tracing::error!("Failed to create Serenity client: {e}");
                None
            }
        }
    } else {
        None
    };

    let config = Arc::new(config);

    // Main loop: multiplex stdin JSON-RPC and Discord Gateway events
    loop {
        tokio::select! {
            Some(line) = stdin_rx.recv() => {
                handle_stdin_line(&line, &stdout, &http, &config, &pending_callbacks, &bot_context).await;
            }
            Some(event) = discord_rx.recv() => {
                handle_discord_event(event, &stdout, &pending_callbacks, &http, &config, &bot_user_id).await;
            }
            else => break,
        }
    }

    tracing::info!("MGP Discord Server shutting down");
}

async fn handle_stdin_line(
    line: &str,
    stdout: &Arc<Mutex<io::Stdout>>,
    http: &Option<Arc<serenity::Http>>,
    config: &Arc<DiscordConfig>,
    pending_callbacks: &PendingCallbacks,
    bot_context: &BotContext,
) {
    let request: JsonRpcRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Invalid JSON-RPC: {e}");
            write_message(stdout, &JsonRpcResponse::err(None, -32700, "Parse error"));
            return;
        }
    };

    // Notifications (no id) — acknowledge silently
    if request.id.is_none() {
        tracing::debug!("Received notification: {}", request.method);
        return;
    }

    let (response, notifications) =
        dispatch(&request, http, config, pending_callbacks, bot_context).await;

    // Emit notifications BEFORE the response (avatar pattern)
    for notif in &notifications {
        write_message(stdout, notif);
    }
    write_message(stdout, &response);
}

async fn dispatch(
    request: &JsonRpcRequest,
    http: &Option<Arc<serenity::Http>>,
    config: &Arc<DiscordConfig>,
    pending_callbacks: &PendingCallbacks,
    bot_context: &BotContext,
) -> (JsonRpcResponse, Vec<JsonRpcNotification>) {
    match request.method.as_str() {
        "initialize" => (handle_initialize(request), vec![]),
        "tools/list" => (handle_tools_list(request), vec![]),
        "tools/call" => handle_tools_call(request, http, config, bot_context).await,
        "mgp/callback/respond" => (
            handle_callback_respond(request, http, config, pending_callbacks).await,
            vec![],
        ),
        "notifications/initialized" => {
            (JsonRpcResponse::ok(request.id.clone(), json!({})), vec![])
        }
        _ => (
            JsonRpcResponse::err(
                request.id.clone(),
                -32601,
                format!("Method not found: {}", request.method),
            ),
            vec![],
        ),
    }
}

fn handle_initialize(request: &JsonRpcRequest) -> JsonRpcResponse {
    tracing::info!("Handling initialize");

    JsonRpcResponse::ok(
        request.id.clone(),
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "mgp-discord",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
}

fn handle_tools_list(request: &JsonRpcRequest) -> JsonRpcResponse {
    let tools: Vec<Value> = tools::tool_list()
        .into_iter()
        .map(|t| serde_json::to_value(t).unwrap())
        .collect();

    JsonRpcResponse::ok(request.id.clone(), json!({ "tools": tools }))
}

async fn handle_tools_call(
    request: &JsonRpcRequest,
    http: &Option<Arc<serenity::Http>>,
    config: &Arc<DiscordConfig>,
    bot_context: &BotContext,
) -> (JsonRpcResponse, Vec<JsonRpcNotification>) {
    let Some(params) = &request.params else {
        return (
            JsonRpcResponse::err(request.id.clone(), -32602, "Missing params"),
            vec![],
        );
    };

    let Some(tool_name) = params.get("name").and_then(|v| v.as_str()) else {
        return (
            JsonRpcResponse::err(request.id.clone(), -32602, "Missing tool name"),
            vec![],
        );
    };

    // set_presence doesn't require http
    if tool_name == "set_presence" {
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        return match tools::execute(tool_name, &args, &Arc::new(serenity::Http::new("")), config, bot_context).await {
            Ok((result, notifications)) => (
                JsonRpcResponse::ok(request.id.clone(), result),
                notifications,
            ),
            Err(msg) => (
                JsonRpcResponse::err(request.id.clone(), -32000, msg),
                vec![],
            ),
        };
    }

    let Some(http) = http else {
        return (
            JsonRpcResponse::err(
                request.id.clone(),
                -32000,
                "Discord not connected (DISCORD_BOT_TOKEN not set or connection failed)",
            ),
            vec![],
        );
    };

    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(serde_json::Map::new()));

    match tools::execute(tool_name, &args, http, config, bot_context).await {
        Ok((result, notifications)) => (
            JsonRpcResponse::ok(request.id.clone(), result),
            notifications,
        ),
        Err(msg) => (
            JsonRpcResponse::err(request.id.clone(), -32000, msg),
            vec![],
        ),
    }
}

/// Handle mgp/callback/respond from the kernel — auto-send response to Discord.
async fn handle_callback_respond(
    request: &JsonRpcRequest,
    http: &Option<Arc<serenity::Http>>,
    config: &Arc<DiscordConfig>,
    pending_callbacks: &PendingCallbacks,
) -> JsonRpcResponse {
    let params = match &request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::err(request.id.clone(), -32602, "Missing params");
        }
    };

    let callback_id = params
        .get("callback_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if callback_id.is_empty() {
        return JsonRpcResponse::err(request.id.clone(), -32602, "callback_id is required");
    }

    // Always remove the callback context first to stop the typing indicator.
    let ctx = pending_callbacks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(callback_id);

    let Some(ctx) = ctx else {
        tracing::warn!(callback_id = %callback_id, "No pending callback found");
        return JsonRpcResponse::ok(
            request.id.clone(),
            json!({"status": "not_found", "callback_id": callback_id}),
        );
    };

    let response = params
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let channel_id: u64 = ctx.channel_id.parse().unwrap_or(0);
    let message_id: u64 = ctx.message_id.parse().unwrap_or(0);

    // Empty response: clean up typing but skip sending
    if response.is_empty() {
        tracing::info!(callback_id = %callback_id, "Empty response — typing stopped");
        return JsonRpcResponse::ok(
            request.id.clone(),
            json!({"status": "empty", "callback_id": callback_id}),
        );
    }

    let Some(http) = http else {
        return JsonRpcResponse::err(request.id.clone(), -32000, "Discord not connected");
    };

    // Send as reply to the original message
    let send_args = json!({
        "channel_id": ctx.channel_id,
        "content": response,
        "reply_to": ctx.message_id,
    });

    // Dummy bot_context (not needed for send_message)
    let dummy_ctx: BotContext = Arc::new(std::sync::Mutex::new(None));

    match tools::execute("send_message", &send_args, http, config, &dummy_ctx).await {
        Ok(_) => {
            // Add completion reaction
            if channel_id > 0 && message_id > 0 {
                let emoji =
                    serenity::ReactionType::Unicode(config.reaction_done.clone());
                let _ = http
                    .create_reaction(
                        serenity::ChannelId::new(channel_id),
                        serenity::MessageId::new(message_id),
                        &emoji,
                    )
                    .await;
                // Remove processing reaction
                let processing_emoji =
                    serenity::ReactionType::Unicode(config.reaction_processing.clone());
                let _ = http
                    .delete_reaction_me(
                        serenity::ChannelId::new(channel_id),
                        serenity::MessageId::new(message_id),
                        &processing_emoji,
                    )
                    .await;
            }

            tracing::info!(
                callback_id = %callback_id,
                channel_id = %ctx.channel_id,
                "Callback response sent to Discord"
            );
            JsonRpcResponse::ok(
                request.id.clone(),
                json!({"status": "sent", "callback_id": callback_id, "channel_id": ctx.channel_id}),
            )
        }
        Err(e) => {
            // Add error reaction
            if channel_id > 0 && message_id > 0 {
                let emoji =
                    serenity::ReactionType::Unicode(config.reaction_error.clone());
                let _ = http
                    .create_reaction(
                        serenity::ChannelId::new(channel_id),
                        serenity::MessageId::new(message_id),
                        &emoji,
                    )
                    .await;
            }

            tracing::error!(
                callback_id = %callback_id,
                error = %e,
                "Failed to send callback response to Discord"
            );
            JsonRpcResponse::err(
                request.id.clone(),
                -32000,
                format!("Discord send failed: {e}"),
            )
        }
    }
}

async fn handle_discord_event(
    event: DiscordEvent,
    stdout: &Arc<Mutex<io::Stdout>>,
    pending_callbacks: &PendingCallbacks,
    http: &Option<Arc<serenity::Http>>,
    config: &Arc<DiscordConfig>,
    bot_user_id: &Arc<std::sync::atomic::AtomicU64>,
) {
    match event {
        DiscordEvent::MessageCreate(msg) => {
            let callback_id = format!("discord-{}", msg.message_id);

            let channel_id: u64 = msg.channel_id.parse().unwrap_or(0);
            let msg_id: u64 = msg.message_id.parse().unwrap_or(0);

            // Start typing indicator
            let typing = if let Some(http) = http {
                if channel_id > 0 {
                    let cid = serenity::ChannelId::new(channel_id);
                    Some(cid.start_typing(http))
                } else {
                    None
                }
            } else {
                None
            };

            // Add processing reaction
            if let Some(http) = http {
                if channel_id > 0 && msg_id > 0 {
                    let emoji =
                        serenity::ReactionType::Unicode(config.reaction_processing.clone());
                    let _ = http
                        .create_reaction(
                            serenity::ChannelId::new(channel_id),
                            serenity::MessageId::new(msg_id),
                            &emoji,
                        )
                        .await;
                }
            }

            // Register pending callback context
            if let Ok(mut cbs) = pending_callbacks.lock() {
                cbs.insert(
                    callback_id.clone(),
                    CallbackContext {
                        channel_id: msg.channel_id.clone(),
                        guild_id: msg.guild_id.clone(),
                        message_id: msg.message_id.clone(),
                        author_name: msg.author_name.clone(),
                        _typing: typing,
                    },
                );
            }

            // Fetch recent channel history for short-term conversation context
            // Reduce context for short messages to prevent history from dominating
            let effective_limit = if msg.content.len() < 20 {
                config.context_history_limit.min(5)
            } else {
                config.context_history_limit
            };
            let conversation_context = if effective_limit > 0 {
                if let Some(http) = http {
                    if channel_id > 0 && msg_id > 0 {
                        let cid = serenity::ChannelId::new(channel_id);
                        let mid = serenity::MessageId::new(msg_id);
                        let builder = serenity::GetMessages::new()
                            .before(mid)
                            .limit(effective_limit);
                        match cid.messages(http, builder).await {
                            Ok(messages) => {
                                let bot_id =
                                    bot_user_id.load(std::sync::atomic::Ordering::Relaxed);
                                messages
                                    .iter()
                                    .rev()
                                    .filter(|m| !m.content.is_empty())
                                    .map(|m| {
                                        if m.author.id.get() == bot_id {
                                            json!({
                                                "role": "assistant",
                                                "content": utils::truncate_str(&m.content, 500),
                                            })
                                        } else {
                                            json!({
                                                "role": "user",
                                                "name": m.author.name,
                                                "content": utils::truncate_str(&m.content, 500),
                                            })
                                        }
                                    })
                                    .collect::<Vec<_>>()
                            }
                            Err(e) => {
                                tracing::warn!("Failed to fetch channel history: {e}");
                                vec![]
                            }
                        }
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

            // Build message content with image attachment info
            let mut message_content = msg.content.clone();
            let image_attachments: Vec<_> = msg
                .attachments
                .iter()
                .filter(|a| {
                    a.content_type
                        .as_deref()
                        .is_some_and(|ct| ct.starts_with("image/"))
                })
                .collect();
            if !image_attachments.is_empty() {
                let image_lines: Vec<String> = image_attachments
                    .iter()
                    .map(|a| format!("- {} ({})", a.url, a.filename))
                    .collect();
                message_content = format!(
                    "{}\n\n[Attached Images]\n{}",
                    message_content,
                    image_lines.join("\n")
                );
            }

            // Emit callback request (MGP §13)
            let notif = JsonRpcNotification::new(
                "notifications/mgp.callback.request",
                Some(json!({
                    "callback_id": callback_id,
                    "type": "external_message",
                    "message": message_content,
                    "metadata": {
                        "source": "discord",
                        "author_id": msg.author_id,
                        "author_name": msg.author_name,
                        "channel_id": msg.channel_id,
                        "guild_id": msg.guild_id,
                        "message_id": msg.message_id,
                        "timestamp": msg.timestamp,
                        "attachments": msg.attachments.iter().map(|a| json!({
                            "url": a.url,
                            "filename": a.filename,
                            "size": a.size,
                            "content_type": a.content_type,
                        })).collect::<Vec<_>>(),
                        "reference": msg.reference.as_ref().map(|r| json!({
                            "author_name": r.author_name,
                            "content": r.content,
                        })),
                        "conversation_context": conversation_context,
                    }
                })),
            );
            write_message(stdout, &notif);
        }
        DiscordEvent::Ready(data) => {
            bot_user_id.store(data.bot_user_id, std::sync::atomic::Ordering::Relaxed);

            let notif = JsonRpcNotification::new(
                "notifications/mgp.lifecycle",
                Some(json!({
                    "server_id": "io.discord",
                    "previous_state": "connecting",
                    "new_state": "connected",
                    "reason": format!(
                        "Connected as {} ({} guilds)",
                        data.username, data.guild_count
                    ),
                })),
            );
            write_message(stdout, &notif);
        }
        DiscordEvent::Resumed => {
            let notif = JsonRpcNotification::new(
                "notifications/mgp.lifecycle",
                Some(json!({
                    "server_id": "io.discord",
                    "previous_state": "reconnecting",
                    "new_state": "connected",
                    "reason": "Gateway session resumed",
                })),
            );
            write_message(stdout, &notif);
        }
    }
}

fn write_message<T: serde::Serialize>(stdout: &Arc<Mutex<io::Stdout>>, msg: &T) {
    if let Ok(json_str) = serde_json::to_string(msg) {
        if let Ok(mut out) = stdout.lock() {
            let _ = writeln!(out, "{json_str}");
            let _ = out.flush();
        }
    }
}
