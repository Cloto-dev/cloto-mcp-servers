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

/// Strip bot mention (`<@ID>` and `<@!ID>`) from message content.
pub fn strip_bot_mention(content: &str, bot_id: u64) -> String {
    let plain = format!("<@{bot_id}>");
    let nick = format!("<@!{bot_id}>");
    content
        .replace(&plain, "")
        .replace(&nick, "")
        .trim()
        .to_string()
}

/// Parse a backtick-wrapped direct tool command.
///
/// Format: `` `tool_name [key=value ...]` ``
///
/// Returns `None` if the message isn't a valid backtick command or the tool
/// name isn't in `known_tools`.
pub fn parse_direct_command(
    content: &str,
    known_tools: &[&str],
) -> Option<(String, std::collections::HashMap<String, String>)> {
    let trimmed = content.trim();
    // Must be wrapped in exactly one pair of backticks (not triple ```)
    if !trimmed.starts_with('`') || !trimmed.ends_with('`') || trimmed.starts_with("```") {
        return None;
    }
    let inner = trimmed[1..trimmed.len() - 1].trim();
    if inner.is_empty() {
        return None;
    }

    let mut parts = inner.split_whitespace();
    let tool_name = parts.next()?.to_string();

    if !known_tools.contains(&tool_name.as_str()) {
        return None;
    }

    let mut args = std::collections::HashMap::new();
    for part in parts {
        if let Some((key, value)) = part.split_once('=') {
            args.insert(key.to_string(), value.to_string());
        }
    }

    Some((tool_name, args))
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

    #[test]
    fn test_parse_direct_command_valid() {
        let tools = vec!["get_history", "list_channels", "recall"];
        let result = parse_direct_command("`get_history limit=5`", &tools);
        assert!(result.is_some());
        let (name, args) = result.unwrap();
        assert_eq!(name, "get_history");
        assert_eq!(args.get("limit").unwrap(), "5");
    }

    #[test]
    fn test_parse_direct_command_no_args() {
        let tools = vec!["list_channels"];
        let result = parse_direct_command("`list_channels`", &tools);
        assert!(result.is_some());
        let (name, args) = result.unwrap();
        assert_eq!(name, "list_channels");
        assert!(args.is_empty());
    }

    #[test]
    fn test_parse_direct_command_unknown_tool() {
        let tools = vec!["get_history"];
        assert!(parse_direct_command("`unknown_tool`", &tools).is_none());
    }

    #[test]
    fn test_parse_direct_command_triple_backtick() {
        let tools = vec!["get_history"];
        assert!(parse_direct_command("```get_history```", &tools).is_none());
    }

    #[test]
    fn test_parse_direct_command_normal_text() {
        let tools = vec!["get_history"];
        assert!(parse_direct_command("hello world", &tools).is_none());
    }
}
