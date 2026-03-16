use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::store::Store as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Returns a workspace overview: active threads, recent memories, recent
/// decisions, active agents, and recent sessions.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ContextOverviewToolInput {}

pub struct ContextOverviewTool {
    context: Arc<ContextHandle>,
}

impl ContextOverviewTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for ContextOverviewTool {
    type Input = ContextOverviewToolInput;
    type Output = String;

    const NAME: &'static str = "context_overview";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Context overview".into()
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        let context = self.context.clone();
        cx.spawn(async move |cx| {
            input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive tool input: {e}"))?;

            let ws_id = context.workspace_id;

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    let overview = context
                        .store
                        .get_workspace_overview(ws_id)
                        .await
                        .map_err(|e| anyhow::anyhow!("get_workspace_overview failed: {e}"))?;
                    serde_json::to_string_pretty(&overview)
                        .map_err(|e| anyhow::anyhow!("serialize failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())
        })
    }
}
