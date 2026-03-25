//! MGP Discord Server — Bidirectional Discord communication.
//!
//! Runs as a single process using stdio JSON-RPC transport + Discord Gateway.
//! Server ID: `io.discord`
//!
//! Communication flow:
//!   Agent → tools/call → Kernel → stdio → this server → Discord API
//!   Discord Gateway → this server → notifications/mgp.event → Kernel → SSE → Dashboard
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
use std::io::{self, BufRead, Write};
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

    tracing::info!("MGP Discord Server starting (stdio + Gateway transport)");

    let config = DiscordConfig::from_env();

    if config.bot_token.is_empty() {
        tracing::error!("DISCORD_BOT_TOKEN not set — Discord Gateway will not connect");
    }

    // Shared stdout writer (Mutex for thread safety between stdin dispatch and Discord events)
    let stdout = Arc::new(Mutex::new(io::stdout()));

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
                handle_stdin_line(&line, &stdout, &http, &config).await;
            }
            Some(event) = discord_rx.recv() => {
                handle_discord_event(event, &stdout);
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

    let (response, notifications) = dispatch(&request, http, config).await;

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
) -> (JsonRpcResponse, Vec<JsonRpcNotification>) {
    match request.method.as_str() {
        "initialize" => (handle_initialize(request), vec![]),
        "tools/list" => (handle_tools_list(request), vec![]),
        "tools/call" => handle_tools_call(request, http, config).await,
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

fn handle_discord_event(event: DiscordEvent, stdout: &Arc<Mutex<io::Stdout>>) {
    match event {
        DiscordEvent::MessageCreate(msg) => {
            let notif = JsonRpcNotification::new(
                "notifications/mgp.event",
                Some(json!({
                    "channel": "discord.message_received",
                    "data": {
                        "guild_id": msg.guild_id,
                        "channel_id": msg.channel_id,
                        "message_id": msg.message_id,
                        "author": {
                            "id": msg.author_id,
                            "name": msg.author_name,
                            "bot": msg.author_bot,
                        },
                        "content": msg.content,
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
