//! Streaming response handling for progressive message editing.
//!
//! Buffers incoming stream chunks and periodically edits the Discord message
//! to show the accumulated content. This provides a "typing" effect for
//! long LLM responses.

use crate::bridge::{BridgeContext, ResponseMode, StreamState, STREAM_EDIT_INTERVAL};
use crate::protocol::JsonRpcRequest;
use crate::rate_limiter;
use serenity::all as serenity;

/// Handle an incoming stream chunk notification -- buffer content for periodic editing.
pub async fn handle_stream_chunk(ctx: &BridgeContext, request: &JsonRpcRequest) {
    let Some(params) = &request.params else {
        return;
    };
    let callback_id = params
        .get("callback_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let chunk = params.get("chunk").and_then(|v| v.as_str()).unwrap_or("");

    if callback_id.is_empty() || chunk.is_empty() {
        return;
    }

    // Check if stream already exists -- if so, just append
    {
        let mut states = ctx.streaming_states.lock().await;
        if let Some(state) = states.get_mut(callback_id) {
            state.buffer.push_str(chunk);
            return;
        }
    }

    // New stream -- extract callback context (drop lock before await)
    let (channel_id, interaction_token) = {
        let cb_ctx = ctx
            .pending_callbacks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(cb) = cb_ctx.get(callback_id) else {
            tracing::debug!("Stream chunk for unknown callback: {callback_id}");
            return;
        };
        let ch: u64 = cb.channel_id.parse().unwrap_or(0);
        let token = match &cb.response_mode {
            ResponseMode::Interaction { token } => Some(token.clone()),
            _ => None,
        };
        (ch, token)
    };

    let Some(http) = &ctx.http else {
        return;
    };

    let initial_content = format!("{chunk}…");
    let callback_id_owned = callback_id.to_string();

    if let Some(ref token) = interaction_token {
        let edit = serenity::EditInteractionResponse::new().content(&initial_content);
        let _ = http
            .edit_original_interaction_response(token, &edit, vec![])
            .await;
        ctx.streaming_states.lock().await.insert(
            callback_id_owned,
            StreamState {
                message_id: 0,
                channel_id,
                buffer: chunk.to_string(),
                last_edit: std::time::Instant::now(),
                interaction_token: Some(token.clone()),
            },
        );
    } else if channel_id > 0 {
        let cid = serenity::ChannelId::new(channel_id);
        let msg_builder = serenity::CreateMessage::new().content(&initial_content);
        ctx.rate_limiter
            .acquire(rate_limiter::Route::ChannelMessage(channel_id))
            .await;
        if let Ok(sent) = cid.send_message(http, msg_builder).await {
            ctx.streaming_states.lock().await.insert(
                callback_id_owned,
                StreamState {
                    message_id: sent.id.get(),
                    channel_id,
                    buffer: chunk.to_string(),
                    last_edit: std::time::Instant::now(),
                    interaction_token: None,
                },
            );
        }
    }
}

/// Periodically flush streaming edit buffers (called from main loop every 500ms).
/// Only edits if >= 1.5s have elapsed since last edit.
pub async fn flush_streaming_edits(ctx: &BridgeContext) {
    let Some(http) = &ctx.http else {
        return;
    };

    // Collect pending edits (drop lock before awaiting)
    let pending: Vec<_> = {
        let mut states = ctx.streaming_states.lock().await;
        let now = std::time::Instant::now();
        states
            .iter_mut()
            .filter(|(_, s)| now.duration_since(s.last_edit) >= STREAM_EDIT_INTERVAL)
            .map(|(_, s)| {
                s.last_edit = now;
                (
                    s.channel_id,
                    s.message_id,
                    format!("{}…", s.buffer),
                    s.interaction_token.clone(),
                )
            })
            .collect()
    };

    for (channel_id, message_id, content, interaction_token) in pending {
        if let Some(ref token) = interaction_token {
            let edit = serenity::EditInteractionResponse::new().content(&content);
            let _ = http
                .edit_original_interaction_response(token, &edit, vec![])
                .await;
        } else if channel_id > 0 && message_id > 0 {
            ctx.rate_limiter
                .acquire(rate_limiter::Route::ChannelMessage(channel_id))
                .await;
            let edit = serenity::EditMessage::new().content(&content);
            let _ = serenity::ChannelId::new(channel_id)
                .edit_message(http, serenity::MessageId::new(message_id), edit)
                .await;
        }
    }
}
