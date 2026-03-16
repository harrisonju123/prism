use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Archives a context thread, marking it as done. Archived threads are excluded
/// from active thread lists but their memories and decisions are preserved.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ThreadArchiveToolInput {
    /// Name of the thread to archive (e.g. "auth-refactor")
    pub name: String,
}

pub struct ThreadArchiveTool {
    context: Arc<ContextHandle>,
}

impl ThreadArchiveTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for ThreadArchiveTool {
    type Input = ThreadArchiveToolInput;
    type Output = String;

    const NAME: &'static str = "thread_archive";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Archive thread".into()
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

            let name = input.name.clone();

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    let thread = context
                        .archive_thread(&name)
                        .await
                        .map_err(|e| anyhow::anyhow!("archive_thread failed: {e}"))?;
                    serde_json::to_string_pretty(&thread)
                        .map_err(|e| anyhow::anyhow!("serialize failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())
        })
    }
}
