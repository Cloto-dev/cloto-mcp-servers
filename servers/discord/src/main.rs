//! MGP Discord Server v0.4.3 -- Bidirectional Discord communication.
//!
//! Runs as a single process using stdio JSON-RPC transport + Discord Gateway.
//! Server ID: `io.discord`
//!
//! Communication flow:
//!   Agent -> tools/call -> Kernel -> stdio -> this server -> Discord API
//!   Discord Gateway -> this server -> notifications/mgp.callback.request -> Kernel -> agentic loop
//!   Kernel -> mgp/callback/respond -> this server -> Discord API (auto-reply)
//!
//! Architecture:
//!   - std::thread reads stdin lines -> mpsc -> main loop
//!   - Serenity Gateway runs in tokio task -> mpsc -> main loop
//!   - Main loop: tokio::select! dispatches both, writes to stdout via Mutex

mod bridge;
mod callback;
mod config;
mod handler;
mod interaction;
mod protocol;
mod queue;
mod rate_limiter;
mod streaming;
mod tools;
mod utils;

use bridge::{
    write_message, BotContext, BridgeContext, BridgeStats, CallbackContext, ResponseMode,
};
use config::DiscordConfig;
use handler::{DiscordEvent, DiscordHandler};
use interaction::build_callback_payload;
use protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use queue::QueueEntry;
use serde_json::{json, Value};
use serenity::all as serenity;
use std::collections::HashMap;
use std::io::{self, BufRead};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

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

    tracing::info!("MGP Discord Server v0.4.3 starting");

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
        tracing::error!("DISCORD_BOT_TOKEN not set -- Discord Gateway will not connect");
    }

    // Shared stdout writer
    let stdout = Arc::new(Mutex::new(io::stdout()));

    // Pending callbacks: maps callback_id -> CallbackContext for response routing
    let pending_callbacks: bridge::PendingCallbacks = Arc::new(Mutex::new(HashMap::new()));

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
    let rate_limiter = Arc::new(rate_limiter::RateLimiter::new());
    let message_queue = Arc::new(Mutex::new(queue::MessageQueue::with_config(
        config.queue_max_size,
        config.queue_timeout_secs,
    )));
    let streaming_states: bridge::StreamingStates =
        Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    // Build the shared BridgeContext
    let chunk_tracker = Arc::new(Mutex::new(bridge::ChunkTracker::new(
        config.chunk_gap_minutes,
    )));
    let ctx = BridgeContext {
        http,
        config,
        rate_limiter,
        pending_callbacks,
        bot_context,
        bridge_stats,
        message_queue,
        streaming_states,
        stdout,
        bot_user_id,
        processed_callbacks: Arc::new(Mutex::new(bridge::ProcessedCallbacks::new(200))),
        chunk_tracker,
    };

    // Periodic intervals
    let mut cleanup_interval = tokio::time::interval(std::time::Duration::from_secs(300));
    cleanup_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut queue_timeout_interval = tokio::time::interval(std::time::Duration::from_secs(15));
    queue_timeout_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut stream_flush_interval = tokio::time::interval(std::time::Duration::from_millis(500));
    stream_flush_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Main loop: multiplex stdin JSON-RPC and Discord Gateway events
    loop {
        tokio::select! {
            Some(line) = stdin_rx.recv() => {
                handle_stdin_line(&ctx, &line).await;
            }
            Some(event) = discord_rx.recv() => {
                handle_discord_event(&ctx, event).await;
            }
            _ = stream_flush_interval.tick() => {
                streaming::flush_streaming_edits(&ctx).await;
            }
            _ = cleanup_interval.tick() => {
                ctx.rate_limiter.cleanup().await;
            }
            _ = queue_timeout_interval.tick() => {
                callback::handle_queue_timeouts(&ctx).await;
            }
            else => break,
        }
    }

    tracing::info!("MGP Discord Server shutting down");
}

async fn handle_stdin_line(ctx: &BridgeContext, line: &str) {
    let request: JsonRpcRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Invalid JSON-RPC: {e}");
            write_message(
                &ctx.stdout,
                &JsonRpcResponse::err(None, -32700, "Parse error"),
            );
            return;
        }
    };

    // Notifications (no id)
    if request.id.is_none() {
        // Handle streaming chunks
        if request.method == "notifications/mgp.stream.chunk" {
            streaming::handle_stream_chunk(ctx, &request).await;
        } else {
            tracing::debug!("Received notification: {}", request.method);
        }
        return;
    }

    let (response, notifications) = dispatch(ctx, &request).await;

    // Emit notifications BEFORE the response (avatar pattern)
    for notif in &notifications {
        write_message(&ctx.stdout, notif);
    }
    write_message(&ctx.stdout, &response);
}

async fn dispatch(
    ctx: &BridgeContext,
    request: &JsonRpcRequest,
) -> (JsonRpcResponse, Vec<JsonRpcNotification>) {
    match request.method.as_str() {
        "initialize" => (handle_initialize(request), vec![]),
        "tools/list" => (handle_tools_list(request), vec![]),
        "tools/call" => handle_tools_call(ctx, request).await,
        "mgp/callback/respond" => (
            callback::handle_callback_respond(ctx, request).await,
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
                "tools": {},
                "mgp": {
                    "version": "0.6.0",
                    "extensions": ["permissions"],
                    "permissions_required": ["network.outbound"]
                }
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
    ctx: &BridgeContext,
    request: &JsonRpcRequest,
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
        let uptime = ctx.bridge_stats.start_time.elapsed().as_secs();
        let connected_secs = ctx
            .bridge_stats
            .connected_since
            .lock()
            .ok()
            .and_then(|g| g.map(|t| t.elapsed().as_secs()));
        let last_event_secs = ctx
            .bridge_stats
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
                    "messages_received": ctx.bridge_stats.messages_received.load(std::sync::atomic::Ordering::Relaxed),
                    "messages_sent": ctx.bridge_stats.messages_sent.load(std::sync::atomic::Ordering::Relaxed),
                    "errors": ctx.bridge_stats.errors.load(std::sync::atomic::Ordering::Relaxed),
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
        let bot_ctx_ref = ctx.bot_context.lock().ok().and_then(|g| g.clone());
        return match tools::execute(
            tool_name,
            &args,
            &Arc::new(serenity::Http::new("")),
            &ctx.config,
            bot_ctx_ref.as_ref(),
            &ctx.rate_limiter,
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

    let Some(http) = &ctx.http else {
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

    let bot_ctx_ref = ctx.bot_context.lock().ok().and_then(|g| g.clone());
    match tools::execute(
        tool_name,
        &args,
        http,
        &ctx.config,
        bot_ctx_ref.as_ref(),
        &ctx.rate_limiter,
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
    }
}

async fn handle_discord_event(ctx: &BridgeContext, event: DiscordEvent) {
    // Update last event timestamp
    if let Ok(mut g) = ctx.bridge_stats.last_event_at.lock() {
        *g = Some(std::time::Instant::now());
    }

    match event {
        DiscordEvent::MessageCreate(msg) => {
            ctx.bridge_stats
                .messages_received
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            let channel_id: u64 = msg.channel_id.parse().unwrap_or(0);
            let msg_id: u64 = msg.message_id.parse().unwrap_or(0);

            // -- Direct tool execution (backtick commands) --
            let author_id: u64 = msg.author_id.parse().unwrap_or(0);
            if ctx.config.direct_tool_users.contains(&author_id) {
                let all_known: Vec<&str> = tools::BRIDGE_TOOL_NAMES
                    .iter()
                    .copied()
                    .chain(ctx.config.direct_tool_ecosystem.iter().map(|s| s.as_str()))
                    .collect();

                if let Some((tool_name, args_map)) =
                    utils::parse_direct_command(&msg.content, &all_known)
                {
                    // Add processing reaction
                    if let Some(http) = &ctx.http {
                        if channel_id > 0 && msg_id > 0 {
                            let emoji = serenity::ReactionType::Unicode(
                                ctx.config.reaction_processing.clone(),
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

                    if tools::BRIDGE_TOOL_NAMES.contains(&tool_name.as_str()) {
                        // F1: Local execution for bridge-native tools
                        if let Some(http) = &ctx.http {
                            let args_json: Value = args_map
                                .iter()
                                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                                .collect::<serde_json::Map<String, Value>>()
                                .into();

                            let bot_ctx_ref = ctx.bot_context.lock().ok().and_then(|g| g.clone());
                            let (reaction, result_text) = match tools::execute(
                                &tool_name,
                                &args_json,
                                http,
                                &ctx.config,
                                bot_ctx_ref.as_ref(),
                                &ctx.rate_limiter,
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
                                    (ctx.config.reaction_done.clone(), text)
                                }
                                Err(e) => {
                                    ctx.bridge_stats
                                        .errors
                                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    (ctx.config.reaction_error.clone(), format!("Error: {e}"))
                                }
                            };

                            // Send result as reply
                            let send_args = json!({
                                "channel_id": msg.channel_id,
                                "content": result_text,
                                "reply_to": msg.message_id,
                            });
                            let bot_ctx_ref2 = ctx.bot_context.lock().ok().and_then(|g| g.clone());
                            let _ = tools::execute(
                                "send_message",
                                &send_args,
                                http,
                                &ctx.config,
                                bot_ctx_ref2.as_ref(),
                                &ctx.rate_limiter,
                            )
                            .await;
                            ctx.bridge_stats
                                .messages_sent
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                            // Swap processing -> done/error reaction
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
                                    ctx.config.reaction_processing.clone(),
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
                    // F2: Ecosystem tool -- fall through with tool_hint metadata
                    // (handled below by injecting tool_hint into callback metadata)
                }
            }

            // -- Normal callback flow (with queue) --
            // Session scoping:
            //   Thread -> shared session (all participants share context)
            //   Regular channel -> per-user session
            let is_thread = msg.thread_info.is_some();
            let session_id = if is_thread {
                format!("{}:shared", msg.channel_id)
            } else {
                ctx.chunk_tracker
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_session_id(
                        msg.channel_id.parse().unwrap_or(0),
                        msg.author_id.parse().unwrap_or(0),
                    )
            };
            let callback_id = format!(
                "discord-{}-{}-{}",
                msg.channel_id, msg.author_id, msg.message_id
            );

            // Check if queue is busy before building payload (#6)
            let is_busy = ctx
                .message_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_busy();

            if !is_busy {
                // Idle -- process immediately without creating a QueueEntry
                ctx.message_queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .set_active(callback_id.clone());

                let typing = if let Some(http) = &ctx.http {
                    if channel_id > 0 {
                        Some(serenity::ChannelId::new(channel_id).start_typing(http))
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Add processing reaction
                if let Some(http) = &ctx.http {
                    if channel_id > 0 && msg_id > 0 {
                        let emoji =
                            serenity::ReactionType::Unicode(ctx.config.reaction_processing.clone());
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
                if let Ok(mut cbs) = ctx.pending_callbacks.lock() {
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

                // Build payload once and emit
                let payload =
                    build_callback_payload(ctx, &callback_id, &session_id, &msg, author_id).await;
                let notif =
                    JsonRpcNotification::new("notifications/mgp.callback.request", Some(payload));
                write_message(&ctx.stdout, &notif);
            } else {
                // Busy -- build payload and enqueue
                let notification_payload =
                    build_callback_payload(ctx, &callback_id, &session_id, &msg, author_id).await;

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
                    ctx.message_queue
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .try_enqueue(entry)
                };

                match enqueue_result {
                    queue::EnqueueResult::ProcessNow => {
                        // Shouldn't happen since we checked is_busy, but handle gracefully
                        let typing = if let Some(http) = &ctx.http {
                            if channel_id > 0 {
                                Some(serenity::ChannelId::new(channel_id).start_typing(http))
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        if let Some(http) = &ctx.http {
                            if channel_id > 0 && msg_id > 0 {
                                let emoji = serenity::ReactionType::Unicode(
                                    ctx.config.reaction_processing.clone(),
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

                        if let Ok(mut cbs) = ctx.pending_callbacks.lock() {
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

                        let payload =
                            build_callback_payload(ctx, &callback_id, &session_id, &msg, author_id)
                                .await;
                        let notif = JsonRpcNotification::new(
                            "notifications/mgp.callback.request",
                            Some(payload),
                        );
                        write_message(&ctx.stdout, &notif);
                    }
                    queue::EnqueueResult::Queued(position) => {
                        // Busy -- send waiting message and store in queue
                        if let Some(http) = &ctx.http {
                            // Add queue reaction to original message
                            if channel_id > 0 && msg_id > 0 {
                                let emoji = serenity::ReactionType::Unicode(
                                    ctx.config.reaction_queued.clone(),
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
                            let mut msg_builder =
                                serenity::CreateMessage::new().content(&wait_text);
                            if msg.reference.is_some() {
                                msg_builder = msg_builder.reference_message(
                                    serenity::MessageReference::from((
                                        cid,
                                        serenity::MessageId::new(msg_id),
                                    )),
                                );
                            }
                            ctx.rate_limiter
                                .acquire(rate_limiter::Route::ChannelMessage(channel_id))
                                .await;
                            if let Ok(sent) = cid.send_message(http, msg_builder).await {
                                // Update the queue entry with the waiting message ID
                                if let Ok(mut q) = ctx.message_queue.lock() {
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
                    queue::EnqueueResult::Full => {
                        // Queue full -- notify user
                        if let Some(http) = &ctx.http {
                            let full_text =
                                "⚠️ 現在キューが満杯です。しばらく待ってからお試しください。";
                            let cid = serenity::ChannelId::new(channel_id);
                            let msg_builder = serenity::CreateMessage::new().content(full_text);
                            ctx.rate_limiter
                                .acquire(rate_limiter::Route::ChannelMessage(channel_id))
                                .await;
                            let _ = cid.send_message(http, msg_builder).await;
                        }
                        tracing::warn!(
                            callback_id = %callback_id,
                            "Queue full -- message rejected"
                        );
                    }
                }
            }
        }
        DiscordEvent::InteractionCreate(interaction) => {
            interaction::handle_interaction_create(ctx, interaction).await;
        }
        DiscordEvent::ComponentInteraction(comp) => {
            interaction::handle_component_interaction(ctx, comp).await;
        }
        DiscordEvent::Ready(data) => {
            ctx.bot_user_id
                .store(data.bot_user_id, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut g) = ctx.bridge_stats.connected_since.lock() {
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
            write_message(&ctx.stdout, &notif);
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
            write_message(&ctx.stdout, &notif);
        }
        DiscordEvent::ShardStageUpdate { old, new } => {
            tracing::info!("Shard stage: {old} -> {new}");
            let notif = JsonRpcNotification::new(
                "notifications/mgp.lifecycle",
                Some(json!({
                    "server_id": "io.discord",
                    "previous_state": old,
                    "new_state": new,
                    "reason": "Shard stage update",
                })),
            );
            write_message(&ctx.stdout, &notif);
        }
    }
}
