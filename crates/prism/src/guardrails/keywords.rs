use crate::error::{PrismError, Result};
use crate::types::{ChatCompletionRequest, ChatCompletionResponse};

use super::Guardrail;

/// Blocks requests/responses containing specific keywords.
pub struct KeywordGuardrail {
    blocked_words: Vec<String>,
}

impl KeywordGuardrail {
    pub fn new(blocked_words: Vec<String>) -> Self {
        Self {
            blocked_words: blocked_words
                .into_iter()
                .map(|w| w.to_lowercase())
                .collect(),
        }
    }

    fn check_text(&self, text: &str) -> Option<&str> {
        let lower = text.to_lowercase();
        for word in &self.blocked_words {
            if lower.contains(word.as_str()) {
                return Some(word);
            }
        }
        None
    }
}

impl Guardrail for KeywordGuardrail {
    fn name(&self) -> &str {
        "keywords"
    }

    fn check_request(&self, request: &ChatCompletionRequest) -> Result<()> {
        for msg in &request.messages {
            if let Some(content) = &msg.content {
                if let Some(text) = content.as_str() {
                    if let Some(word) = self.check_text(text) {
                        return Err(PrismError::ContentFiltered(format!(
                            "request blocked: contains blocked keyword '{word}'"
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    fn check_response(&self, response: &ChatCompletionResponse) -> Result<()> {
        for choice in &response.choices {
            if let Some(content) = &choice.message.content {
                if let Some(text) = content.as_str() {
                    if let Some(word) = self.check_text(text) {
                        return Err(PrismError::ContentFiltered(format!(
                            "response blocked: contains blocked keyword '{word}'"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_keyword() {
        let guard = KeywordGuardrail::new(vec!["forbidden".into(), "secret".into()]);
        assert_eq!(
            guard.check_text("This is forbidden content"),
            Some("forbidden")
        );
    }

    #[test]
    fn case_insensitive() {
        let guard = KeywordGuardrail::new(vec!["danger".into()]);
        assert_eq!(guard.check_text("DANGER zone"), Some("danger"));
    }

    #[test]
    fn passes_clean_text() {
        let guard = KeywordGuardrail::new(vec!["blocked".into()]);
        assert!(guard.check_text("This is fine").is_none());
    }
}
