use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use gpui_tokio::Tokio;
use prism_context::model::{InboxEntryType, InboxSeverity};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::context_handle::ContextHandle;

/// Submit a batch of clarification questions to the supervisory inbox.
/// Use during the clarify phase to ask multiple questions at once rather than
/// pausing for each one individually. The questions will be grouped into a single
/// inbox notification for the user to answer together.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SubmitQuestionsToolInput {
    /// List of questions to ask (will be presented as a numbered list)
    pub questions: Vec<String>,
    /// Optional context explaining why these questions are needed
    #[serde(default)]
    pub context: String,
}

pub struct SubmitQuestionsTool {
    context: Arc<ContextHandle>,
}

impl SubmitQuestionsTool {
    pub fn new(context: Arc<ContextHandle>) -> Self {
        Self { context }
    }
}

impl AgentTool for SubmitQuestionsTool {
    type Input = SubmitQuestionsToolInput;
    type Output = String;

    const NAME: &'static str = "submit_questions";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        _input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        "Submit questions".into()
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

            if input.questions.is_empty() {
                return Err("questions list is empty".to_string());
            }

            let count = input.questions.len();
            let mut body = if !input.context.is_empty() {
                format!("{}\n\n", input.context)
            } else {
                String::new()
            };
            for (i, q) in input.questions.iter().enumerate() {
                body.push_str(&format!("{}. {}\n", i + 1, q));
            }

            let title = format!("{count} clarification question{}", if count == 1 { "" } else { "s" });

            cx.update(|cx| {
                Tokio::spawn_result(cx, async move {
                    context
                        .create_inbox_entry(
                            InboxEntryType::Blocked,
                            &title,
                            &body,
                            InboxSeverity::Warning,
                            None,
                            None,
                        )
                        .await
                        .map(|_| ())
                        .map_err(|e| anyhow::anyhow!("submit_questions failed: {e}"))
                })
            })
            .await
            .map_err(|e: anyhow::Error| e.to_string())?;

            Ok(format!(
                "{count} question{} submitted to inbox. Stop and await user response before continuing.",
                if count == 1 { "" } else { "s" }
            ))
        })
    }
}
