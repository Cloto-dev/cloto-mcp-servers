//! MGP Discord Server v0.3.0 — Bidirectional Discord communication.
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
    /// Whether the original message was a reply to a bot message.
    /// If true, the response uses reply format; otherwise, a normal message.
    is_reply: bool,
    /// Typing indicator guard — dropping this stops the typing indicator.
    _typing: Option<serenity::http::Typing>,
}

type PendingCallbacks = Arc<Mutex<HashMap<String, CallbackContext>>>;
type BotContext = Arc<std::sync::Mutex<Option<serenity::Context>>>;

/// Internal counters for bridge health monitoring.
struct BridgeStats {
    connected_since: std::sync::Mutex<Option<std::time::Instant>>,
    messages_received: std::sync::atomic::AtomicU64,
    messages_sent: std::sync::atomic::AtomicU64,
    errors: std::sync::atomic::AtomicU64,
    last_event_at: std::sync::Mutex<Option<std::time::Instant>>,
    start_time: std::time::Instant,
}

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

    tracing::info!("MGP Discord Server v0.3.0 starting");

    let bridge_stats = Arc::new(BridgeStats {
        connected_since: std::sync::Mutex::new(None),
        messages_received: std::sync::atomic::AtomicU64::new(0),
        messages_sent: std::sync::atomic::AtomicU64::new(0),
        errors: std::sync::atomic::AtomicU64::new(0),
        last_event_at: std::sync::Mutex::new(None),
        start_time: std::time::Instant::now(),
    });

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
            direct_tool_users: config.direct_tool_users.clone(),
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
                handle_stdin_line(&line, &stdout, &http, &config, &pending_callbacks, &bot_context, &bridge_stats).await;
            }
            Some(event) = discord_rx.recv() => {
                handle_discord_event(event, &stdout, &pending_callbacks, &http, &config, &bot_user_id, &bridge_stats).await;
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
    bridge_stats: &Arc<BridgeStats>,
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

    let (response, notifications) = dispatch(
        &request,
        http,
        config,
        pending_callbacks,
        bot_context,
        bridge_stats,
    )
    .await;

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
    bridge_stats: &Arc<BridgeStats>,
) -> (JsonRpcResponse, Vec<JsonRpcNotification>) {
    match request.method.as_str() {
        "initialize" => (handle_initialize(request), vec![]),
        "tools/list" => (handle_tools_list(request), vec![]),
        "tools/call" => handle_tools_call(request, http, config, bot_context, bridge_stats).await,
        "mgp/callback/respond" => (
            handle_callback_respond(request, http, config, pending_callbacks, bridge_stats).await,
            vec![],
        ),
        "notifications/initialized" => (JsonRpcResponse::ok(request.id.clone(), json!({})), vec![]),
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
    bridge_stats: &Arc<BridgeStats>,
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

    // bridge_status: handled locally (needs bridge_stats)
    if tool_name == "bridge_status" {
        let uptime = bridge_stats.start_time.elapsed().as_secs();
        let connected_secs = bridge_stats
            .connected_since
            .lock()
            .ok()
            .and_then(|g| g.map(|t| t.elapsed().as_secs()));
        let last_event_secs = bridge_stats
            .last_event_at
            .lock()
            .ok()
            .and_then(|g| g.map(|t| t.elapsed().as_secs()));
        let result = json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&json!({
                    "status": if connected_secs.is_some() { "connected" } else { "disconnected" },
                    "uptime_seconds": uptime,
                    "connected_seconds": connected_secs,
                    "messages_received": bridge_stats.messages_received.load(std::sync::atomic::Ordering::Relaxed),
                    "messages_sent": bridge_stats.messages_sent.load(std::sync::atomic::Ordering::Relaxed),
                    "errors": bridge_stats.errors.load(std::sync::atomic::Ordering::Relaxed),
                    "last_event_seconds_ago": last_event_secs,
                })).unwrap_or_default()
            }]
        });
        return (JsonRpcResponse::ok(request.id.clone(), result), vec![]);
    }

    // set_presence doesn't require http
    if tool_name == "set_presence" {
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        return match tools::execute(
            tool_name,
            &args,
            &Arc::new(serenity::Http::new("")),
            config,
            bot_context,
        )
        .await
        {
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
    bridge_stats: &Arc<BridgeStats>,
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

    // Extract values and immediately drop ctx to stop typing indicator.
    // Serenity's Typing guard runs a background task that POSTs typing every ~9s;
    // dropping it aborts the task. Holding ctx until function end causes typing to
    // persist through the entire send_message + reaction flow.
    let ctx_channel_id = ctx.channel_id.clone();
    let ctx_message_id = ctx.message_id.clone();
    let ctx_is_reply = ctx.is_reply;
    drop(ctx); // ← _typing dropped here → typing stops immediately

    let response = params
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let channel_id: u64 = ctx_channel_id.parse().unwrap_or(0);
    let message_id: u64 = ctx_message_id.parse().unwrap_or(0);

    // Empty response: typing already stopped above
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

    // Reply format only when the user replied to a bot message;
    // mention-triggered messages get a normal (non-reply) response.
    let mut send_args = json!({
        "channel_id": ctx_channel_id,
        "content": response,
    });
    if ctx_is_reply {
        send_args
            .as_object_mut()
            .unwrap()
            .insert("reply_to".into(), json!(ctx_message_id));
    }

    // Dummy bot_context (not needed for send_message)
    let dummy_ctx: BotContext = Arc::new(std::sync::Mutex::new(None));

    match tools::execute("send_message", &send_args, http, config, &dummy_ctx).await {
        Ok(_) => {
            bridge_stats
                .messages_sent
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Add completion reaction
            if channel_id > 0 && message_id > 0 {
                let emoji = serenity::ReactionType::Unicode(config.reaction_done.clone());
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
                channel_id = %ctx_channel_id,
                "Callback response sent to Discord"
            );
            JsonRpcResponse::ok(
                request.id.clone(),
                json!({"status": "sent", "callback_id": callback_id, "channel_id": ctx_channel_id}),
            )
        }
        Err(e) => {
            bridge_stats
                .errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // Add error reaction
            if channel_id > 0 && message_id > 0 {
                let emoji = serenity::ReactionType::Unicode(config.reaction_error.clone());
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
    bridge_stats: &Arc<BridgeStats>,
) {
    // Update last event timestamp
    if let Ok(mut g) = bridge_stats.last_event_at.lock() {
        *g = Some(std::time::Instant::now());
    }

    match event {
        DiscordEvent::MessageCreate(msg) => {
            bridge_stats
                .messages_received
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let channel_id: u64 = msg.channel_id.parse().unwrap_or(0);
            let msg_id: u64 = msg.message_id.parse().unwrap_or(0);

            // ── Direct tool execution (backtick commands) ──
            let author_id: u64 = msg.author_id.parse().unwrap_or(0);
            if config.direct_tool_users.contains(&author_id) {
                let all_known: Vec<&str> = tools::BRIDGE_TOOL_NAMES
                    .iter()
                    .copied()
                    .chain(config.direct_tool_ecosystem.iter().map(|s| s.as_str()))
                    .collect();

                if let Some((tool_name, args_map)) =
                    utils::parse_direct_command(&msg.content, &all_known)
                {
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

                    if tools::BRIDGE_TOOL_NAMES.contains(&tool_name.as_str()) {
                        // F1: Local execution for bridge-native tools
                        if let Some(http) = http {
                            let args_json: Value = args_map
                                .iter()
                                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                                .collect::<serde_json::Map<String, Value>>()
                                .into();

                            let dummy_ctx: BotContext = Arc::new(std::sync::Mutex::new(None));
                            let (reaction, result_text) = match tools::execute(
                                &tool_name, &args_json, http, config, &dummy_ctx,
                            )
                            .await
                            {
                                Ok((result, _)) => {
                                    let text = result
                                        .get("content")
                                        .and_then(|c| c.as_array())
                                        .and_then(|a| a.first())
                                        .and_then(|e| e.get("text"))
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("(no result)")
                                        .to_string();
                                    (config.reaction_done.clone(), text)
                                }
                                Err(e) => {
                                    bridge_stats
                                        .errors
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    (config.reaction_error.clone(), format!("Error: {e}"))
                                }
                            };

                            // Send result as reply
                            let send_args = json!({
                                "channel_id": msg.channel_id,
                                "content": result_text,
                                "reply_to": msg.message_id,
                            });
                            let dummy_ctx2: BotContext = Arc::new(std::sync::Mutex::new(None));
                            let _ = tools::execute(
                                "send_message",
                                &send_args,
                                http,
                                config,
                                &dummy_ctx2,
                            )
                            .await;
                            bridge_stats
                                .messages_sent
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                            // Swap processing → done/error reaction
                            if channel_id > 0 && msg_id > 0 {
                                let done_emoji = serenity::ReactionType::Unicode(reaction);
                                let _ = http
                                    .create_reaction(
                                        serenity::ChannelId::new(channel_id),
                                        serenity::MessageId::new(msg_id),
                                        &done_emoji,
                                    )
                                    .await;
                                let processing_emoji = serenity::ReactionType::Unicode(
                                    config.reaction_processing.clone(),
                                );
                                let _ = http
                                    .delete_reaction_me(
                                        serenity::ChannelId::new(channel_id),
                                        serenity::MessageId::new(msg_id),
                                        &processing_emoji,
                                    )
                                    .await;
                            }
                        }
                        return; // Skip normal callback flow
                    }
                    // F2: Ecosystem tool — fall through with tool_hint metadata
                    // (handled below by injecting tool_hint into callback metadata)
                }
            }

            // ── Normal callback flow ──
            let callback_id = format!("discord-{}", msg.message_id);

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

            // Add processing reaction (if not already added by direct tool path)
            if let Some(http) = http {
                if channel_id > 0 && msg_id > 0 {
                    let emoji = serenity::ReactionType::Unicode(config.reaction_processing.clone());
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
                        is_reply: msg.reference.is_some(),
                        _typing: typing,
                    },
                );
            }

            // Fetch recent channel history for short-term conversation context
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
                                let bot_id = bot_user_id.load(std::sync::atomic::Ordering::Relaxed);
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

            // Prefix message with author name so the LLM clearly knows who is
            // speaking, even when conversation_context contains other users' messages.
            let mut message_content = format!("[{}] {}", msg.author_name, msg.content);
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

            // Check for F2 ecosystem tool_hint (direct command that wasn't bridge-native)
            let mut extra_metadata = serde_json::Map::new();
            if config.direct_tool_users.contains(&author_id) {
                let all_known: Vec<&str> = tools::BRIDGE_TOOL_NAMES
                    .iter()
                    .copied()
                    .chain(config.direct_tool_ecosystem.iter().map(|s| s.as_str()))
                    .collect();
                if let Some((tool_name, args_map)) =
                    utils::parse_direct_command(&msg.content, &all_known)
                {
                    extra_metadata.insert("tool_hint".into(), json!(tool_name));
                    let tool_args: serde_json::Map<String, Value> = args_map
                        .iter()
                        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                        .collect();
                    extra_metadata.insert(
                        "tool_args".into(),
                        json!(serde_json::to_string(&tool_args).unwrap_or_default()),
                    );
                }
            }

            // Build callback metadata
            let mut metadata = json!({
                "source": "discord",
                "author_id": msg.author_id,
                "author_name": msg.author_name,
                "author_roles": msg.author_roles,
                "channel_id": msg.channel_id,
                "guild_id": msg.guild_id,
                "guild_name": msg.guild_name,
                "message_id": msg.message_id,
                "timestamp": msg.timestamp,
                "is_thread": msg.thread_info.is_some(),
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
            });
            // Add thread info
            if let Some(ref ti) = msg.thread_info {
                metadata.as_object_mut().unwrap().insert(
                    "thread_info".into(),
                    json!({
                        "parent_channel_id": ti.parent_id,
                        "thread_name": ti.thread_name,
                        "archived": ti.archived,
                    }),
                );
            }
            // Add F2 tool_hint metadata
            for (k, v) in &extra_metadata {
                metadata
                    .as_object_mut()
                    .unwrap()
                    .insert(k.clone(), v.clone());
            }

            // Emit callback request (MGP §13)
            let notif = JsonRpcNotification::new(
                "notifications/mgp.callback.request",
                Some(json!({
                    "callback_id": callback_id,
                    "type": "external_message",
                    "message": message_content,
                    "metadata": metadata,
                })),
            );
            write_message(stdout, &notif);
        }
        DiscordEvent::Ready(data) => {
            bot_user_id.store(data.bot_user_id, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut g) = bridge_stats.connected_since.lock() {
                *g = Some(std::time::Instant::now());
            }

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
        DiscordEvent::ShardStageUpdate { old, new } => {
            tracing::info!("Shard stage: {old} → {new}");
            let notif = JsonRpcNotification::new(
                "notifications/mgp.lifecycle",
                Some(json!({
                    "server_id": "io.discord",
                    "previous_state": old,
                    "new_state": new,
                    "reason": "Shard stage update",
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
