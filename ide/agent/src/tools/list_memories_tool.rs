use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::store::MemoryFilters;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Lists memories from the project's context store, optionally filtered
/// by thread name or tags.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListMemoriesToolInput {
    /// Filter by thread name (e.g. "auth-refactor")
    #[serde(default)]
    pub thread: Option<String>,
    /// Filter by tags
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

pub struct ListMemoriesTool {
    context: Arc<ContextHandle>,
}

impl ListMemoriesTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for ListMemoriesTool {
    type Input = ListMemoriesToolInput;
    type Output = String;

    const NAME: &'static str = "list_memories";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "List memories".into()
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

            let thread_name = input.thread.clone();
            let tags = input.tags.clone();

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    let filters = MemoryFilters {
                        thread_name,
                        tags,
                        ..Default::default()
                    };
                    let memories = context
                        .load_memories(filters)
                        .await
                        .map_err(|e| anyhow::anyhow!("load_memories failed: {e}"))?;
                    serde_json::to_string_pretty(&memories)
                        .map_err(|e| anyhow::anyhow!("serialize failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())
        })
    }
}
