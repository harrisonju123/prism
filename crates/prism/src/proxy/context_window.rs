use crate::types::Message;

/// Estimate token count via character heuristic (4 chars ≈ 1 token).
pub fn estimate_tokens(text: &str) -> u32 {
    (text.len() / 4).max(1) as u32
}

fn message_text(msg: &Message) -> String {
    match &msg.content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(" "),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

/// Estimate total tokens for a message list.
pub fn estimate_messages_tokens(messages: &[Message]) -> u32 {
    messages.iter().map(|m| estimate_tokens(&message_text(m))).sum()
}

/// Truncate oldest non-system messages until estimated tokens fit budget.
/// Returns number of messages dropped.
pub fn truncate_to_fit(messages: &mut Vec<Message>, budget: u32) -> usize {
    let mut total = estimate_messages_tokens(messages);
    let mut dropped = 0;
    while messages.len() > 1 && total > budget {
        // Find first non-system message and remove it
        let Some(idx) = messages.iter().position(|m| m.role != "system") else {
            break; // Only system messages remain — cannot truncate further
        };
        total = total.saturating_sub(estimate_tokens(&message_text(&messages[idx])));
        messages.remove(idx);
        dropped += 1;
    }
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;

    fn make_msg(role: &str, content: &str) -> Message {
        Message {
            role: role.to_string(),
            content: Some(serde_json::Value::String(content.to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens("hello"), 1); // 5/4 = 1
        assert_eq!(estimate_tokens("hello world"), 2); // 11/4 = 2
        assert_eq!(estimate_tokens(""), 1); // max(0,1) = 1
    }

    #[test]
    fn test_truncate_to_fit() {
        let mut messages = vec![
            make_msg("system", "You are a helpful assistant."),
            make_msg("user", "message one"),
            make_msg("assistant", "response one"),
            make_msg("user", "message two"),
        ];
        // With a very small budget, should drop non-system messages
        let dropped = truncate_to_fit(&mut messages, 10);
        assert!(dropped > 0);
        // System message should be preserved
        assert_eq!(messages[0].role, "system");
    }

    #[test]
    fn test_no_truncation_needed() {
        let mut messages = vec![make_msg("user", "hi")];
        let dropped = truncate_to_fit(&mut messages, 10_000);
        assert_eq!(dropped, 0);
        assert_eq!(messages.len(), 1);
    }
}
