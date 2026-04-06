//! Discord interaction handlers (slash commands and component interactions).
//!
//! Handles `/chat`, `/status` slash commands and button/select menu interactions,
//! routing them through the message queue and callback system.

use crate::bridge::{
    ephemeral_response, write_message, BridgeContext, CallbackContext, ResponseMode,
};
use crate::handler::{ComponentData, InteractionData, MessageData, ReferenceData};
use crate::protocol::JsonRpcNotification;
use crate::queue::QueueEntry;
use crate::utils;
use serde_json::{json, Value};
use serenity::all as serenity;

/// Handle a slash command interaction (/chat, /status).
pub async fn handle_interaction_create(ctx: &BridgeContext, interaction: Box<InteractionData>) {
    ctx.bridge_stats
        .messages_received
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // Handle /status locally
    if interaction.command_name == "status" {
        let uptime = ctx.bridge_stats.start_time.elapsed().as_secs();
        let queue_info = ctx
            .message_queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status();
        let status_text = format!(
            "**Bridge Status**\nUptime: {}s\nMessages: {} recv / {} sent\nErrors: {}\nQueue: {}/{} (active: {})",
            uptime,
            ctx.bridge_stats.messages_received.load(std::sync::atomic::Ordering::Relaxed),
            ctx.bridge_stats.messages_sent.load(std::sync::atomic::Ordering::Relaxed),
            ctx.bridge_stats.errors.load(std::sync::atomic::Ordering::Relaxed),
            queue_info.waiting,
            queue_info.max_size,
            queue_info.active,
        );
        if let Some(http) = &ctx.http {
            let edit = serenity::EditInteractionResponse::new().content(&status_text);
            let _ = http
                .edit_original_interaction_response(&interaction.interaction_token, &edit, vec![])
                .await;
        }
        return;
    }

    // /chat -- route through callback flow
    if interaction.message.is_empty() {
        if let Some(http) = &ctx.http {
            let edit =
                serenity::EditInteractionResponse::new().content("メッセージを入力してください。");
            let _ = http
                .edit_original_interaction_response(&interaction.interaction_token, &edit, vec![])
                .await;
        }
        return;
    }

    let author_id: u64 = interaction.author_id.parse().unwrap_or(0);
    let channel_id: u64 = interaction.channel_id.parse().unwrap_or(0);
    let session_id = ctx
        .chunk_tracker
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_session_id(channel_id, author_id);
    let callback_id = format!(
        "discord-{}-{}-{}",
        interaction.channel_id, interaction.author_id, interaction.interaction_id
    );

    // Build a MessageData-like structure for build_callback_payload
    let msg_data = MessageData {
        guild_id: interaction.guild_id.clone(),
        guild_name: interaction.guild_name.clone(),
        channel_id: interaction.channel_id.clone(),
        message_id: interaction.interaction_id.clone(),
        author_id: interaction.author_id.clone(),
        author_name: interaction.author_name.clone(),
        author_bot: false,
        is_webhook: false,
        webhook_id: None,
        author_roles: interaction.author_roles.clone(),
        content: interaction.message.clone(),
        timestamp: interaction.timestamp.clone(),
        attachments: vec![],
        reference: None,
        thread_info: None,
    };

    // Check if queue is busy before building payload
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

        // Register pending callback with interaction mode
        if let Ok(mut cbs) = ctx.pending_callbacks.lock() {
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

        // Build payload once and emit
        let payload =
            build_callback_payload(ctx, &callback_id, &session_id, &msg_data, author_id).await;
        let notif = JsonRpcNotification::new("notifications/mgp.callback.request", Some(payload));
        write_message(&ctx.stdout, &notif);
    } else {
        // Busy -- build payload and enqueue
        let notification_payload =
            build_callback_payload(ctx, &callback_id, &session_id, &msg_data, author_id).await;

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
            ctx.message_queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .try_enqueue(entry)
        };

        match enqueue_result {
            crate::queue::EnqueueResult::ProcessNow => {
                // This shouldn't happen since we checked is_busy, but handle gracefully
                if let Ok(mut cbs) = ctx.pending_callbacks.lock() {
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
                            _typing: None,
                        },
                    );
                }
                let payload =
                    build_callback_payload(ctx, &callback_id, &session_id, &msg_data, author_id)
                        .await;
                let notif =
                    JsonRpcNotification::new("notifications/mgp.callback.request", Some(payload));
                write_message(&ctx.stdout, &notif);
            }
            crate::queue::EnqueueResult::Queued(position) => {
                // Edit the deferred response to show queue position
                if let Some(http) = &ctx.http {
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
            crate::queue::EnqueueResult::Full => {
                if let Some(http) = &ctx.http {
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
                tracing::warn!(callback_id = %callback_id, "Queue full -- interaction rejected");
            }
        }
    }
}

/// Handle a component interaction (button click, select menu).
pub async fn handle_component_interaction(ctx: &BridgeContext, comp: Box<ComponentData>) {
    let iid = serenity::InteractionId::new(comp.interaction_id);

    // Verify authorization: only DISCORD_DIRECT_TOOL_USERS can press action buttons
    if !ctx.config.direct_tool_users.contains(&comp.user_id) {
        // Ephemeral rejection (only the clicker sees this)
        if let Some(http) = &ctx.http {
            let resp = ephemeral_response("🔒 この操作を行う権限がありません。");
            let _ = http
                .create_interaction_response(iid, &comp.interaction_token, &resp, vec![])
                .await;
        }
        return;
    }

    // Acknowledge the interaction immediately
    if let Some(http) = &ctx.http {
        let resp = ephemeral_response(&format!("✅ {} が操作しました。", comp.user_name));
        let _ = http
            .create_interaction_response(iid, &comp.interaction_token, &resp, vec![])
            .await;
    }

    // Emit as MGP notification for kernel processing
    let notif = JsonRpcNotification::new(
        "notifications/mgp.callback.request",
        Some(json!({
            "callback_id": format!("discord-component-{}", comp.custom_id),
            "type": "component_interaction",
            "message": comp.custom_id,
            "metadata": {
                "source": "discord",
                "interaction_type": "component",
                "custom_id": comp.custom_id,
                "values": comp.values,
                "user_id": comp.user_id.to_string(),
                "user_name": comp.user_name,
                "channel_id": comp.channel_id,
            },
        })),
    );
    write_message(&ctx.stdout, &notif);

    tracing::info!(
        custom_id = %comp.custom_id,
        user = %comp.user_name,
        "Component interaction processed"
    );
}

/// Build the callback notification payload for a Discord message.
///
/// Context scoping:
///   - Thread sessions (session_id ends with ":shared"): all messages included
///   - User sessions: only speaker's messages + bot replies to them
///
/// Reply chain: when the message is a reply, traverses the reference chain
/// (up to 3 hops) to provide conversation lineage context.
pub async fn build_callback_payload(
    ctx: &BridgeContext,
    callback_id: &str,
    session_id: &str,
    msg: &MessageData,
    author_id: u64,
) -> Value {
    let channel_id: u64 = msg.channel_id.parse().unwrap_or(0);
    let msg_id: u64 = msg.message_id.parse().unwrap_or(0);
    let is_shared_session = session_id.ends_with(":shared");

    // Fetch conversation context (mode depends on session type)
    let effective_limit = if msg.content.len() < 20 {
        ctx.config.context_history_limit.min(5)
    } else {
        ctx.config.context_history_limit
    };
    let conversation_context = if effective_limit > 0 {
        if let Some(http) = &ctx.http {
            if channel_id > 0 && msg_id > 0 {
                let cid = serenity::ChannelId::new(channel_id);
                let mid = serenity::MessageId::new(msg_id);
                let fetch_limit = if is_shared_session {
                    effective_limit // Threads: all messages are relevant
                } else {
                    (effective_limit as u16 * 3).min(50) as u8 // Over-fetch for strict filter
                };
                let builder = serenity::GetMessages::new().before(mid).limit(fetch_limit);
                match cid.messages(http, builder).await {
                    Ok(messages) => {
                        let bot_id = ctx.bot_user_id.load(std::sync::atomic::Ordering::Relaxed);
                        let limit = effective_limit as usize;

                        messages
                            .iter()
                            .rev()
                            .filter(|m| !m.content.is_empty())
                            .filter(|m| {
                                if is_shared_session {
                                    // Thread: include all messages
                                    true
                                } else {
                                    // Per-user: strict session filter
                                    let is_speaker = m.author.id.get() == author_id;
                                    let is_bot_reply_to_speaker = m.author.id.get() == bot_id
                                        && m.referenced_message
                                            .as_ref()
                                            .is_some_and(|r| r.author.id.get() == author_id);
                                    is_speaker || is_bot_reply_to_speaker
                                }
                            })
                            .take(limit)
                            .map(|m| {
                                if m.author.id.get() == bot_id {
                                    json!({
                                        "role": "assistant",
                                        "content": utils::truncate_str(&m.content, 500),
                                        "timestamp": m.timestamp.to_string(),
                                    })
                                } else {
                                    json!({
                                        "role": "user",
                                        "name": m.author.name,
                                        "user_id": m.author.id.get().to_string(),
                                        "content": utils::truncate_str(&m.content, 500),
                                        "timestamp": m.timestamp.to_string(),
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

    // Build reply chain: traverse referenced messages to provide conversation lineage.
    // This allows User B replying to Bot's reply to User A to see the full thread.
    let reference_chain = if let Some(ref reference) = msg.reference {
        build_reply_chain(ctx, channel_id, reference).await
    } else {
        vec![]
    };

    // Build message content with author prefix (strip Discord mentions before storage)
    let clean_content = utils::strip_all_mentions(&msg.content);
    let mut message_content = format!("[{}] {}", msg.author_name, clean_content);
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
        "is_webhook": msg.is_webhook,
        "webhook_id": msg.webhook_id,
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
        "reference_chain": reference_chain,
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

    let callback_type = if msg.is_webhook {
        "webhook_message"
    } else {
        "external_message"
    };

    json!({
        "callback_id": callback_id,
        "type": callback_type,
        "message": message_content,
        "metadata": metadata,
    })
}

/// Traverse the reply chain starting from a reference, fetching up to 3 hops.
/// Returns a list of messages from oldest (root) to newest (direct parent).
pub async fn build_reply_chain(
    ctx: &BridgeContext,
    channel_id: u64,
    initial_ref: &ReferenceData,
) -> Vec<Value> {
    const MAX_HOPS: usize = 3;
    let Some(http) = &ctx.http else {
        return vec![];
    };
    if channel_id == 0 {
        return vec![];
    }

    let bot_id = ctx.bot_user_id.load(std::sync::atomic::Ordering::Relaxed);
    let cid = serenity::ChannelId::new(channel_id);
    let mut chain = Vec::new();

    // First entry from the reference we already have
    let role = if initial_ref.author_id.parse::<u64>().unwrap_or(0) == bot_id {
        "assistant"
    } else {
        "user"
    };
    chain.push(json!({
        "role": role,
        "name": initial_ref.author_name,
        "content": utils::truncate_str(&initial_ref.content, 500),
    }));

    // Fetch further up the chain via Discord API
    let mut cursor_id: u64 = initial_ref.message_id.parse().unwrap_or(0);
    for _ in 1..MAX_HOPS {
        if cursor_id == 0 {
            break;
        }
        let mid = serenity::MessageId::new(cursor_id);
        match cid.message(http, mid).await {
            Ok(fetched) => {
                let Some(ref parent) = fetched.referenced_message else {
                    break; // No further parent
                };
                let parent_role = if parent.author.id.get() == bot_id {
                    "assistant"
                } else {
                    "user"
                };
                chain.push(json!({
                    "role": parent_role,
                    "name": parent.author.name,
                    "content": utils::truncate_str(&parent.content, 500),
                }));
                cursor_id = parent.id.get();
            }
            Err(e) => {
                tracing::debug!("Reply chain fetch stopped: {e}");
                break;
            }
        }
    }

    // Reverse so oldest (root) is first
    chain.reverse();
    chain
}
