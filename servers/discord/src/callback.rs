//! Callback response handling and queue management.
//!
//! Handles `mgp/callback/respond` from the kernel -- routes the response
//! back to Discord (as a channel message or interaction edit), manages
//! streaming finalization, and drives the message queue forward.

use crate::bridge::{write_message, BridgeContext, ResponseMode};
use crate::protocol::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use crate::rate_limiter;
use crate::tools;
use serde_json::json;
use serenity::all as serenity;

/// Handle mgp/callback/respond from the kernel -- auto-send response to Discord.
pub async fn handle_callback_respond(
    ctx: &BridgeContext,
    request: &JsonRpcRequest,
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
    let cb_ctx = ctx
        .pending_callbacks
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(callback_id);

    let Some(mut cb_ctx) = cb_ctx else {
        tracing::warn!(callback_id = %callback_id, "No pending callback found");
        return JsonRpcResponse::ok(
            request.id.clone(),
            json!({"status": "not_found", "callback_id": callback_id}),
        );
    };

    // Extract values and immediately drop cb_ctx to stop typing indicator.
    let ctx_channel_id = cb_ctx.channel_id.clone();
    let ctx_message_id = cb_ctx.message_id.clone();
    // Take response_mode out by replacing with a dummy, then drop cb_ctx to stop typing
    let ctx_response_mode = std::mem::replace(
        &mut cb_ctx.response_mode,
        ResponseMode::Message { is_reply: false },
    );
    drop(cb_ctx); // _typing dropped here -> typing stops immediately

    let response = params
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let channel_id: u64 = ctx_channel_id.parse().unwrap_or(0);
    let message_id: u64 = ctx_message_id.parse().unwrap_or(0);

    // Empty response: typing already stopped above
    if response.is_empty() {
        tracing::info!(callback_id = %callback_id, "Empty response -- typing stopped");
        return JsonRpcResponse::ok(
            request.id.clone(),
            json!({"status": "empty", "callback_id": callback_id}),
        );
    }

    // Check if there's an active streaming state for this callback.
    // If so, finalize by editing the existing message instead of sending a new one.
    let stream_state = ctx.streaming_states.lock().await.remove(callback_id);

    let Some(http) = &ctx.http else {
        return JsonRpcResponse::err(request.id.clone(), -32000, "Discord not connected");
    };

    // If streaming was active, edit the stream message with final content
    if let Some(ss) = &stream_state {
        if let Some(ref token) = ss.interaction_token {
            let edit = serenity::EditInteractionResponse::new().content(response);
            let _ = http
                .edit_original_interaction_response(token, &edit, vec![])
                .await;
        } else if ss.channel_id > 0 && ss.message_id > 0 {
            ctx.rate_limiter
                .acquire(rate_limiter::Route::ChannelMessage(ss.channel_id))
                .await;
            let edit = serenity::EditMessage::new().content(response);
            let _ = serenity::ChannelId::new(ss.channel_id)
                .edit_message(http, serenity::MessageId::new(ss.message_id), edit)
                .await;
        }
        ctx.bridge_stats
            .messages_sent
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Handle reactions for the original message (Message mode only)
        if let ResponseMode::Message { .. } = &ctx_response_mode {
            if channel_id > 0 && message_id > 0 {
                let emoji = serenity::ReactionType::Unicode(ctx.config.reaction_done.clone());
                let _ = http
                    .create_reaction(
                        serenity::ChannelId::new(channel_id),
                        serenity::MessageId::new(message_id),
                        &emoji,
                    )
                    .await;
                let processing_emoji =
                    serenity::ReactionType::Unicode(ctx.config.reaction_processing.clone());
                let _ = http
                    .delete_reaction_me(
                        serenity::ChannelId::new(channel_id),
                        serenity::MessageId::new(message_id),
                        &processing_emoji,
                    )
                    .await;
            }
        }

        tracing::info!(callback_id = %callback_id, "Streaming response finalized");
        let result = JsonRpcResponse::ok(
            request.id.clone(),
            json!({"status": "sent", "callback_id": callback_id, "mode": "stream_finalized"}),
        );
        process_next_in_queue(ctx).await;
        return result;
    }

    let result = match ctx_response_mode {
        ResponseMode::Interaction { token } => {
            // Edit the deferred interaction response
            let edit = serenity::EditInteractionResponse::new().content(response);
            match http
                .edit_original_interaction_response(&token, &edit, vec![])
                .await
            {
                Ok(_) => {
                    ctx.bridge_stats
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
                    ctx.bridge_stats
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

            let bot_ctx_ref = ctx.bot_context.lock().ok().and_then(|g| g.clone());
            match tools::execute(
                "send_message",
                &send_args,
                http,
                &ctx.config,
                bot_ctx_ref.as_ref(),
                &ctx.rate_limiter,
            )
            .await
            {
                Ok(_) => {
                    ctx.bridge_stats
                        .messages_sent
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if channel_id > 0 && message_id > 0 {
                        let emoji =
                            serenity::ReactionType::Unicode(ctx.config.reaction_done.clone());
                        let _ = http
                            .create_reaction(
                                serenity::ChannelId::new(channel_id),
                                serenity::MessageId::new(message_id),
                                &emoji,
                            )
                            .await;
                        let processing_emoji =
                            serenity::ReactionType::Unicode(ctx.config.reaction_processing.clone());
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
                    ctx.bridge_stats
                        .errors
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if channel_id > 0 && message_id > 0 {
                        let emoji =
                            serenity::ReactionType::Unicode(ctx.config.reaction_error.clone());
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
    process_next_in_queue(ctx).await;

    result
}

/// Process the next item in the queue after a callback completes.
pub async fn process_next_in_queue(ctx: &BridgeContext) {
    let dequeue_result = ctx
        .message_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .complete_active();

    // Update position displays for remaining entries
    if let Some(http) = &ctx.http {
        for (wait_msg_id, ch_id_str, new_pos) in &dequeue_result.position_updates {
            let ch_id: u64 = ch_id_str.parse().unwrap_or(0);
            let msg_id: u64 = wait_msg_id.parse().unwrap_or(0);
            if ch_id > 0 && msg_id > 0 {
                let new_text =
                    format!("⏳ 待機中です（{}番目）。順番が来たら応答します。", new_pos);
                let edit = serenity::EditMessage::new().content(&new_text);
                ctx.rate_limiter
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
        if let (Some(http), Some(wait_id)) = (&ctx.http, &next.waiting_message_id) {
            let wait_msg_id: u64 = wait_id.parse().unwrap_or(0);
            if channel_id > 0 && wait_msg_id > 0 {
                let edit = serenity::EditMessage::new().content("🔄 応答を生成中...");
                ctx.rate_limiter
                    .acquire(rate_limiter::Route::ChannelMessage(channel_id))
                    .await;
                let _ = serenity::ChannelId::new(channel_id)
                    .edit_message(http, serenity::MessageId::new(wait_msg_id), edit)
                    .await;
            }
        }

        // Start typing
        let typing = if let Some(http) = &ctx.http {
            if channel_id > 0 {
                Some(serenity::ChannelId::new(channel_id).start_typing(http))
            } else {
                None
            }
        } else {
            None
        };

        // Add processing reaction to original message
        if let Some(http) = &ctx.http {
            if channel_id > 0 && original_msg_id > 0 {
                let emoji = serenity::ReactionType::Unicode(ctx.config.reaction_processing.clone());
                let _ = http
                    .create_reaction(
                        serenity::ChannelId::new(channel_id),
                        serenity::MessageId::new(original_msg_id),
                        &emoji,
                    )
                    .await;
                // Remove queue reaction
                let queue_emoji =
                    serenity::ReactionType::Unicode(ctx.config.reaction_queued.clone());
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
        if let Ok(mut cbs) = ctx.pending_callbacks.lock() {
            cbs.insert(
                next.callback_id.clone(),
                crate::bridge::CallbackContext {
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
        write_message(&ctx.stdout, &notif);

        tracing::info!(
            callback_id = %next.callback_id,
            "Dequeued and started processing"
        );
    }
}

/// Handle timed-out queue entries -- edit their waiting messages and remove them.
pub async fn handle_queue_timeouts(ctx: &BridgeContext) {
    let expired = ctx
        .message_queue
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain_expired();

    if expired.is_empty() {
        return;
    }

    if let Some(http) = &ctx.http {
        for entry in &expired {
            if let Some(wait_id) = &entry.waiting_message_id {
                let ch_id: u64 = entry.channel_id.parse().unwrap_or(0);
                let wait_msg_id: u64 = wait_id.parse().unwrap_or(0);
                if ch_id > 0 && wait_msg_id > 0 {
                    let timeout_text = "⌛ タイムアウトしました。もう一度お試しください。";
                    let edit = serenity::EditMessage::new().content(timeout_text);
                    ctx.rate_limiter
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
