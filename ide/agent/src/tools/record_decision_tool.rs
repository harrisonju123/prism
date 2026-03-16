use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::model::DecisionScope;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

fn default_scope() -> String {
    "thread".to_string()
}

/// Records an architectural or design decision with its rationale.
/// Future sessions can understand why this path was chosen.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RecordDecisionToolInput {
    /// Short title (e.g. "Use JWT over sessions")
    pub title: String,
    /// Full rationale
    pub content: String,
    /// Optional tags
    #[serde(default)]
    pub tags: Vec<String>,
    /// Scope: "thread" (default) or "workspace" (notifies all agents)
    #[serde(default = "default_scope")]
    pub scope: String,
}

pub struct RecordDecisionTool {
    context: Arc<ContextHandle>,
}

impl RecordDecisionTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for RecordDecisionTool {
    type Input = RecordDecisionToolInput;
    type Output = String;

    const NAME: &'static str = "record_decision";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Record decision".into()
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

            let scope = match input.scope.as_str() {
                "workspace" => DecisionScope::Workspace,
                _ => DecisionScope::Thread,
            };
            let thread_id = if matches!(scope, DecisionScope::Workspace) {
                None
            } else {
                context.context_thread.read().as_ref().map(|t| t.id)
            };
            let title = input.title.clone();
            let content = input.content.clone();
            let tags = input.tags.clone();

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    context
                        .save_decision(&title, &content, thread_id, tags, scope)
                        .await
                        .map(|_| ())
                        .map_err(|e| anyhow::anyhow!("save_decision failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())?;

            Ok(format!("Decision recorded: {}", input.title))
        })
    }
}
