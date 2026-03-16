use std::sync::{Arc, Mutex};

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::task_store::TaskStore;

/// Get details for a specific task by ID.
///
/// <example>
/// { "id": "abc12345" }
/// </example>
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskGetToolInput {
    /// Task ID or prefix (at least 4 characters).
    pub id: String,
}

pub struct TaskGetTool {
    store: Arc<Mutex<TaskStore>>,
}

impl TaskGetTool {
    pub fn new(store: Arc<Mutex<TaskStore>>) -> Self {
        Self { store }
    }
}

impl AgentTool for TaskGetTool {
    type Input = TaskGetToolInput;
    type Output = String;

    const NAME: &'static str = "task_get";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        if let Ok(input) = input {
            format!("Get task [{}]", &input.id[..input.id.len().min(8)]).into()
        } else {
            "Get task".into()
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

            let guard = store
                .lock()
                .map_err(|e| format!("Task store lock poisoned: {e}"))?;

            let entry = guard
                .get(&input.id)
                .ok_or_else(|| format!("Task '{}' not found", input.id))?;

            Ok(entry.format())
        })
    }
}
