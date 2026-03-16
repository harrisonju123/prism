use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::store::Store as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::{AGENT_SOURCE, ContextHandle};

/// Saves a persistent memory (key-value fact) to the project's context store.
/// Use when you learn something important that should persist across sessions:
/// architecture decisions, user preferences, project conventions.
/// Memories are keyed — saving with an existing key updates the value.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SaveMemoryToolInput {
    /// A short unique key (e.g. "auth_approach", "db_schema")
    pub key: String,
    /// The content to remember
    pub value: String,
    /// Optional tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
}

pub struct SaveMemoryTool {
    context: Arc<ContextHandle>,
}

impl SaveMemoryTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for SaveMemoryTool {
    type Input = SaveMemoryToolInput;
    type Output = String;

    const NAME: &'static str = "save_memory";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Save memory".into()
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

            let thread_id = context.context_thread.read().as_ref().map(|t| t.id);
            let ws_id = context.workspace_id;
            let key = input.key.clone();
            let value = input.value.clone();
            let tags = input.tags.clone();

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    context
                        .store
                        .save_memory(ws_id, &key, &value, thread_id, AGENT_SOURCE, tags)
                        .await
                        .map_err(|e| anyhow::anyhow!("save_memory failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())?;

            Ok(format!("Saved memory: {}", input.key))
        })
    }
}
