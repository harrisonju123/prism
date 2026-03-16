use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Sends a message to another agent in the workspace. The recipient will see
/// it in their inbox on the next poll cycle.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SendMessageToolInput {
    /// Name of the recipient agent (e.g. "claude-zed-surface")
    pub to: String,
    /// Message content
    pub content: String,
}

pub struct SendMessageTool {
    context: Arc<ContextHandle>,
}

impl SendMessageTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for SendMessageTool {
    type Input = SendMessageToolInput;
    type Output = String;

    const NAME: &'static str = "send_message";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Send message".into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        let context = self.context.clone();
        cx.spawn(async move |cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive tool input: {e}"))?;

            let to = input.to.clone();
            let content = input.content.clone();

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    context
                        .send_message(&to, &content, None)
                        .await
                        .map(|_| ())
                        .map_err(|e| anyhow::anyhow!("send_message failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())?;

            Ok(format!("Message sent to {}", input.to))
        })
    }
}
