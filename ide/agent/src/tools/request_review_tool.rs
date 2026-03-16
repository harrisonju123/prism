use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::model::{InboxEntryType, InboxSeverity};
use prism_context::store::Store as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::{AGENT_SOURCE, ContextHandle};

fn default_info() -> String {
    "info".to_string()
}

/// Flags something for human review without blocking. Fire-and-forget.
/// Creates an inbox entry visible in PrisM HQ panels.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RequestReviewToolInput {
    /// What needs review
    pub title: String,
    /// Context and details
    pub body: String,
    /// "critical", "warning", or "info" (default: "info")
    #[serde(default = "default_info")]
    pub severity: String,
}

pub struct RequestReviewTool {
    context: Arc<ContextHandle>,
}

impl RequestReviewTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

fn parse_severity(s: &str) -> InboxSeverity {
    InboxSeverity::from_str(s).unwrap_or(InboxSeverity::Info)
}

impl AgentTool for RequestReviewTool {
    type Input = RequestReviewToolInput;
    type Output = String;

    const NAME: &'static str = "request_review";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Request review".into()
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

            let ws_id = context.workspace_id;
            let title = input.title.clone();
            let body = input.body.clone();
            let severity = parse_severity(&input.severity);

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    context
                        .store
                        .create_inbox_entry(
                            ws_id,
                            InboxEntryType::Suggestion,
                            &title,
                            &body,
                            severity,
                            Some(AGENT_SOURCE),
                            None,
                            None,
                        )
                        .await
                        .map_err(|e| anyhow::anyhow!("create_inbox_entry failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())?;

            Ok(format!("Review request filed: {}", input.title))
        })
    }
}
