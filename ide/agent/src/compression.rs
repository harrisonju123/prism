use crate::thread::{Message, UserMessage, UserMessageContent};
use acp_thread::UserMessageId;

/// FIFO-based context compressor for trimming conversation history.
pub struct ContextCompressor {
    /// Trigger compression when message count / max >= this ratio (default 0.7)
    pub threshold_ratio: f64,
    /// Number of recent messages to preserve verbatim (default 20)
    pub preserve_recent: usize,
}

impl Default for ContextCompressor {
    fn default() -> Self {
        Self {
            threshold_ratio: 0.7,
            preserve_recent: 20,
        }
    }
}

impl ContextCompressor {
    /// Returns true if compression should be triggered.
    pub fn should_compress(&self, msg_count: usize, max: usize) -> bool {
        if max == 0 {
            return false;
        }
        msg_count as f64 >= max as f64 * self.threshold_ratio
    }

    /// FIFO trim: keep first message + most recent `preserve_recent` messages.
    /// Inserts a summary placeholder at index 1 if any messages were removed.
    pub fn trim_fifo(&self, messages: &mut Vec<Message>) {
        let keep = self.preserve_recent;
        if messages.len() <= keep + 1 {
            return;
        }
        // Remove messages from index 1 up to (len - keep), leaving first + last `keep`
        let remove_end = messages.len().saturating_sub(keep);
        if remove_end <= 1 {
            return;
        }
        let removed_count = remove_end - 1;
        messages.drain(1..remove_end);
        // Insert a placeholder so the model knows context was trimmed
        messages.insert(
            1,
            Message::User(UserMessage {
                id: UserMessageId::new(),
                content: vec![UserMessageContent::Text(format!(
                    "[{removed_count} earlier messages trimmed to save context window]"
                ))],
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_msg(text: &str) -> Message {
        Message::User(UserMessage {
            id: UserMessageId::new(),
            content: vec![UserMessageContent::Text(text.to_string())],
        })
    }

    #[test]
    fn should_compress_below_threshold() {
        let c = ContextCompressor::default();
        assert!(!c.should_compress(50, 100));
        assert!(!c.should_compress(69, 100));
    }

    #[test]
    fn should_compress_at_threshold() {
        let c = ContextCompressor::default();
        assert!(c.should_compress(70, 100));
        assert!(c.should_compress(100, 100));
    }

    #[test]
    fn should_compress_zero_max_noop() {
        let c = ContextCompressor::default();
        assert!(!c.should_compress(50, 0));
    }

    #[test]
    fn trim_fifo_within_limit() {
        let mut c = ContextCompressor {
            threshold_ratio: 0.7,
            preserve_recent: 20,
        };
        let mut msgs: Vec<Message> = (0..5).map(|i| user_msg(&format!("msg-{i}"))).collect();
        c.trim_fifo(&mut msgs);
        assert_eq!(msgs.len(), 5);
    }

    #[test]
    fn trim_fifo_removes_old_messages() {
        let c = ContextCompressor {
            threshold_ratio: 0.7,
            preserve_recent: 3,
        };
        let mut msgs: Vec<Message> = (0..10).map(|i| user_msg(&format!("msg-{i}"))).collect();
        c.trim_fifo(&mut msgs);
        // Should have: placeholder + 3 recent + first = original[0], placeholder, original[7], original[8], original[9]
        // Actually: first stays, placeholder inserted at 1, then last 3 = 5 total
        assert_eq!(msgs.len(), 5);
        // First message unchanged
        if let Message::User(m) = &msgs[0] {
            let text = m.content.iter().find_map(|c| match c {
                UserMessageContent::Text(t) => Some(t.clone()),
                _ => None,
            });
            assert_eq!(text.as_deref(), Some("msg-0"));
        }
        // Placeholder at index 1
        if let Message::User(m) = &msgs[1] {
            let text = m.content.iter().find_map(|c| match c {
                UserMessageContent::Text(t) => Some(t.clone()),
                _ => None,
            });
            assert!(text.unwrap().contains("trimmed"));
        }
    }
}
