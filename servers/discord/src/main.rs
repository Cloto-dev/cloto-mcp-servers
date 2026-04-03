//! MGP Discord Server v0.4.0 — Bidirectional Discord communication.
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
mod queue;
mod rate_limiter;
mod tools;
mod utils;

use config::DiscordConfig;
use handler::{DiscordEvent, DiscordHandler};
use protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use queue::{EnqueueResult, MessageQueue, QueueEntry};
use rate_limiter::RateLimiter;
use serde_json::{json, Value};
use serenity::all as serenity;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

/// How the callback response should be delivered.
enum ResponseMode {
    /// Regular channel message (with optional reply-to).
    Message { is_reply: bool },
    /// Edit a deferred interaction response.
    Interaction { token: String },
}

/// Context stored for pending callbacks awaiting kernel response.
struct CallbackContext {
    channel_id: String,
    #[allow(dead_code)]
    guild_id: Option<String>,
    message_id: String,
    #[allow(dead_code)]
    author_name: String,
    /// How to deliver the response.
    response_mode: ResponseMode,
    /// Typing indicator guard — dropping this stops the typing indicator.
    _typing: Option<serenity::http::Typing>,
}

type PendingCallbacks = Arc<Mutex<HashMap<String, CallbackContext>>>;
type BotContext = Arc<std::sync::Mutex<Option<serenity::Context>>>;
type SharedQueue = Arc<Mutex<MessageQueue>>;

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

    tracing::info!("MGP Discord Server v0.4.0 starting");

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
                    let mut backoff_secs = 1u64;
                    const MAX_BACKOFF: u64 = 60;
                    loop {
                        match client.start().await {
                            Ok(()) => {
                                tracing::info!("Serenity client exited cleanly");
                                break;
                            }
                            Err(e) => {
                                tracing::error!(
                                    "Serenity client error (reconnecting in {backoff_secs}s): {e}"
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(backoff_secs))
                                    .await;
                                backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF);
                            }
                        }
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
    let rate_limiter = Arc::new(RateLimiter::new());
    let message_queue = Arc::new(Mutex::new(MessageQueue::with_config(
        config.queue_max_size,
        config.queue_timeout_secs,
    )));

    // Periodic intervals
    let mut cleanup_interval = tokio::time::interval(std::time::Duration::from_secs(300));
    cleanup_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut queue_timeout_interval = tokio::time::interval(std::time::Duration::from_secs(15));
    queue_timeout_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Main loop: multiplex stdin JSON-RPC and Discord Gateway events
    loop {
        tokio::select! {
            Some(line) = stdin_rx.recv() => {
                handle_stdin_line(&line, &stdout, &http, &config, &pending_callbacks, &bot_context, &bridge_stats, &rate_limiter, &message_queue).await;
            }
            Some(event) = discord_rx.recv() => {
                handle_discord_event(event, &stdout, &pending_callbacks, &http, &config, &bot_user_id, &bridge_stats, &rate_limiter, &message_queue).await;
            }
            _ = cleanup_interval.tick() => {
                rate_limiter.cleanup().await;
            }
            _ = queue_timeout_interval.tick() => {
                handle_queue_timeouts(&message_queue, &http, &config, &rate_limiter).await;
            }
            else => break,
        }
    }

    tracing::info!("MGP Discord Server shutting down");
}

#[allow(clippy::too_many_arguments)]
async fn handle_stdin_line(
    line: &str,
    stdout: &Arc<Mutex<io::Stdout>>,
    http: &Option<Arc<serenity::Http>>,
    config: &Arc<DiscordConfig>,
    pending_callbacks: &PendingCallbacks,
    bot_context: &BotContext,
    bridge_stats: &Arc<BridgeStats>,
    rate_limiter: &Arc<RateLimiter>,
    message_queue: &SharedQueue,
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
        rate_limiter,
        message_queue,
        stdout,
    )
    .await;

    // Emit notifications BEFORE the response (avatar pattern)
    for notif in &notifications {
        write_message(stdout, notif);
    }
    write_message(stdout, &response);
}

#[allow(clippy::too_many_arguments)]
async fn dispatch(
    request: &JsonRpcRequest,
    http: &Option<Arc<serenity::Http>>,
    config: &Arc<DiscordConfig>,
    pending_callbacks: &PendingCallbacks,
    bot_context: &BotContext,
    bridge_stats: &Arc<BridgeStats>,
    rate_limiter: &Arc<RateLimiter>,
    message_queue: &SharedQueue,
    stdout: &Arc<Mutex<io::Stdout>>,
) -> (JsonRpcResponse, Vec<JsonRpcNotification>) {
    match request.method.as_str() {
        "initialize" => (handle_initialize(request), vec![]),
        "tools/list" => (handle_tools_list(request), vec![]),
        "tools/call" => {
            handle_tools_call(request, http, config, bot_context, bridge_stats, rate_limiter).await
        }
        "mgp/callback/respond" => (
            handle_callback_respond(
                request,
                http,
                config,
                pending_callbacks,
                bridge_stats,
                rate_limiter,
                message_queue,
                stdout,
            )
            .await,
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
    rate_limiter: &Arc<RateLimiter>,
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
            rate_limiter,
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

    match tools::execute(tool_name, &args, http, config, bot_context, rate_limiter).await {
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
#[allow(clippy::too_many_arguments)]
async fn handle_callback_respond(
    request: &JsonRpcRequest,
    http: &Option<Arc<serenity::Http>>,
    config: &Arc<DiscordConfig>,
    pending_callbacks: &PendingCallbacks,
    bridge_stats: &Arc<BridgeStats>,
    rate_limiter: &Arc<RateLimiter>,
    message_queue: &SharedQueue,
    stdout: &Arc<Mutex<io::Stdout>>,
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

    let Some(mut ctx) = ctx else {
        tracing::warn!(callback_id = %callback_id, "No pending callback found");
        return JsonRpcResponse::ok(
            request.id.clone(),
            json!({"status": "not_found", "callback_id": callback_id}),
        );
    };

    // Extract values and immediately drop ctx to stop typing indicator.
    let ctx_channel_id = ctx.channel_id.clone();
    let ctx_message_id = ctx.message_id.clone();
    // Take response_mode out by replacing with a dummy, then drop ctx to stop typing
    let ctx_response_mode = std::mem::replace(
        &mut ctx.response_mode,
        ResponseMode::Message { is_reply: false },
    );
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

    let http_opt = http; // Preserve original Option for queue processing
    let Some(http) = http else {
        return JsonRpcResponse::err(request.id.clone(), -32000, "Discord not connected");
    };

    let result = match ctx_response_mode {
        ResponseMode::Interaction { token } => {
            // Edit the deferred interaction response
            let edit = serenity::EditInteractionResponse::new().content(response);
            match http.edit_original_interaction_response(&token, &edit, vec![]).await {
                Ok(_) => {
                    bridge_stats
                        .messages_sent
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::info!(
                        callback_id = %callback_id,
                        "Interaction response sent"
                    );
                    JsonRpcResponse::ok(
                        request.id.clone(),
                        json!({"status": "sent", "callback_id": callback_id, "channel_id": ctx_channel_id, "mode": "interaction"}),
                    )
                }
                Err(e) => {
                    bridge_stats
                        .errors
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::error!(callback_id = %callback_id, error = %e, "Failed to edit interaction response");
                    JsonRpcResponse::err(
                        request.id.clone(),
                        -32000,
                        format!("Interaction edit failed: {e}"),
                    )
                }
            }
        }
        ResponseMode::Message { is_reply } => {
            // Regular channel message
            let mut send_args = json!({
                "channel_id": ctx_channel_id,
                "content": response,
            });
            if is_reply {
                send_args
                    .as_object_mut()
                    .unwrap()
                    .insert("reply_to".into(), json!(ctx_message_id));
            }

            let dummy_ctx: BotContext = Arc::new(std::sync::Mutex::new(None));
            match tools::execute("send_message", &send_args, http, config, &dummy_ctx, rate_limiter).await {
                Ok(_) => {
                    bridge_stats
                        .messages_sent
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if channel_id > 0 && message_id > 0 {
                        let emoji = serenity::ReactionType::Unicode(config.reaction_done.clone());
                        let _ = http
                            .create_reaction(
                                serenity::ChannelId::new(channel_id),
                                serenity::MessageId::new(message_id),
                                &emoji,
                            )
                            .await;
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
                    tracing::error!(callback_id = %callback_id, error = %e, "Failed to send callback response");
                    JsonRpcResponse::err(
                        request.id.clone(),
                        -32000,
                        format!("Discord send failed: {e}"),
                    )
                }
            }
        }
    };

    // Dequeue next item and start processing
    process_next_in_queue(message_queue, http_opt, config, pending_callbacks, rate_limiter, stdout).await;

    result
}

#[allow(clippy::too_many_arguments)]
async fn handle_discord_event(
    event: DiscordEvent,
    stdout: &Arc<Mutex<io::Stdout>>,
    pending_callbacks: &PendingCallbacks,
    http: &Option<Arc<serenity::Http>>,
    config: &Arc<DiscordConfig>,
    bot_user_id: &Arc<std::sync::atomic::AtomicU64>,
    bridge_stats: &Arc<BridgeStats>,
    rate_limiter: &Arc<RateLimiter>,
    message_queue: &SharedQueue,
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
                                &tool_name, &args_json, http, config, &dummy_ctx, rate_limiter,
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
                                rate_limiter,
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

            // ── Normal callback flow (with queue) ──
            // Session = (channel_id, user_id) — physically separates conversation context
            let session_id = format!("{}:{}", msg.channel_id, msg.author_id);
            let callback_id = format!(
                "discord-{}-{}-{}",
                msg.channel_id, msg.author_id, msg.message_id
            );

            // Build the notification payload (used immediately or stored in queue)
            let notification_payload = build_callback_payload(
                &callback_id,
                &session_id,
                &msg,
                http,
                config,
                bot_user_id,
                author_id,
            )
            .await;

            // Try to enqueue
            let enqueue_result = {
                let entry = QueueEntry {
                    callback_id: callback_id.clone(),
                    session_id: session_id.clone(),
                    channel_id: msg.channel_id.clone(),
                    original_message_id: msg.message_id.clone(),
                    waiting_message_id: None,
                    guild_id: msg.guild_id.clone(),
                    author_name: msg.author_name.clone(),
                    author_id: msg.author_id.clone(),
                    is_reply: msg.reference.is_some(),
                    interaction_token: None,
                    notification_payload,
                    enqueued_at: std::time::Instant::now(),
                };
                message_queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .try_enqueue(entry)
            };

            match enqueue_result {
                EnqueueResult::ProcessNow => {
                    // Idle — process immediately
                    let typing = if let Some(http) = http {
                        if channel_id > 0 {
                            Some(serenity::ChannelId::new(channel_id).start_typing(http))
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // Add processing reaction
                    if let Some(http) = http {
                        if channel_id > 0 && msg_id > 0 {
                            let emoji = serenity::ReactionType::Unicode(
                                config.reaction_processing.clone(),
                            );
                            let _ = http
                                .create_reaction(
                                    serenity::ChannelId::new(channel_id),
                                    serenity::MessageId::new(msg_id),
                                    &emoji,
                                )
                                .await;
                        }
                    }

                    // Register pending callback
                    if let Ok(mut cbs) = pending_callbacks.lock() {
                        cbs.insert(
                            callback_id.clone(),
                            CallbackContext {
                                channel_id: msg.channel_id.clone(),
                                guild_id: msg.guild_id.clone(),
                                message_id: msg.message_id.clone(),
                                author_name: msg.author_name.clone(),
                                response_mode: ResponseMode::Message {
                                    is_reply: msg.reference.is_some(),
                                },
                                _typing: typing,
                            },
                        );
                    }

                    // Re-retrieve the payload from the queue to emit
                    // (the entry was consumed by try_enqueue returning ProcessNow,
                    //  so we rebuild it — the entry itself wasn't stored)
                    let payload = build_callback_payload(
                        &callback_id,
                        &session_id,
                        &msg,
                        http,
                        config,
                        bot_user_id,
                        author_id,
                    )
                    .await;
                    let notif = JsonRpcNotification::new(
                        "notifications/mgp.callback.request",
                        Some(payload),
                    );
                    write_message(stdout, &notif);
                }
                EnqueueResult::Queued(position) => {
                    // Busy — send waiting message and store in queue
                    if let Some(http) = http {
                        // Add queue reaction to original message
                        if channel_id > 0 && msg_id > 0 {
                            let emoji = serenity::ReactionType::Unicode(
                                config.reaction_queued.clone(),
                            );
                            let _ = http
                                .create_reaction(
                                    serenity::ChannelId::new(channel_id),
                                    serenity::MessageId::new(msg_id),
                                    &emoji,
                                )
                                .await;
                        }

                        // Send waiting indicator message
                        let wait_text = format!(
                            "⏳ 待機中です（{}番目）。順番が来たら応答します。",
                            position
                        );
                        let cid = serenity::ChannelId::new(channel_id);
                        let mut msg_builder = serenity::CreateMessage::new().content(&wait_text);
                        if msg.reference.is_some() {
                            msg_builder = msg_builder.reference_message(
                                serenity::MessageReference::from((
                                    cid,
                                    serenity::MessageId::new(msg_id),
                                )),
                            );
                        }
                        rate_limiter
                            .acquire(rate_limiter::Route::ChannelMessage(channel_id))
                            .await;
                        if let Ok(sent) = cid.send_message(http, msg_builder).await {
                            // Update the queue entry with the waiting message ID
                            if let Ok(mut q) = message_queue.lock() {
                                for entry in q.waiting_iter_mut() {
                                    if entry.callback_id == callback_id {
                                        entry.waiting_message_id = Some(sent.id.to_string());
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    tracing::info!(
                        callback_id = %callback_id,
                        position = position,
                        "Message queued"
                    );
                }
                EnqueueResult::Full => {
                    // Queue full — notify user
                    if let Some(http) = http {
                        let full_text =
                            "⚠️ 現在キューが満杯です。しばらく待ってからお試しください。";
                        let cid = serenity::ChannelId::new(channel_id);
                        let msg_builder = serenity::CreateMessage::new().content(full_text);
                        rate_limiter
                            .acquire(rate_limiter::Route::ChannelMessage(channel_id))
                            .await;
                        let _ = cid.send_message(http, msg_builder).await;
                    }
                    tracing::warn!(
                        callback_id = %callback_id,
                        "Queue full — message rejected"
                    );
                }
            }
        }
        DiscordEvent::InteractionCreate(interaction) => {
            bridge_stats
                .messages_received
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            // Handle /status locally
            if interaction.command_name == "status" {
                let uptime = bridge_stats.start_time.elapsed().as_secs();
                let queue_info = message_queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .status();
                let status_text = format!(
                    "**Bridge Status**\nUptime: {}s\nMessages: {} recv / {} sent\nErrors: {}\nQueue: {}/{} (active: {})",
                    uptime,
                    bridge_stats.messages_received.load(std::sync::atomic::Ordering::Relaxed),
                    bridge_stats.messages_sent.load(std::sync::atomic::Ordering::Relaxed),
                    bridge_stats.errors.load(std::sync::atomic::Ordering::Relaxed),
                    queue_info.waiting,
                    queue_info.max_size,
                    queue_info.active,
                );
                if let Some(http) = http {
                    let edit = serenity::EditInteractionResponse::new().content(&status_text);
                    let _ = http
                        .edit_original_interaction_response(&interaction.interaction_token, &edit, vec![])
                        .await;
                }
                return;
            }

            // /chat — route through callback flow
            if interaction.message.is_empty() {
                if let Some(http) = http {
                    let edit = serenity::EditInteractionResponse::new()
                        .content("メッセージを入力してください。");
                    let _ = http
                        .edit_original_interaction_response(&interaction.interaction_token, &edit, vec![])
                        .await;
                }
                return;
            }

            let author_id: u64 = interaction.author_id.parse().unwrap_or(0);
            let session_id = format!("{}:{}", interaction.channel_id, interaction.author_id);
            let callback_id = format!(
                "discord-{}-{}-{}",
                interaction.channel_id, interaction.author_id, interaction.interaction_id
            );

            // Build a MessageData-like structure for build_callback_payload
            let msg_data = handler::MessageData {
                guild_id: interaction.guild_id.clone(),
                guild_name: interaction.guild_name.clone(),
                channel_id: interaction.channel_id.clone(),
                message_id: interaction.interaction_id.clone(),
                author_id: interaction.author_id.clone(),
                author_name: interaction.author_name.clone(),
                author_bot: false,
                author_roles: interaction.author_roles.clone(),
                content: interaction.message.clone(),
                timestamp: interaction.timestamp.clone(),
                attachments: vec![],
                reference: None,
                thread_info: None,
            };

            let notification_payload = build_callback_payload(
                &callback_id,
                &session_id,
                &msg_data,
                http,
                config,
                bot_user_id,
                author_id,
            )
            .await;

            let enqueue_result = {
                let entry = QueueEntry {
                    callback_id: callback_id.clone(),
                    session_id: session_id.clone(),
                    channel_id: interaction.channel_id.clone(),
                    original_message_id: interaction.interaction_id.clone(),
                    waiting_message_id: None,
                    guild_id: interaction.guild_id.clone(),
                    author_name: interaction.author_name.clone(),
                    author_id: interaction.author_id.clone(),
                    is_reply: false,
                    interaction_token: Some(interaction.interaction_token.clone()),
                    notification_payload,
                    enqueued_at: std::time::Instant::now(),
                };
                message_queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .try_enqueue(entry)
            };

            match enqueue_result {
                EnqueueResult::ProcessNow => {
                    // Register pending callback with interaction mode
                    if let Ok(mut cbs) = pending_callbacks.lock() {
                        cbs.insert(
                            callback_id.clone(),
                            CallbackContext {
                                channel_id: interaction.channel_id.clone(),
                                guild_id: interaction.guild_id.clone(),
                                message_id: interaction.interaction_id.clone(),
                                author_name: interaction.author_name.clone(),
                                response_mode: ResponseMode::Interaction {
                                    token: interaction.interaction_token.clone(),
                                },
                                _typing: None, // Interaction shows "thinking..." automatically
                            },
                        );
                    }

                    let payload = build_callback_payload(
                        &callback_id,
                        &session_id,
                        &msg_data,
                        http,
                        config,
                        bot_user_id,
                        author_id,
                    )
                    .await;
                    let notif = JsonRpcNotification::new(
                        "notifications/mgp.callback.request",
                        Some(payload),
                    );
                    write_message(stdout, &notif);
                }
                EnqueueResult::Queued(position) => {
                    // Edit the deferred response to show queue position
                    if let Some(http) = http {
                        let wait_text = format!(
                            "⏳ 待機中です（{}番目）。順番が来たら応答します。",
                            position
                        );
                        let edit = serenity::EditInteractionResponse::new().content(&wait_text);
                        let _ = http
                            .edit_original_interaction_response(
                                &interaction.interaction_token,
                                &edit,
                                vec![],
                            )
                            .await;
                    }
                    tracing::info!(callback_id = %callback_id, position, "Interaction queued");
                }
                EnqueueResult::Full => {
                    if let Some(http) = http {
                        let edit = serenity::EditInteractionResponse::new()
                            .content("⚠️ 現在キューが満杯です。しばらく待ってからお試しください。");
                        let _ = http
                            .edit_original_interaction_response(
                                &interaction.interaction_token,
                                &edit,
                                vec![],
                            )
                            .await;
                    }
                    tracing::warn!(callback_id = %callback_id, "Queue full — interaction rejected");
                }
            }
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

/// Build the callback notification payload for a Discord message.
///
/// Context is strictly session-scoped: only messages from the current speaker
/// and bot replies to them are included. Messages from other users are excluded
/// entirely, preventing cross-user context contamination.
#[allow(clippy::too_many_arguments)]
async fn build_callback_payload(
    callback_id: &str,
    session_id: &str,
    msg: &handler::MessageData,
    http: &Option<Arc<serenity::Http>>,
    config: &Arc<DiscordConfig>,
    bot_user_id: &Arc<std::sync::atomic::AtomicU64>,
    author_id: u64,
) -> Value {
    let channel_id: u64 = msg.channel_id.parse().unwrap_or(0);
    let msg_id: u64 = msg.message_id.parse().unwrap_or(0);

    // Fetch session-scoped conversation context.
    // Strict filtering: ONLY messages from this session (current user + bot replies to them).
    // Other users' messages are completely excluded.
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
                // Fetch more than needed since we'll filter strictly
                let fetch_limit = (effective_limit as u16 * 3).min(50) as u8;
                let builder = serenity::GetMessages::new()
                    .before(mid)
                    .limit(fetch_limit);
                match cid.messages(http, builder).await {
                    Ok(messages) => {
                        let bot_id = bot_user_id.load(std::sync::atomic::Ordering::Relaxed);
                        let limit = effective_limit as usize;

                        messages
                            .iter()
                            .rev()
                            .filter(|m| !m.content.is_empty())
                            .filter(|m| {
                                let is_speaker = m.author.id.get() == author_id;
                                let is_bot_reply_to_speaker = m.author.id.get() == bot_id
                                    && m.referenced_message
                                        .as_ref()
                                        .is_some_and(|r| r.author.id.get() == author_id);
                                is_speaker || is_bot_reply_to_speaker
                            })
                            .take(limit)
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
                            .collect()
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

    // Build message content with author prefix
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

    // Build metadata
    let mut metadata = json!({
        "source": "discord",
        "session_id": session_id,
        "current_speaker": msg.author_name,
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

    json!({
        "callback_id": callback_id,
        "type": "external_message",
        "message": message_content,
        "metadata": metadata,
    })
}

/// Process the next item in the queue after a callback completes.
async fn process_next_in_queue(
    message_queue: &SharedQueue,
    http: &Option<Arc<serenity::Http>>,
    config: &Arc<DiscordConfig>,
    pending_callbacks: &PendingCallbacks,
    rate_limiter: &Arc<RateLimiter>,
    stdout: &Arc<Mutex<io::Stdout>>,
) {
    let dequeue_result = message_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .complete_active();

    // Update position displays for remaining entries
    if let Some(http) = http {
        for (wait_msg_id, ch_id_str, new_pos) in &dequeue_result.position_updates {
            let ch_id: u64 = ch_id_str.parse().unwrap_or(0);
            let msg_id: u64 = wait_msg_id.parse().unwrap_or(0);
            if ch_id > 0 && msg_id > 0 {
                let new_text = format!(
                    "⏳ 待機中です（{}番目）。順番が来たら応答します。",
                    new_pos
                );
                let edit = serenity::EditMessage::new().content(&new_text);
                rate_limiter
                    .acquire(rate_limiter::Route::ChannelMessage(ch_id))
                    .await;
                let _ = serenity::ChannelId::new(ch_id)
                    .edit_message(http, serenity::MessageId::new(msg_id), edit)
                    .await;
            }
        }
    }

    // Process next entry if available
    if let Some(next) = dequeue_result.next {
        let channel_id: u64 = next.channel_id.parse().unwrap_or(0);
        let original_msg_id: u64 = next.original_message_id.parse().unwrap_or(0);

        // Edit waiting message to "processing" state
        if let (Some(http), Some(wait_id)) = (http, &next.waiting_message_id) {
            let wait_msg_id: u64 = wait_id.parse().unwrap_or(0);
            if channel_id > 0 && wait_msg_id > 0 {
                let edit = serenity::EditMessage::new().content("🔄 応答を生成中...");
                rate_limiter
                    .acquire(rate_limiter::Route::ChannelMessage(channel_id))
                    .await;
                let _ = serenity::ChannelId::new(channel_id)
                    .edit_message(http, serenity::MessageId::new(wait_msg_id), edit)
                    .await;
            }
        }

        // Start typing
        let typing = if let Some(http) = http {
            if channel_id > 0 {
                Some(serenity::ChannelId::new(channel_id).start_typing(http))
            } else {
                None
            }
        } else {
            None
        };

        // Add processing reaction to original message
        if let Some(http) = http {
            if channel_id > 0 && original_msg_id > 0 {
                let emoji = serenity::ReactionType::Unicode(config.reaction_processing.clone());
                let _ = http
                    .create_reaction(
                        serenity::ChannelId::new(channel_id),
                        serenity::MessageId::new(original_msg_id),
                        &emoji,
                    )
                    .await;
                // Remove queue reaction
                let queue_emoji = serenity::ReactionType::Unicode(config.reaction_queued.clone());
                let _ = http
                    .delete_reaction_me(
                        serenity::ChannelId::new(channel_id),
                        serenity::MessageId::new(original_msg_id),
                        &queue_emoji,
                    )
                    .await;
            }
        }

        // Register pending callback
        let response_mode = if let Some(token) = next.interaction_token.clone() {
            ResponseMode::Interaction { token }
        } else {
            ResponseMode::Message {
                is_reply: next.is_reply,
            }
        };
        if let Ok(mut cbs) = pending_callbacks.lock() {
            cbs.insert(
                next.callback_id.clone(),
                CallbackContext {
                    channel_id: next.channel_id.clone(),
                    guild_id: next.guild_id.clone(),
                    message_id: next.original_message_id.clone(),
                    author_name: next.author_name.clone(),
                    response_mode,
                    _typing: typing,
                },
            );
        }

        // Emit callback notification
        let notif = JsonRpcNotification::new(
            "notifications/mgp.callback.request",
            Some(next.notification_payload),
        );
        write_message(stdout, &notif);

        tracing::info!(
            callback_id = %next.callback_id,
            "Dequeued and started processing"
        );
    }
}

/// Handle timed-out queue entries — edit their waiting messages and remove them.
async fn handle_queue_timeouts(
    message_queue: &SharedQueue,
    http: &Option<Arc<serenity::Http>>,
    config: &Arc<DiscordConfig>,
    rate_limiter: &Arc<RateLimiter>,
) {
    let expired = message_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain_expired();

    if expired.is_empty() {
        return;
    }

    let _ = config; // suppress unused warning (may be used for timeout message customization)

    if let Some(http) = http {
        for entry in &expired {
            if let Some(wait_id) = &entry.waiting_message_id {
                let ch_id: u64 = entry.channel_id.parse().unwrap_or(0);
                let wait_msg_id: u64 = wait_id.parse().unwrap_or(0);
                if ch_id > 0 && wait_msg_id > 0 {
                    let timeout_text = "⌛ タイムアウトしました。もう一度お試しください。";
                    let edit = serenity::EditMessage::new().content(timeout_text);
                    rate_limiter
                        .acquire(rate_limiter::Route::ChannelMessage(ch_id))
                        .await;
                    let _ = serenity::ChannelId::new(ch_id)
                        .edit_message(http, serenity::MessageId::new(wait_msg_id), edit)
                        .await;
                }
            }
            tracing::info!(
                callback_id = %entry.callback_id,
                "Queue entry timed out"
            );
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
