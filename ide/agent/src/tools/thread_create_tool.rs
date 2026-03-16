use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Creates a new named context thread (a bucket for grouping related memories,
/// decisions, and sessions). Use to organize work into logical streams.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ThreadCreateToolInput {
    /// Thread name (e.g. "auth-refactor", "api-v2")
    pub name: String,
    /// Short description of the thread's purpose
    #[serde(default)]
    pub description: String,
    /// Optional tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
}

pub struct ThreadCreateTool {
    context: Arc<ContextHandle>,
}

impl ThreadCreateTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for ThreadCreateTool {
    type Input = ThreadCreateToolInput;
    type Output = String;

    const NAME: &'static str = "thread_create";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Create thread".into()
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
            let desc = input.description.clone();
            let tags = input.tags.clone();

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    let thread = context
                        .create_thread(&name, &desc, tags)
                        .await
                        .map_err(|e| anyhow::anyhow!("create_thread failed: {e}"))?;
                    serde_json::to_string_pretty(&thread)
                        .map_err(|e| anyhow::anyhow!("serialize failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())
        })
    }
}
