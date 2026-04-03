//! Agent-level message queue for sequential processing.
//!
//! Ensures only one callback is active at a time per bot (agent).
//! Queued messages get a "waiting" indicator that is edited when their turn comes.
//!
//! - Max queue size: 5 (configurable via DISCORD_QUEUE_MAX)
//! - Timeout: 3 minutes (configurable via DISCORD_QUEUE_TIMEOUT_SECS)
//!
//! Flow:
//!   Message arrives → if idle: process immediately
//!                   → if busy & queue has space: send "⏳ waiting" msg, enqueue
//!                   → if busy & queue full: send "queue full" msg, reject
//!   Active completes → dequeue next → edit waiting msg → start processing

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A queued message waiting for processing.
#[derive(Debug)]
#[allow(dead_code)]
pub struct QueueEntry {
    /// Unique callback ID for this message.
    pub callback_id: String,
    /// Session ID (channel_id:user_id) for context scoping.
    pub session_id: String,
    /// Discord channel ID where the message was sent.
    pub channel_id: String,
    /// Discord message ID of the original user message.
    pub original_message_id: String,
    /// Discord message ID of the bot's "waiting" indicator message.
    /// This message will be edited to the response when the entry is processed.
    pub waiting_message_id: Option<String>,
    /// Guild ID (optional).
    pub guild_id: Option<String>,
    /// Author display name.
    pub author_name: String,
    /// Author ID.
    pub author_id: String,
    /// Whether the original was a reply to a bot message.
    pub is_reply: bool,
    /// Interaction token (Some = slash command, None = message).
    pub interaction_token: Option<String>,
    /// The notification payload to send when this entry becomes active.
    pub notification_payload: serde_json::Value,
    /// When this entry was enqueued.
    pub enqueued_at: Instant,
}

/// Result of attempting to enqueue a new message.
pub enum EnqueueResult {
    /// No active item — process immediately (no queuing needed).
    ProcessNow,
    /// Queued at the given position (1-indexed).
    Queued(usize),
    /// Queue is full — reject.
    Full,
}

/// Result of completing the active item.
pub struct DequeueResult {
    /// The next entry to process, if any.
    pub next: Option<QueueEntry>,
    /// Entries whose position changed and need their waiting message updated.
    /// Each tuple: (waiting_message_id, channel_id, new_position).
    pub position_updates: Vec<(String, String, usize)>,
}

/// Agent-level (per-bot) message queue.
#[allow(dead_code)]
pub struct MessageQueue {
    /// Currently processing entry's callback_id (None = idle).
    active_callback_id: Option<String>,
    /// Waiting entries in FIFO order.
    waiting: VecDeque<QueueEntry>,
    /// Maximum queue size.
    max_size: usize,
    /// Timeout duration for queued entries.
    timeout: Duration,
}

#[allow(dead_code)]
impl MessageQueue {
    pub fn new() -> Self {
        Self::with_config(5, 180)
    }

    pub fn with_config(max_size: usize, timeout_secs: u64) -> Self {
        Self {
            active_callback_id: None,
            waiting: VecDeque::new(),
            max_size: max_size.max(1),
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    /// Whether the queue has an active (processing) item.
    pub fn is_busy(&self) -> bool {
        self.active_callback_id.is_some()
    }

    /// Number of items waiting in the queue.
    pub fn waiting_count(&self) -> usize {
        self.waiting.len()
    }

    /// Try to enqueue a new message.
    /// Returns `ProcessNow` if idle, `Queued(position)` if added, `Full` if at capacity.
    pub fn try_enqueue(&mut self, entry: QueueEntry) -> EnqueueResult {
        if self.active_callback_id.is_none() {
            // Idle — mark as active, no queuing needed
            self.active_callback_id = Some(entry.callback_id.clone());
            // Entry is consumed by the caller for immediate processing
            return EnqueueResult::ProcessNow;
        }

        if self.waiting.len() >= self.max_size {
            return EnqueueResult::Full;
        }

        let position = self.waiting.len() + 1;
        self.waiting.push_back(entry);
        EnqueueResult::Queued(position)
    }

    /// Mark the active item as completed and dequeue the next.
    pub fn complete_active(&mut self) -> DequeueResult {
        self.active_callback_id = None;

        if let Some(next) = self.waiting.pop_front() {
            self.active_callback_id = Some(next.callback_id.clone());

            // Collect position updates for remaining entries
            let position_updates = self
                .waiting
                .iter()
                .enumerate()
                .filter_map(|(i, entry)| {
                    entry
                        .waiting_message_id
                        .as_ref()
                        .map(|wid| (wid.clone(), entry.channel_id.clone(), i + 1))
                })
                .collect();

            DequeueResult {
                next: Some(next),
                position_updates,
            }
        } else {
            DequeueResult {
                next: None,
                position_updates: vec![],
            }
        }
    }

    /// Remove and return timed-out entries.
    pub fn drain_expired(&mut self) -> Vec<QueueEntry> {
        let now = Instant::now();
        let mut expired = Vec::new();
        let mut i = 0;
        while i < self.waiting.len() {
            if now.duration_since(self.waiting[i].enqueued_at) > self.timeout {
                expired.push(self.waiting.remove(i).unwrap());
            } else {
                i += 1;
            }
        }
        expired
    }

    /// Mutable iterator over waiting entries (for updating waiting_message_id).
    pub fn waiting_iter_mut(&mut self) -> impl Iterator<Item = &mut QueueEntry> {
        self.waiting.iter_mut()
    }

    /// Get current queue status for bridge_status tool.
    pub fn status(&self) -> QueueStatus {
        QueueStatus {
            active: self.active_callback_id.is_some(),
            waiting: self.waiting.len(),
            max_size: self.max_size,
            timeout_secs: self.timeout.as_secs(),
        }
    }
}

/// Snapshot of queue state for status reporting.
#[derive(Debug, serde::Serialize)]
#[allow(dead_code)]
pub struct QueueStatus {
    pub active: bool,
    pub waiting: usize,
    pub max_size: usize,
    pub timeout_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_entry(id: &str) -> QueueEntry {
        QueueEntry {
            callback_id: id.to_string(),
            session_id: "100:1".to_string(),
            channel_id: "100".to_string(),
            original_message_id: id.to_string(),
            waiting_message_id: Some(format!("wait-{id}")),
            guild_id: None,
            author_name: "test".to_string(),
            author_id: "1".to_string(),
            is_reply: false,
            interaction_token: None,
            notification_payload: json!({}),
            enqueued_at: Instant::now(),
        }
    }

    #[test]
    fn test_idle_processes_immediately() {
        let mut q = MessageQueue::new();
        let entry = make_entry("1");
        assert!(!q.is_busy());
        match q.try_enqueue(entry) {
            EnqueueResult::ProcessNow => {}
            _ => panic!("Expected ProcessNow"),
        }
        assert!(q.is_busy());
        assert_eq!(q.waiting_count(), 0);
    }

    #[test]
    fn test_busy_queues() {
        let mut q = MessageQueue::new();
        // First — becomes active
        let _ = q.try_enqueue(make_entry("1"));

        // Second — queued at position 1
        match q.try_enqueue(make_entry("2")) {
            EnqueueResult::Queued(pos) => assert_eq!(pos, 1),
            _ => panic!("Expected Queued"),
        }
        assert_eq!(q.waiting_count(), 1);
    }

    #[test]
    fn test_queue_full() {
        let mut q = MessageQueue::with_config(2, 180);
        let _ = q.try_enqueue(make_entry("active"));
        let _ = q.try_enqueue(make_entry("wait-1"));
        let _ = q.try_enqueue(make_entry("wait-2"));

        match q.try_enqueue(make_entry("overflow")) {
            EnqueueResult::Full => {}
            _ => panic!("Expected Full"),
        }
    }

    #[test]
    fn test_complete_and_dequeue() {
        let mut q = MessageQueue::new();
        let _ = q.try_enqueue(make_entry("1"));
        let _ = q.try_enqueue(make_entry("2"));
        let _ = q.try_enqueue(make_entry("3"));

        let result = q.complete_active();
        assert!(result.next.is_some());
        assert_eq!(result.next.as_ref().unwrap().callback_id, "2");
        // Position updates for entry "3": now at position 1
        assert_eq!(result.position_updates.len(), 1);
        assert_eq!(result.position_updates[0].2, 1); // new position

        assert!(q.is_busy());
        assert_eq!(q.waiting_count(), 1);
    }

    #[test]
    fn test_complete_when_empty() {
        let mut q = MessageQueue::new();
        let _ = q.try_enqueue(make_entry("1"));

        let result = q.complete_active();
        assert!(result.next.is_none());
        assert!(!q.is_busy());
    }

    #[test]
    fn test_drain_expired() {
        let mut q = MessageQueue::with_config(5, 0); // 0 second timeout = everything expires
        let _ = q.try_enqueue(make_entry("active"));
        let _ = q.try_enqueue(make_entry("wait-1"));
        let _ = q.try_enqueue(make_entry("wait-2"));

        // Sleep briefly to ensure entries are past 0-second timeout
        std::thread::sleep(Duration::from_millis(10));

        let expired = q.drain_expired();
        assert_eq!(expired.len(), 2);
        assert_eq!(q.waiting_count(), 0);
    }
}
