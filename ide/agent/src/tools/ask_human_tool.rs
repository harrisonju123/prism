use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput, ToolPermissionContext};

/// Asks the human user a question and waits for their response.
/// Use when you need clarification, approval, or input before proceeding.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AskHumanToolInput {
    /// The question to ask
    pub question: String,
}

pub struct AskHumanTool;

impl AgentTool for AskHumanTool {
    type Input = AskHumanToolInput;
    type Output = String;

    const NAME: &'static str = "ask_human";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Ask human".into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        cx.spawn(async move |cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive tool input: {e}"))?;

            let authorize = cx.update(|cx| {
                let context =
                    ToolPermissionContext::new(Self::NAME, vec![input.question.clone()]);
                event_stream.authorize(input.question.clone(), context, cx)
            });

            authorize
                .await
                .map_err(|e: anyhow::Error| e.to_string())?;

            Ok("approved".to_string())
        })
    }
}
