pub mod keywords;
pub mod pii;
pub mod policy;

use crate::error::Result;
use crate::types::{ChatCompletionRequest, ChatCompletionResponse};

/// Trait for content guardrails that can inspect requests and responses.
pub trait Guardrail: Send + Sync {
    fn name(&self) -> &str;
    fn check_request(&self, request: &ChatCompletionRequest) -> Result<()>;
    fn check_response(&self, response: &ChatCompletionResponse) -> Result<()>;
}
