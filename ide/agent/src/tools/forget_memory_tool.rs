use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::store::Store as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Deletes a memory from the project's context store by key.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ForgetMemoryToolInput {
    /// The key of the memory to delete (e.g. "auth_approach")
    pub key: String,
}

pub struct ForgetMemoryTool {
    context: Arc<ContextHandle>,
}

impl ForgetMemoryTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for ForgetMemoryTool {
    type Input = ForgetMemoryToolInput;
    type Output = String;

    const NAME: &'static str = "forget_memory";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Forget memory".into()
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
            let key = input.key.clone();

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    context
                        .store
                        .delete_memory(ws_id, &key)
                        .await
                        .map_err(|e| anyhow::anyhow!("delete_memory failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())?;

            Ok(format!("Deleted memory: {}", input.key))
        })
    }
}
