use crate::common::truncate_with_ellipsis;
use prism_client::PrismClient;
use prism_types::{ChatCompletionRequest, Message};
use serde_json::json;

/// Max total chars for the transcript sent to the summarization model.
const MAX_TRANSCRIPT_CHARS: usize = 50_000;

pub struct ContextCompressor {
    /// Model to use for summarization (e.g. "gpt-4o-mini")
    summary_model: String,
    /// Trigger compression at this fraction of max messages (default 0.7)
    threshold_ratio: f64,
    /// Number of recent messages to preserve verbatim (default 20)
    preserve_recent: usize,
}

impl ContextCompressor {
    pub fn new(summary_model: String, threshold_ratio: f64, preserve_recent: usize) -> Self {
        Self {
            summary_model,
            threshold_ratio,
            preserve_recent,
        }
    }

    pub fn should_compress(&self, count: usize, max: usize) -> bool {
        if max == 0 {
            return false;
        }
        count as f64 >= max as f64 * self.threshold_ratio
    }

    /// Compress old messages into a summary, keeping recent ones intact.
    /// Returns the new message list, or None on failure.
    pub async fn compress(
        &self,
        client: &PrismClient,
        messages: &[Message],
        max: usize,
    ) -> Option<Vec<Message>> {
        if messages.len() < 3 {
            return None;
        }

        // messages[0] = system, messages[1..] = conversation
        let system_msg = messages[0].clone();
        let conversation = &messages[1..];

        // Keep at least preserve_recent messages from the end
        let keep = self.preserve_recent.min(conversation.len());
        let split = conversation.len().saturating_sub(keep);

        if split == 0 {
            return None;
        }

        let old_messages = &conversation[..split];
        let recent_messages = &conversation[split..];

        let summary_text = self.summarize(client, old_messages).await?;

        let mut result = Vec::with_capacity(3 + recent_messages.len());
        result.push(system_msg);
        result.push(Message {
            role: "user".into(),
            content: Some(json!(format!(
                "[Context compressed — summary of {} earlier messages]\n\n{}",
                old_messages.len(),
                summary_text
            ))),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        });
        result.push(Message {
            role: "assistant".into(),
            content: Some(json!(
                "Understood. I have the context from the earlier conversation summary and will continue from here."
            )),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        });
        result.extend_from_slice(recent_messages);

        // Ensure we didn't accidentally grow — apply FIFO as safety net
        if result.len() > max && max > 0 {
            trim_messages_fifo(&mut result, max);
        }

        Some(result)
    }

    async fn summarize(&self, client: &PrismClient, messages: &[Message]) -> Option<String> {
        let mut transcript = String::new();
        for msg in messages {
            let role = &msg.role;
            let content = msg
                .content
                .as_ref()
                .and_then(|v| v.as_str())
                .unwrap_or("[no content]");

            let truncated = truncate_with_ellipsis(content, 2000);

            use std::fmt::Write;
            let _ = write!(transcript, "{role}: {truncated}\n\n");

            if transcript.len() >= MAX_TRANSCRIPT_CHARS {
                transcript.truncate(MAX_TRANSCRIPT_CHARS);
                transcript.push_str("\n[... transcript truncated]");
                break;
            }
        }

        let prompt = format!(
            "Summarize this conversation transcript concisely. Preserve:\n\
             - The task being worked on and current goal\n\
             - Key decisions made and their rationale\n\
             - Files modified and their current state\n\
             - Errors encountered and how they were resolved\n\
             - Any pending work or next steps\n\n\
             Transcript:\n{transcript}"
        );

        let req = ChatCompletionRequest {
            model: self.summary_model.clone(),
            messages: vec![Message {
                role: "user".into(),
                content: Some(json!(prompt)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            }],
            ..Default::default()
        };

        match client.chat_completion(&req).await {
            Ok(resp) => resp
                .choices
                .first()
                .and_then(|c| c.message.content.as_ref())
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            Err(e) => {
                tracing::warn!("compression summarization failed: {e}");
                None
            }
        }
    }
}

/// FIFO trim: keep system prompt (index 0) + most recent (max - 1) messages.
pub fn trim_messages_fifo(messages: &mut Vec<Message>, max: usize) {
    if max == 0 || messages.len() <= max {
        return;
    }
    let drain_end = messages.len() - (max - 1);
    messages.drain(1..drain_end);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(role: &str, content: &str) -> Message {
        Message {
            role: role.into(),
            content: Some(json!(content)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn should_compress_below_threshold() {
        let c = ContextCompressor::new("gpt-4o-mini".into(), 0.7, 20);
        assert!(!c.should_compress(50, 100));
        assert!(!c.should_compress(69, 100));
    }

    #[test]
    fn should_compress_at_threshold() {
        let c = ContextCompressor::new("gpt-4o-mini".into(), 0.7, 20);
        assert!(c.should_compress(70, 100));
        assert!(c.should_compress(100, 100));
    }

    #[test]
    fn should_compress_zero_max() {
        let c = ContextCompressor::new("gpt-4o-mini".into(), 0.7, 20);
        assert!(!c.should_compress(50, 0));
    }

    #[test]
    fn trim_messages_fifo_within_limit() {
        let mut msgs = vec![make_msg("system", "sys"), make_msg("user", "hi")];
        trim_messages_fifo(&mut msgs, 10);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn trim_messages_fifo_exceeds_limit() {
        let mut msgs: Vec<Message> = (0..20)
            .map(|i| make_msg("user", &format!("msg-{i}")))
            .collect();
        msgs[0] = make_msg("system", "sys");
        trim_messages_fifo(&mut msgs, 5);
        assert_eq!(msgs.len(), 5);
        assert_eq!(msgs[0].role, "system");
        // Should keep the last 4
        assert_eq!(
            msgs[4].content.as_ref().unwrap().as_str().unwrap(),
            "msg-19"
        );
    }

    #[test]
    fn trim_messages_fifo_zero_max_noop() {
        let mut msgs = vec![make_msg("system", "sys"), make_msg("user", "hi")];
        trim_messages_fifo(&mut msgs, 0);
        assert_eq!(msgs.len(), 2);
    }
}
