//! MGP Discord Server — Bidirectional Discord communication.
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
    #[allow(dead_code)]
    message_id: String,
    #[allow(dead_code)]
    author_name: String,
    /// Typing indicator guard — dropping this stops the typing indicator.
    /// Serenity's `Typing` holds a `oneshot::Sender`; when dropped, the internal
    /// refresh task sees `Closed` and exits.
    _typing: Option<serenity::http::Typing>,
}

type PendingCallbacks = Arc<Mutex<HashMap<String, CallbackContext>>>;

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

    tracing::info!("MGP Discord Server starting (stdio + Gateway transport)");

    let config = DiscordConfig::from_env();

    if config.bot_token.is_empty() {
        tracing::error!("DISCORD_BOT_TOKEN not set — Discord Gateway will not connect");
    }

    // Shared stdout writer (Mutex for thread safety between stdin dispatch and Discord events)
    let stdout = Arc::new(Mutex::new(io::stdout()));

    // Pending callbacks: maps callback_id → CallbackContext for response routing
    let pending_callbacks: PendingCallbacks = Arc::new(Mutex::new(HashMap::new()));

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
        };

        match serenity::Client::builder(&config.bot_token, intents)
            .event_handler(handler)
            .await
        {
            Ok(mut client) => {
                let http_arc = client.http.clone();
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
                handle_stdin_line(&line, &stdout, &http, &config, &pending_callbacks).await;
            }
            Some(event) = discord_rx.recv() => {
                handle_discord_event(event, &stdout, &pending_callbacks, &http).await;
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

    let (response, notifications) = dispatch(&request, http, config, pending_callbacks).await;

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
) -> (JsonRpcResponse, Vec<JsonRpcNotification>) {
    match request.method.as_str() {
        "initialize" => (handle_initialize(request), vec![]),
        "tools/list" => (handle_tools_list(request), vec![]),
        "tools/call" => handle_tools_call(request, http, config).await,
        "mgp/callback/respond" => {
            (handle_callback_respond(request, http, config, pending_callbacks).await, vec![])
        }
        "notifications/initialized" => {
            // MCP initialized notification — no response needed but we already
            // filtered notifications above. This is a safety fallback.
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

    match tools::execute(tool_name, &args, http, config).await {
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
        return JsonRpcResponse::err(
            request.id.clone(),
            -32602,
            "callback_id is required",
        );
    }

    // Always remove the callback context first to stop the typing indicator,
    // regardless of whether the response is empty or sending fails.
    let ctx = pending_callbacks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(callback_id);

    let Some(ctx) = ctx else {
        tracing::warn!(callback_id = %callback_id, "No pending callback found — may have expired");
        return JsonRpcResponse::ok(
            request.id.clone(),
            json!({"status": "not_found", "callback_id": callback_id}),
        );
    };

    // Typing indicator stops automatically when ctx (and its _typing guard) is dropped.

    let response = params
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Empty response: clean up typing but skip sending a message
    if response.is_empty() {
        tracing::info!(callback_id = %callback_id, "Empty response — typing stopped, no message sent");
        return JsonRpcResponse::ok(
            request.id.clone(),
            json!({"status": "empty", "callback_id": callback_id, "channel_id": ctx.channel_id}),
        );
    }

    // Send the response to the original Discord channel
    let Some(http) = http else {
        return JsonRpcResponse::err(request.id.clone(), -32000, "Discord not connected");
    };

    let send_args = json!({
        "channel_id": ctx.channel_id,
        "content": response,
    });

    match tools::execute("send_message", &send_args, http, config).await {
        Ok(_) => {
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
            tracing::error!(
                callback_id = %callback_id,
                error = %e,
                "Failed to send callback response to Discord"
            );
            JsonRpcResponse::err(request.id.clone(), -32000, format!("Discord send failed: {e}"))
        }
    }
}

async fn handle_discord_event(
    event: DiscordEvent,
    stdout: &Arc<Mutex<io::Stdout>>,
    pending_callbacks: &PendingCallbacks,
    http: &Option<Arc<serenity::Http>>,
) {
    match event {
        DiscordEvent::MessageCreate(msg) => {
            let callback_id = format!("discord-{}", msg.message_id);

            // Start typing indicator (auto-refreshes until Typing guard is dropped)
            let typing = if let Some(http) = http {
                let channel_id: u64 = msg.channel_id.parse().unwrap_or(0);
                if channel_id > 0 {
                    let cid = serenity::ChannelId::new(channel_id);
                    Some(cid.start_typing(http))
                } else {
                    None
                }
            } else {
                None
            };

            // Register pending callback context for response routing
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

            // Emit callback request (MGP §13) — kernel will process and respond
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
                    }
                })),
            );
            write_message(stdout, &notif);
        }
        DiscordEvent::Ready(data) => {
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
