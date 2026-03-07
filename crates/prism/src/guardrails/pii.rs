use regex::Regex;

use crate::error::{PrismError, Result};
use crate::types::{ChatCompletionRequest, ChatCompletionResponse};

use super::Guardrail;

/// Detects and blocks requests/responses containing PII patterns.
pub struct PiiGuardrail {
    patterns: Vec<PiiPattern>,
}

struct PiiPattern {
    name: &'static str,
    regex: Regex,
}

impl PiiGuardrail {
    pub fn new() -> Self {
        Self {
            patterns: vec![
                PiiPattern {
                    name: "SSN",
                    regex: Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap(),
                },
                PiiPattern {
                    name: "credit_card",
                    regex: Regex::new(r"\b\d{4}[- ]?\d{4}[- ]?\d{4}[- ]?\d{4}\b").unwrap(),
                },
                PiiPattern {
                    name: "email",
                    regex: Regex::new(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b")
                        .unwrap(),
                },
                PiiPattern {
                    name: "phone_us",
                    regex: Regex::new(r"\b(?:\+1[- ]?)?\(?\d{3}\)?[- ]?\d{3}[- ]?\d{4}\b").unwrap(),
                },
            ],
        }
    }

    fn check_text(&self, text: &str) -> Option<&'static str> {
        for pattern in &self.patterns {
            if pattern.regex.is_match(text) {
                return Some(pattern.name);
            }
        }
        None
    }
}

impl Guardrail for PiiGuardrail {
    fn name(&self) -> &str {
        "pii"
    }

    fn check_request(&self, request: &ChatCompletionRequest) -> Result<()> {
        for msg in &request.messages {
            if let Some(content) = &msg.content {
                if let Some(text) = content.as_str() {
                    if let Some(pii_type) = self.check_text(text) {
                        return Err(PrismError::ContentFiltered(format!(
                            "request blocked: detected {pii_type} in message content"
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
                    if let Some(pii_type) = self.check_text(text) {
                        return Err(PrismError::ContentFiltered(format!(
                            "response blocked: detected {pii_type} in completion"
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
    fn detects_ssn() {
        let guard = PiiGuardrail::new();
        assert_eq!(guard.check_text("My SSN is 123-45-6789"), Some("SSN"));
    }

    #[test]
    fn detects_credit_card() {
        let guard = PiiGuardrail::new();
        assert_eq!(
            guard.check_text("Card: 4111-1111-1111-1111"),
            Some("credit_card")
        );
    }

    #[test]
    fn detects_email() {
        let guard = PiiGuardrail::new();
        assert_eq!(
            guard.check_text("Email me at john@example.com"),
            Some("email")
        );
    }

    #[test]
    fn no_pii_passes() {
        let guard = PiiGuardrail::new();
        assert_eq!(guard.check_text("Hello, how are you?"), None);
    }
}
