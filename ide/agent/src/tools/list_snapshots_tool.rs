use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Lists workspace snapshots in reverse chronological order.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListSnapshotsToolInput {
    /// Maximum number of snapshots to return (default: 20)
    #[serde(default)]
    pub limit: Option<i64>,
}

pub struct ListSnapshotsTool {
    context: Arc<ContextHandle>,
}

impl ListSnapshotsTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for ListSnapshotsTool {
    type Input = ListSnapshotsToolInput;
    type Output = String;

    const NAME: &'static str = "list_snapshots";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "List snapshots".into()
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

            let limit = input.limit;

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    let snapshots = context
                        .list_snapshots(limit)
                        .await
                        .map_err(|e| anyhow::anyhow!("list_snapshots failed: {e}"))?;
                    serde_json::to_string_pretty(&snapshots)
                        .map_err(|e| anyhow::anyhow!("serialize failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())
        })
    }
}
