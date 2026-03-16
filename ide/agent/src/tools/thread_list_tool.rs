use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::model::ThreadStatus;
use prism_context::store::Store as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Lists context threads in the workspace. Optionally filter by status.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ThreadListToolInput {
    /// Filter by status: "active", "archived", or omit for all
    #[serde(default)]
    pub status: Option<String>,
}

pub struct ThreadListTool {
    context: Arc<ContextHandle>,
}

impl ThreadListTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for ThreadListTool {
    type Input = ThreadListToolInput;
    type Output = String;

    const NAME: &'static str = "thread_list";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "List threads".into()
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
            let status = match input.status.as_deref() {
                Some("active") => Some(ThreadStatus::Active),
                Some("archived") => Some(ThreadStatus::Archived),
                _ => None,
            };

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    let threads = context
                        .store
                        .list_threads(ws_id, status)
                        .await
                        .map_err(|e| anyhow::anyhow!("list_threads failed: {e}"))?;
                    serde_json::to_string_pretty(&threads)
                        .map_err(|e| anyhow::anyhow!("serialize failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())
        })
    }
}
