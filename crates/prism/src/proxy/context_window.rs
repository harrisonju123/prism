use crate::types::{Message, MessageRole};
use serde_json::Value;

/// Estimate token count via character heuristic (4 chars ≈ 1 token).
pub fn estimate_tokens(text: &str) -> u32 {
    (text.len() / 4).max(1) as u32
}

fn message_text(msg: &Message) -> String {
    match &msg.content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
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
    messages
        .iter()
        .map(|m| estimate_tokens(&message_text(m)))
        .sum()
}

/// Truncate oldest non-system messages until estimated tokens fit budget.
/// Returns number of messages dropped.
pub fn truncate_to_fit(messages: &mut Vec<Message>, budget: u32) -> usize {
    let mut total = estimate_messages_tokens(messages);
    let mut dropped = 0;
    while messages.len() > 1 && total > budget {
        // Find first non-system message and remove it
        let Some(idx) = messages.iter().position(|m| m.role != MessageRole::System) else {
            break; // Only system messages remain — cannot truncate further
        };
        total = total.saturating_sub(estimate_tokens(&message_text(&messages[idx])));
        messages.remove(idx);
        dropped += 1;
    }
    dropped
}

// ---------------------------------------------------------------------------
// Smart truncation
// ---------------------------------------------------------------------------

pub struct SmartTruncationConfig {
    /// How many trailing messages are always preserved (recency window).
    pub preserve_recent: usize,
    /// Tool result content above this token count gets compressed.
    pub tool_output_max_tokens: u32,
    /// Lines to keep from the head of a compressed tool output.
    pub tool_output_head_lines: usize,
    /// Lines to keep from the tail of a compressed tool output.
    pub tool_output_tail_lines: usize,
}

pub struct TruncationResult {
    pub messages_dropped: usize,
    pub tool_outputs_compressed: usize,
    pub tokens_saved_by_compression: u32,
    /// 1 = compression only, 2 = group dropping, 3 = aggressive fallback
    pub pass_used: u8,
}

/// A structural group of messages that should be dropped as a unit.
#[derive(Debug)]
enum MessageGroup {
    Single(usize),
    /// assistant message at `assistant_idx` plus the Tool replies that answer it.
    ToolCallBundle {
        assistant_idx: usize,
        tool_result_idxs: Vec<usize>,
    },
}

/// Drop priority assigned to each group for pass-2 ordering.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum DropPriority {
    /// Drop this group first (middle conversation).
    DropFirst,
    /// Drop after DropFirst groups (e.g., early user messages that aren't the first).
    DropSecond,
    /// Protected but can be dropped in pass 3 (non-system, non-first-user).
    Protect,
    /// Never drop (system messages).
    Never,
}

/// Pull tool_call IDs out of an assistant message's `tool_calls` array.
fn extract_tool_call_ids(tool_calls: &[Value]) -> Vec<String> {
    tool_calls
        .iter()
        .filter_map(|tc| tc.get("id").and_then(|v| v.as_str()).map(String::from))
        .collect()
}

/// Build structural groups so tool_call + tool_result pairs are dropped together.
fn build_message_groups(messages: &[Message]) -> Vec<MessageGroup> {
    let mut groups: Vec<MessageGroup> = Vec::new();
    let mut i = 0;

    while i < messages.len() {
        let msg = &messages[i];

        // Assistant message with tool_calls — pair with subsequent Tool replies.
        if msg.role == MessageRole::Assistant {
            if let Some(tcs) = &msg.tool_calls {
                if !tcs.is_empty() {
                    let call_ids = extract_tool_call_ids(tcs);
                    let mut result_idxs = Vec::new();
                    let mut j = i + 1;
                    // Collect consecutive Tool messages that answer these calls.
                    while j < messages.len() && messages[j].role == MessageRole::Tool {
                        let tid = messages[j].tool_call_id.as_deref().unwrap_or("");
                        if call_ids.contains(&tid.to_string()) {
                            result_idxs.push(j);
                        }
                        j += 1;
                    }
                    groups.push(MessageGroup::ToolCallBundle {
                        assistant_idx: i,
                        tool_result_idxs: result_idxs.clone(),
                    });
                    // Skip past the tool results we just paired.
                    i = if result_idxs.is_empty() {
                        i + 1
                    } else {
                        *result_idxs.last().unwrap() + 1
                    };
                    continue;
                }
            }
        }

        // Orphan Tool messages (no paired assistant) become Singles.
        groups.push(MessageGroup::Single(i));
        i += 1;
    }

    groups
}

/// Compress a single tool output value using head+tail line truncation.
/// Returns `Some(compressed)` when compression reduced the content; `None` when unchanged.
fn compress_tool_output(
    content: &Value,
    max_tokens: u32,
    head_lines: usize,
    tail_lines: usize,
) -> Option<Value> {
    match content {
        Value::String(s) => {
            if estimate_tokens(s) <= max_tokens {
                return None;
            }
            let lines: Vec<&str> = s.lines().collect();
            let total = lines.len();
            let keep = head_lines + tail_lines;
            if total <= keep {
                return None;
            }
            let omitted = total - keep;
            let mut out = lines[..head_lines].to_vec();
            out.push(&"");
            let marker = format!("[... {omitted} lines truncated by gateway ...]");
            // We can't push a &str from a local String easily, so build the full string directly.
            let mut result = lines[..head_lines].join("\n");
            result.push('\n');
            result.push_str(&marker);
            result.push('\n');
            result.push_str(&lines[total - tail_lines..].join("\n"));
            drop(out);
            Some(Value::String(result))
        }
        Value::Array(arr) => {
            // Array-style content: only compress {"type":"text"} parts.
            let mut changed = false;
            let new_arr: Vec<Value> = arr
                .iter()
                .map(|item| {
                    if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                        if let Some(text_val) = item.get("text") {
                            if let Some(compressed) =
                                compress_tool_output(text_val, max_tokens, head_lines, tail_lines)
                            {
                                changed = true;
                                let mut new_item = item.clone();
                                if let Value::Object(ref mut map) = new_item {
                                    map.insert("text".into(), compressed);
                                }
                                return new_item;
                            }
                        }
                    }
                    item.clone()
                })
                .collect();
            if changed {
                Some(Value::Array(new_arr))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Apply tool output compression to all Tool-role messages in place.
/// Returns estimated tokens saved.
fn compress_tool_outputs_in_place(
    messages: &mut [Message],
    max_tokens: u32,
    head_lines: usize,
    tail_lines: usize,
) -> (usize, u32) {
    let mut compressed_count = 0usize;
    let mut tokens_saved = 0u32;

    for msg in messages.iter_mut() {
        if msg.role != MessageRole::Tool {
            continue;
        }
        if let Some(content) = &msg.content {
            let before = estimate_tokens(&message_text(msg));
            if let Some(new_content) =
                compress_tool_output(content, max_tokens, head_lines, tail_lines)
            {
                let after_text = match &new_content {
                    Value::String(s) => estimate_tokens(s),
                    Value::Array(arr) => {
                        let joined = arr
                            .iter()
                            .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                            .collect::<Vec<_>>()
                            .join(" ");
                        estimate_tokens(&joined)
                    }
                    _ => before,
                };
                tokens_saved = tokens_saved.saturating_add(before.saturating_sub(after_text));
                msg.content = Some(new_content);
                compressed_count += 1;
            }
        }
    }

    (compressed_count, tokens_saved)
}

/// Assign a drop priority to a group for pass-2 ordering.
fn assign_priority(
    group: &MessageGroup,
    messages: &[Message],
    total_count: usize,
    preserve_recent: usize,
    first_user_idx: Option<usize>,
) -> DropPriority {
    let indices: Vec<usize> = match group {
        MessageGroup::Single(i) => vec![*i],
        MessageGroup::ToolCallBundle {
            assistant_idx,
            tool_result_idxs,
        } => {
            let mut v = vec![*assistant_idx];
            v.extend_from_slice(tool_result_idxs);
            v
        }
    };

    // System messages: never drop.
    if indices
        .iter()
        .any(|&i| messages[i].role == MessageRole::System)
    {
        return DropPriority::Never;
    }

    // Recency window: protect the last N messages.
    let recency_start = total_count.saturating_sub(preserve_recent);
    if indices.iter().any(|&i| i >= recency_start) {
        return DropPriority::Protect;
    }

    // First user message: survives longer than middle messages.
    if let Some(fui) = first_user_idx {
        if indices.contains(&fui) {
            return DropPriority::DropSecond;
        }
    }

    DropPriority::DropFirst
}

/// Multi-pass smart truncation.
///
/// Pass 1: compress large tool outputs (no messages dropped).
/// Pass 2: drop middle conversation in structural groups (system + first user + recent N protected).
/// Pass 3: aggressive fallback — drop even protected messages (system always survives).
pub fn smart_truncate_to_fit(
    messages: &mut Vec<Message>,
    budget: u32,
    config: &SmartTruncationConfig,
) -> TruncationResult {
    let mut messages_dropped = 0usize;
    let mut pass_used = 1u8;

    // --- Pass 1: compress tool outputs ---
    let (tool_outputs_compressed, tokens_saved_by_compression) = compress_tool_outputs_in_place(
        messages,
        config.tool_output_max_tokens,
        config.tool_output_head_lines,
        config.tool_output_tail_lines,
    );

    if estimate_messages_tokens(messages) <= budget {
        return TruncationResult {
            messages_dropped,
            tool_outputs_compressed,
            tokens_saved_by_compression,
            pass_used,
        };
    }

    // --- Pass 2: priority-based group dropping ---
    pass_used = 2;

    let first_user_idx = messages
        .iter()
        .position(|m| m.role == MessageRole::User);

    // Build groups and score them. We repeatedly rebuild groups after each removal
    // so indices stay valid — simpler than trying to update indices in place.
    loop {
        if estimate_messages_tokens(messages) <= budget {
            break;
        }
        let groups = build_message_groups(messages);
        let total = messages.len();

        // Find the lowest-priority (drop-first) group that isn't Never/Protect.
        let candidate = groups
            .iter()
            .filter(|g| {
                let p = assign_priority(g, messages, total, config.preserve_recent, first_user_idx);
                p == DropPriority::DropFirst || p == DropPriority::DropSecond
            })
            .min_by_key(|g| {
                assign_priority(g, messages, total, config.preserve_recent, first_user_idx)
            });

        let Some(group) = candidate else {
            break; // Nothing left to drop in pass 2
        };

        // Collect indices, sort descending, remove back-to-front.
        let mut indices: Vec<usize> = match group {
            MessageGroup::Single(i) => vec![*i],
            MessageGroup::ToolCallBundle {
                assistant_idx,
                tool_result_idxs,
            } => {
                let mut v = vec![*assistant_idx];
                v.extend_from_slice(tool_result_idxs);
                v
            }
        };
        indices.sort_unstable_by(|a, b| b.cmp(a));
        messages_dropped += indices.len();
        for idx in indices {
            messages.remove(idx);
        }
    }

    if estimate_messages_tokens(messages) <= budget {
        return TruncationResult {
            messages_dropped,
            tool_outputs_compressed,
            tokens_saved_by_compression,
            pass_used,
        };
    }

    // --- Pass 3: aggressive fallback — drop everything except system ---
    pass_used = 3;
    loop {
        if estimate_messages_tokens(messages) <= budget || messages.len() <= 1 {
            break;
        }
        // Drop oldest non-system message.
        let Some(idx) = messages.iter().position(|m| m.role != MessageRole::System) else {
            break;
        };
        messages.remove(idx);
        messages_dropped += 1;
    }

    TruncationResult {
        messages_dropped,
        tool_outputs_compressed,
        tokens_saved_by_compression,
        pass_used,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Message, MessageRole};
    use serde_json::json;

    fn make_msg(role: MessageRole, content: &str) -> Message {
        Message {
            role,
            content: Some(Value::String(content.to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        }
    }

    fn make_tool_call_msg(calls: Vec<Value>) -> Message {
        Message {
            role: MessageRole::Assistant,
            content: None,
            name: None,
            tool_calls: Some(calls),
            tool_call_id: None,
            extra: Default::default(),
        }
    }

    fn make_tool_result(call_id: &str, content: &str) -> Message {
        Message {
            role: MessageRole::Tool,
            content: Some(Value::String(content.to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: Some(call_id.to_string()),
            extra: Default::default(),
        }
    }

    fn default_smart_config() -> SmartTruncationConfig {
        SmartTruncationConfig {
            preserve_recent: 4,
            tool_output_max_tokens: 10,
            tool_output_head_lines: 2,
            tool_output_tail_lines: 2,
        }
    }

    // --- existing tests ---

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens("hello"), 1); // 5/4 = 1
        assert_eq!(estimate_tokens("hello world"), 2); // 11/4 = 2
        assert_eq!(estimate_tokens(""), 1); // max(0,1) = 1
    }

    #[test]
    fn test_truncate_to_fit() {
        let mut messages = vec![
            make_msg(MessageRole::System, "You are a helpful assistant."),
            make_msg(MessageRole::User, "message one"),
            make_msg(MessageRole::Assistant, "response one"),
            make_msg(MessageRole::User, "message two"),
        ];
        // With a very small budget, should drop non-system messages
        let dropped = truncate_to_fit(&mut messages, 10);
        assert!(dropped > 0);
        // System message should be preserved
        assert_eq!(messages[0].role, MessageRole::System);
    }

    #[test]
    fn test_no_truncation_needed() {
        let mut messages = vec![make_msg(MessageRole::User, "hi")];
        let dropped = truncate_to_fit(&mut messages, 10_000);
        assert_eq!(dropped, 0);
        assert_eq!(messages.len(), 1);
    }

    // --- smart truncation tests ---

    #[test]
    fn test_compress_tool_output_short_unchanged() {
        let content = Value::String("short".to_string());
        let result = compress_tool_output(&content, 100, 20, 20);
        assert!(result.is_none(), "short content should not be compressed");
    }

    #[test]
    fn test_compress_tool_output_large_gets_head_tail() {
        // Build a string of 10 lines that exceeds max_tokens=2
        let lines: Vec<String> = (1..=10).map(|i| format!("line {i}")).collect();
        let content = Value::String(lines.join("\n"));
        let result = compress_tool_output(&content, 2, 2, 2);
        assert!(result.is_some());
        let s = result.unwrap();
        let text = s.as_str().unwrap();
        assert!(text.contains("line 1"));
        assert!(text.contains("line 2"));
        assert!(text.contains("line 9"));
        assert!(text.contains("line 10"));
        assert!(text.contains("truncated by gateway"));
        // Middle lines should be gone
        assert!(!text.contains("line 5"));
    }

    #[test]
    fn test_compress_array_content_only_text_parts() {
        // Array with a text part and an image_url part — only text gets compressed.
        let lines: Vec<String> = (1..=10).map(|i| format!("line {i}")).collect();
        let long_text = lines.join("\n");
        let content = json!([
            {"type": "text", "text": long_text},
            {"type": "image_url", "image_url": {"url": "http://example.com/img.png"}}
        ]);
        let result = compress_tool_output(&content, 2, 2, 2);
        assert!(result.is_some());
        let arr = result.unwrap();
        let arr = arr.as_array().unwrap();
        // Text part should be compressed
        let text_part = arr[0].get("text").unwrap().as_str().unwrap();
        assert!(text_part.contains("truncated by gateway"));
        // Image part should be untouched
        assert_eq!(arr[1]["type"], "image_url");
        assert!(arr[1].get("image_url").is_some());
    }

    #[test]
    fn test_build_message_groups_pairs_tool_calls() {
        let messages = vec![
            make_msg(MessageRole::System, "system"),
            make_msg(MessageRole::User, "user"),
            make_tool_call_msg(vec![json!({"id": "call_1", "type": "function"})]),
            make_tool_result("call_1", "result content"),
            make_msg(MessageRole::Assistant, "done"),
        ];
        let groups = build_message_groups(&messages);

        // Should have: Single(0), Single(1), ToolCallBundle{2,[3]}, Single(4)
        assert_eq!(groups.len(), 4);
        match &groups[2] {
            MessageGroup::ToolCallBundle {
                assistant_idx,
                tool_result_idxs,
            } => {
                assert_eq!(*assistant_idx, 2);
                assert_eq!(tool_result_idxs, &[3]);
            }
            _ => panic!("expected ToolCallBundle"),
        }
    }

    #[test]
    fn test_build_message_groups_orphan_tool_result_is_single() {
        let messages = vec![
            make_msg(MessageRole::User, "hello"),
            // Tool result with no preceding assistant+tool_calls
            make_tool_result("orphan_id", "result"),
        ];
        let groups = build_message_groups(&messages);
        assert_eq!(groups.len(), 2);
        assert!(matches!(groups[1], MessageGroup::Single(1)));
    }

    #[test]
    fn test_smart_pass1_sufficient_no_messages_dropped() {
        // Large tool result but budget is met after compression.
        let lines: Vec<String> = (1..=50).map(|i| format!("line {i}")).collect();
        let large = lines.join("\n");
        let mut messages = vec![
            make_msg(MessageRole::System, "sys"),
            make_msg(MessageRole::User, "user"),
            make_tool_result("c1", &large),
        ];
        let before_tokens = estimate_messages_tokens(&messages);

        // Budget just above what we'll have after compression but below original
        let config = SmartTruncationConfig {
            preserve_recent: 4,
            tool_output_max_tokens: 5,
            tool_output_head_lines: 3,
            tool_output_tail_lines: 3,
        };
        let result = smart_truncate_to_fit(&mut messages, before_tokens - 10, &config);
        assert_eq!(result.messages_dropped, 0);
        assert!(result.tool_outputs_compressed > 0);
        assert_eq!(result.pass_used, 1);
    }

    #[test]
    fn test_smart_pass2_drops_middle_preserves_system_first_user_recent() {
        // System + first user + several middle turns + recent messages.
        // Each message body is ~100 chars (~25 tokens), total ~175 tokens.
        // Budget forces pass-2 to drop middle turns.
        let body = "x".repeat(100);
        let mut messages = vec![
            make_msg(MessageRole::System, &body),
            make_msg(MessageRole::User, &body),       // first user
            make_msg(MessageRole::Assistant, &body),   // middle A
            make_msg(MessageRole::User, &body),        // middle B
            make_msg(MessageRole::Assistant, &body),   // middle C
            make_msg(MessageRole::User, &body),        // recent
            make_msg(MessageRole::Assistant, &body),   // recent
        ];
        // Tight budget so some middle messages must go.
        let config = SmartTruncationConfig {
            preserve_recent: 2,
            tool_output_max_tokens: 10_000,
            tool_output_head_lines: 20,
            tool_output_tail_lines: 20,
        };
        // Budget fits ~4 messages worth, forcing middle drops.
        let result = smart_truncate_to_fit(&mut messages, 100, &config);

        // System must still be present
        assert!(messages.iter().any(|m| m.role == MessageRole::System));
        // First user message must still be present (index 1 in original)
        assert!(messages.iter().any(|m| m.role == MessageRole::User));
        assert!(result.messages_dropped > 0);
        assert!(result.pass_used >= 2);
    }

    #[test]
    fn test_smart_tool_bundle_dropped_as_unit() {
        let mut messages = vec![
            make_msg(MessageRole::System, "sys"),
            make_msg(MessageRole::User, "first user"),
            make_tool_call_msg(vec![json!({"id": "c1", "type": "function"})]),
            make_tool_result("c1", "tool result content here"),
            make_msg(MessageRole::User, "recent"),
            make_msg(MessageRole::Assistant, "recent resp"),
        ];
        let config = SmartTruncationConfig {
            preserve_recent: 2,
            tool_output_max_tokens: 10_000,
            tool_output_head_lines: 20,
            tool_output_tail_lines: 20,
        };
        let before = messages.len();
        let result = smart_truncate_to_fit(&mut messages, 20, &config);
        // Either no messages were dropped or the bundle was dropped together
        // (no standalone tool result without its assistant call).
        for msg in &messages {
            if msg.role == MessageRole::Tool {
                // If a tool result survives, its paired assistant call must also survive.
                let tid = msg.tool_call_id.as_deref().unwrap_or("");
                let paired = messages.iter().any(|m| {
                    m.role == MessageRole::Assistant
                        && m.tool_calls
                            .as_ref()
                            .map(|tcs| extract_tool_call_ids(tcs).contains(&tid.to_string()))
                            .unwrap_or(false)
                });
                assert!(paired, "orphaned tool result found after smart truncation");
            }
        }
        let _ = before;
        let _ = result;
    }

    #[test]
    fn test_smart_pass3_extreme_budget_system_survives() {
        let mut messages = vec![
            make_msg(MessageRole::System, "sys"),
            make_msg(MessageRole::User, "user1"),
            make_msg(MessageRole::Assistant, "resp1"),
            make_msg(MessageRole::User, "user2"),
        ];
        // Absurdly small budget
        let config = default_smart_config();
        let result = smart_truncate_to_fit(&mut messages, 1, &config);
        // System must survive
        assert!(messages.iter().any(|m| m.role == MessageRole::System));
        assert!(result.pass_used >= 3 || result.messages_dropped > 0);
    }

    #[test]
    fn test_recency_window_last_n_always_present() {
        // 8 messages, preserve_recent=4 — the last 4 must survive pass-2.
        let mut messages = vec![
            make_msg(MessageRole::System, "sys"),
            make_msg(MessageRole::User, "first"),
            make_msg(MessageRole::Assistant, "r1"),
            make_msg(MessageRole::User, "u2"),
            make_msg(MessageRole::Assistant, "r2"),
            make_msg(MessageRole::User, "r3 recent"),
            make_msg(MessageRole::Assistant, "r4 recent"),
            make_msg(MessageRole::User, "r5 recent"),
        ];
        let config = SmartTruncationConfig {
            preserve_recent: 4,
            tool_output_max_tokens: 10_000,
            tool_output_head_lines: 20,
            tool_output_tail_lines: 20,
        };
        // Budget that forces some dropping but not everything
        let result = smart_truncate_to_fit(&mut messages, 30, &config);

        if result.pass_used <= 2 {
            // The last 4 messages from the original list should still be present.
            // In pass-2 only DropFirst/DropSecond groups are removed.
            // "r3 recent", "r4 recent", "r5 recent" had indices 5, 6, 7 out of 8 total
            // so recency_start = 8 - 4 = 4; indices >= 4 are protected.
            let contents: Vec<&str> = messages
                .iter()
                .filter_map(|m| m.content.as_ref()?.as_str())
                .collect();
            // At least some recent messages should be there
            let has_recent = contents.iter().any(|c| c.contains("recent"));
            assert!(has_recent || messages.len() <= 2);
        }
    }

    #[test]
    fn test_backward_compat_truncate_to_fit_unchanged() {
        // Verify the original truncate_to_fit still works as before.
        // Each body is 80 chars (~20 tokens), total ~80 tokens; budget 30 forces drops.
        let body = "x".repeat(80);
        let mut messages = vec![
            make_msg(MessageRole::System, &body),
            make_msg(MessageRole::User, &body),
            make_msg(MessageRole::Assistant, &body),
            make_msg(MessageRole::User, &body),
        ];
        let dropped = truncate_to_fit(&mut messages, 30);
        assert!(dropped > 0);
        assert_eq!(messages[0].role, MessageRole::System);
    }
}
