use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::store::Store as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Captures a point-in-time snapshot of the workspace state (threads, memories,
/// decisions, agents) as a JSON blob for later reference.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateSnapshotToolInput {
    /// Short description of this snapshot (e.g. "before auth refactor")
    pub label: String,
}

pub struct CreateSnapshotTool {
    context: Arc<ContextHandle>,
}

impl CreateSnapshotTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for CreateSnapshotTool {
    type Input = CreateSnapshotToolInput;
    type Output = String;

    const NAME: &'static str = "create_snapshot";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Create snapshot".into()
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
            let label = input.label.clone();

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    context
                        .store
                        .create_snapshot(ws_id, &label)
                        .await
                        .map(|s| format!("Snapshot created: {} ({})", s.label, s.summary))
                        .map_err(|e| anyhow::anyhow!("create_snapshot failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())
        })
    }
}
