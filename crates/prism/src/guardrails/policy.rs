use crate::error::Result;
use crate::types::{ChatCompletionRequest, ChatCompletionResponse};

use super::Guardrail;
use super::keywords::KeywordGuardrail;
use super::pii::PiiGuardrail;

/// A policy that chains multiple guardrails.
pub struct GuardrailPolicy {
    guardrails: Vec<Box<dyn Guardrail>>,
}

impl GuardrailPolicy {
    pub fn new() -> Self {
        Self {
            guardrails: Vec::new(),
        }
    }

    pub fn with_pii(mut self) -> Self {
        self.guardrails.push(Box::new(PiiGuardrail::new()));
        self
    }

    pub fn with_keywords(mut self, blocked_words: Vec<String>) -> Self {
        if !blocked_words.is_empty() {
            self.guardrails
                .push(Box::new(KeywordGuardrail::new(blocked_words)));
        }
        self
    }

    /// Check request against all guardrails. Returns first error if any.
    pub fn check_request(&self, request: &ChatCompletionRequest) -> Result<()> {
        for guard in &self.guardrails {
            guard.check_request(request)?;
        }
        Ok(())
    }

    /// Check response against all guardrails. Returns first error if any.
    pub fn check_response(&self, response: &ChatCompletionResponse) -> Result<()> {
        for guard in &self.guardrails {
            guard.check_response(response)?;
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.guardrails.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChatCompletionRequest, Message};

    fn make_request(user_msg: &str) -> ChatCompletionRequest {
        ChatCompletionRequest {
            model: "test".into(),
            messages: vec![Message {
                role: "user".into(),
                content: Some(serde_json::Value::String(user_msg.into())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: serde_json::Map::new(),
            }],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: false,
            stream_options: None,
            stop: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            user: None,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn empty_policy_passes() {
        let policy = GuardrailPolicy::new();
        let request = make_request("Hello world");
        assert!(policy.check_request(&request).is_ok());
    }

    #[test]
    fn pii_blocks_ssn() {
        let policy = GuardrailPolicy::new().with_pii();
        let request = make_request("My SSN is 123-45-6789");
        assert!(policy.check_request(&request).is_err());
    }

    #[test]
    fn keyword_blocks() {
        let policy = GuardrailPolicy::new().with_keywords(vec!["banned".into()]);
        let request = make_request("This contains banned content");
        assert!(policy.check_request(&request).is_err());
    }

    #[test]
    fn clean_request_passes() {
        let policy = GuardrailPolicy::new()
            .with_pii()
            .with_keywords(vec!["forbidden".into()]);
        let request = make_request("Hello, how can I help you?");
        assert!(policy.check_request(&request).is_ok());
    }
}
