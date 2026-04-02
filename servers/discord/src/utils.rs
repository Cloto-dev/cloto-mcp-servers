//! Utility functions ported from ai_karin.
//!
//! Snowflake ID ↔ timestamp conversion and message splitting.

use chrono::{DateTime, TimeZone, Utc};
use unicode_segmentation::UnicodeSegmentation;

/// Discord epoch: 2015-01-01T00:00:00Z in milliseconds.
const DISCORD_EPOCH: i64 = 1420070400000;

/// Convert a UTC timestamp to a Discord Snowflake ID.
pub fn timestamp_to_snowflake(time: DateTime<Utc>) -> u64 {
    let timestamp_ms = time.timestamp_millis();
    if timestamp_ms > DISCORD_EPOCH {
        ((timestamp_ms - DISCORD_EPOCH) << 22) as u64
    } else {
        0
    }
}

/// Convert a Discord Snowflake ID to a UTC timestamp.
#[allow(dead_code)]
pub fn snowflake_to_timestamp(snowflake: u64) -> Option<DateTime<Utc>> {
    let ms = (snowflake >> 22) as i64 + DISCORD_EPOCH;
    Utc.timestamp_millis_opt(ms).single()
}

/// Split a message into chunks of at most `limit` bytes, respecting grapheme boundaries.
/// Ported from ai_karin's message_handler.rs.
pub fn split_message(text: &str, limit: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for grapheme in text.graphemes(true) {
        if current.len() + grapheme.len() > limit && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(grapheme);
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Truncate a string to at most `max_len` characters, appending "..." if truncated.
pub fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }
    // Find a safe char boundary
    let mut end = max_len.saturating_sub(3);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

/// Strip @everyone and @here from a message.
pub fn sanitize_mentions(text: &str) -> String {
    text.replace("@everyone", "@\u{200b}everyone")
        .replace("@here", "@\u{200b}here")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snowflake_roundtrip() {
        let now = Utc::now();
        let snowflake = timestamp_to_snowflake(now);
        let recovered = snowflake_to_timestamp(snowflake).unwrap();
        // Snowflake has ~4ms resolution due to bit shifting
        assert!((now.timestamp_millis() - recovered.timestamp_millis()).abs() < 5);
    }

    #[test]
    fn test_split_message_under_limit() {
        let chunks = split_message("hello world", 2000);
        assert_eq!(chunks, vec!["hello world"]);
    }

    #[test]
    fn test_split_message_over_limit() {
        let long = "a".repeat(2500);
        let chunks = split_message(&long, 2000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 2000);
        assert_eq!(chunks[1].len(), 500);
    }

    #[test]
    fn test_sanitize_mentions() {
        let input = "hello @everyone and @here";
        let output = sanitize_mentions(input);
        assert!(!output.contains("@everyone"));
        assert!(!output.contains("@here"));
    }
}
