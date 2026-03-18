use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput, ToolPermissionContext};

/// Escalates a decision to the human user when a worker agent needs approval before proceeding.
/// Use when a subagent encounters a significant choice that requires human judgment.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EscalateDecisionToolInput {
    /// The worker agent escalating the decision (e.g. "Worker-B")
    pub worker: String,
    /// A concise summary of what decision is needed
    pub summary: String,
    /// Additional context about why this decision matters
    pub context: Option<String>,
    /// Informational labels describing the available options
    #[serde(default)]
    pub options: Vec<String>,
}

pub struct EscalateDecisionTool;

impl AgentTool for EscalateDecisionTool {
    type Input = EscalateDecisionToolInput;
    type Output = String;

    const NAME: &'static str = "escalate_decision";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Escalate decision".into()
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
                let context = ToolPermissionContext::new(
                    Self::NAME,
                    vec![input.summary.clone()],
                );
                event_stream.authorize(input.summary.clone(), context, cx)
            });

            authorize
                .await
                .map_err(|e: anyhow::Error| e.to_string())?;

            Ok("acknowledged".to_string())
        })
    }
}
