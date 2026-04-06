//! Shared context and type definitions for the Discord bridge.
//!
//! `BridgeContext` bundles all shared resources (HTTP client, config, rate limiter,
//! queues, etc.) into a single struct, replacing the 10+ parameter lists that were
//! threaded through every function.

use crate::config::DiscordConfig;
use crate::queue::MessageQueue;
use crate::rate_limiter::RateLimiter;
use serde_json::Value;
use serenity::all as serenity;
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

/// How the callback response should be delivered.
pub enum ResponseMode {
    /// Regular channel message (with optional reply-to).
    Message { is_reply: bool },
    /// Edit a deferred interaction response.
    Interaction { token: String },
}

/// Context stored for pending callbacks awaiting kernel response.
pub struct CallbackContext {
    pub channel_id: String,
    #[allow(dead_code)]
    pub guild_id: Option<String>,
    pub message_id: String,
    #[allow(dead_code)]
    pub author_name: String,
    /// How to deliver the response.
    pub response_mode: ResponseMode,
    /// Typing indicator guard -- dropping this stops the typing indicator.
    pub _typing: Option<serenity::http::Typing>,
}

/// State for an active streaming response (progressive message editing).
pub struct StreamState {
    /// The Discord message ID being edited with streaming content.
    pub message_id: u64,
    pub channel_id: u64,
    /// Accumulated content buffer.
    pub buffer: String,
    /// Last time we edited the message.
    pub last_edit: std::time::Instant,
    /// Whether this is an interaction response (uses webhook edit instead).
    pub interaction_token: Option<String>,
}

/// Minimum interval between streaming edits (1.5 seconds).
pub const STREAM_EDIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1500);

/// Internal counters for bridge health monitoring.
pub struct BridgeStats {
    pub connected_since: std::sync::Mutex<Option<std::time::Instant>>,
    pub messages_received: std::sync::atomic::AtomicU64,
    pub messages_sent: std::sync::atomic::AtomicU64,
    pub errors: std::sync::atomic::AtomicU64,
    pub last_event_at: std::sync::Mutex<Option<std::time::Instant>>,
    pub start_time: std::time::Instant,
}

pub type PendingCallbacks = Arc<Mutex<HashMap<String, CallbackContext>>>;
pub type BotContext = Arc<std::sync::Mutex<Option<serenity::Context>>>;
pub type SharedQueue = Arc<Mutex<MessageQueue>>;
pub type StreamingStates = Arc<tokio::sync::Mutex<HashMap<String, StreamState>>>;

/// Bounded ring buffer tracking recently processed callback IDs for idempotency.
pub struct ProcessedCallbacks {
    ids: std::collections::VecDeque<String>,
    max_size: usize,
}

impl ProcessedCallbacks {
    pub fn new(max_size: usize) -> Self {
        Self {
            ids: std::collections::VecDeque::with_capacity(max_size),
            max_size,
        }
    }

    /// Returns `true` if newly added, `false` if already processed (duplicate).
    pub fn mark_processed(&mut self, id: String) -> bool {
        if self.ids.iter().any(|existing| existing == &id) {
            return false;
        }
        if self.ids.len() >= self.max_size {
            self.ids.pop_front();
        }
        self.ids.push_back(id);
        true
    }

    pub fn contains(&self, id: &str) -> bool {
        self.ids.iter().any(|existing| existing == id)
    }
}

/// Conversation chunk tracker for session_id generation.
///
/// Groups messages into chunks based on time gaps. Within the same
/// channel:user pair, a new chunk starts when the gap between messages
/// exceeds `gap_threshold`.  The session_id format is `"{channel}:{user}:{chunk}"`.
pub struct ChunkTracker {
    /// "channel:user" → (current_chunk_counter, last_message_at)
    sessions: HashMap<String, (u32, std::time::Instant)>,
    gap_threshold: std::time::Duration,
}

impl ChunkTracker {
    pub fn new(gap_minutes: u64) -> Self {
        Self {
            sessions: HashMap::new(),
            gap_threshold: std::time::Duration::from_secs(gap_minutes * 60),
        }
    }

    /// Return a session_id for the given channel/user, incrementing the chunk
    /// counter when the time gap since the last message exceeds the threshold.
    pub fn get_session_id(&mut self, channel: u64, user: u64) -> String {
        let key = format!("{channel}:{user}");
        let now = std::time::Instant::now();

        let chunk = if let Some((chunk, last_msg)) = self.sessions.get_mut(&key) {
            if now.duration_since(*last_msg) > self.gap_threshold {
                *chunk += 1;
            }
            *last_msg = now;
            *chunk
        } else {
            self.sessions.insert(key.clone(), (1, now));
            1
        };

        format!("{key}:{chunk}")
    }
}

/// Central shared context for the Discord bridge.
///
/// Bundles all shared resources into a single struct so that functions
/// can accept `&BridgeContext` instead of 10+ individual parameters.
pub struct BridgeContext {
    pub http: Option<Arc<serenity::Http>>,
    pub config: Arc<DiscordConfig>,
    pub rate_limiter: Arc<RateLimiter>,
    pub pending_callbacks: PendingCallbacks,
    pub bot_context: BotContext,
    pub bridge_stats: Arc<BridgeStats>,
    pub message_queue: SharedQueue,
    pub streaming_states: StreamingStates,
    pub stdout: Arc<Mutex<io::Stdout>>,
    pub bot_user_id: Arc<std::sync::atomic::AtomicU64>,
    pub processed_callbacks: Arc<Mutex<ProcessedCallbacks>>,
    pub chunk_tracker: Arc<Mutex<ChunkTracker>>,
}

/// Write a JSON-serializable message to stdout (JSON-RPC transport).
pub fn write_message<T: serde::Serialize>(stdout: &Arc<Mutex<io::Stdout>>, msg: &T) {
    if let Ok(json_str) = serde_json::to_string(msg) {
        if let Ok(mut out) = stdout.lock() {
            let _ = writeln!(out, "{json_str}");
            let _ = out.flush();
        }
    }
}

/// Build an ephemeral interaction response payload (only visible to the user who triggered it).
pub fn ephemeral_response(content: &str) -> Value {
    serde_json::json!({
        "type": 4,
        "data": {
            "content": content,
            "flags": 64
        }
    })
}
