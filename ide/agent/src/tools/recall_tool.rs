use std::sync::Arc;

use agent_client_protocol as acp;
use chrono::{DateTime, Utc};
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::store::Store as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Recalls memories, decisions, and activity from the project's context store.
/// Provide at least one of: thread name, tags, or since duration.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RecallToolInput {
    /// Thread name to recall context for (e.g. "auth-refactor")
    #[serde(default)]
    pub thread: Option<String>,
    /// Tags to filter by
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Duration string for recency filter (e.g. "2h", "30m", "1d")
    #[serde(default)]
    pub since: Option<String>,
}

pub struct RecallTool {
    context: Arc<ContextHandle>,
}

impl RecallTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for RecallTool {
    type Input = RecallToolInput;
    type Output = String;

    const NAME: &'static str = "recall";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Recall context".into()
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

            if input.thread.is_none() && input.tags.is_none() && input.since.is_none() {
                return Err(
                    "At least one of 'thread', 'tags', or 'since' must be provided".to_string(),
                );
            }

            let ws_id = context.workspace_id;
            let thread_name = input.thread.clone();
            let tags = input.tags.clone().unwrap_or_default();
            let since: Option<DateTime<Utc>> = input.since.as_deref().and_then(|s| {
                prism_context::util::parse_duration(s).ok().map(|d| Utc::now() - d)
            });

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    if let Some(thread_name) = thread_name {
                        let ctx = context
                            .store
                            .recall_thread(ws_id, &thread_name)
                            .await
                            .map_err(|e| anyhow::anyhow!("recall_thread failed: {e}"))?;
                        serde_json::to_string_pretty(&ctx)
                            .map_err(|e| anyhow::anyhow!("serialize failed: {e}"))
                    } else {
                        let result = context
                            .store
                            .recall_by_tags(ws_id, tags, since)
                            .await
                            .map_err(|e| anyhow::anyhow!("recall_by_tags failed: {e}"))?;
                        serde_json::to_string_pretty(&result)
                            .map_err(|e| anyhow::anyhow!("serialize failed: {e}"))
                    }
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())
        })
    }
}
