use std::sync::{Arc, Mutex};

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::task_store::TaskStore;

/// Create a new task for tracking work items.
///
/// Tasks help organize multi-step work. Use them to break down complex goals into
/// trackable pieces and coordinate parallel or sequential work.
///
/// <example>
/// { "subject": "Implement authentication", "description": "Add JWT-based auth to the API" }
/// </example>
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskCreateToolInput {
    /// Short title for the task.
    pub subject: String,
    /// Optional detailed description of what this task involves.
    pub description: Option<String>,
}

pub struct TaskCreateTool {
    store: Arc<Mutex<TaskStore>>,
}

impl TaskCreateTool {
    pub fn new(store: Arc<Mutex<TaskStore>>) -> Self {
        Self { store }
    }
}

impl AgentTool for TaskCreateTool {
    type Input = TaskCreateToolInput;
    type Output = String;

    const NAME: &'static str = "task_create";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        if let Ok(input) = input {
            format!("Create task: {}", input.subject).into()
        } else {
            "Create task".into()
        }
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        let store = self.store.clone();
        cx.spawn(async move |_cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive tool input: {e}"))?;

            let entry = store
                .lock()
                .map_err(|e| format!("Task store lock poisoned: {e}"))?
                .create(input.subject, input.description);

            Ok(entry.format())
        })
    }
}
