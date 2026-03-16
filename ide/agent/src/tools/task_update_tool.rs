use std::sync::{Arc, Mutex};

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::task_store::{TaskStatus, TaskStore};

/// Update a task's status, description, or dependency relationships.
///
/// <example>
/// { "id": "abc12345", "status": "in_progress" }
/// { "id": "abc12345", "add_blocks": ["def67890"], "status": "blocked" }
/// </example>
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskUpdateToolInput {
    /// Task ID or prefix (at least 4 characters) to identify the task.
    pub id: String,
    /// New status for the task.
    pub status: Option<TaskStatus>,
    /// New description (replaces existing description).
    pub description: Option<String>,
    /// IDs of tasks that this task now blocks.
    pub add_blocks: Option<Vec<String>>,
    /// IDs of tasks that this task is now blocked by.
    pub add_blocked_by: Option<Vec<String>>,
}

pub struct TaskUpdateTool {
    store: Arc<Mutex<TaskStore>>,
}

impl TaskUpdateTool {
    pub fn new(store: Arc<Mutex<TaskStore>>) -> Self {
        Self { store }
    }
}

impl AgentTool for TaskUpdateTool {
    type Input = TaskUpdateToolInput;
    type Output = String;

    const NAME: &'static str = "task_update";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        if let Ok(input) = input {
            if let Some(status) = &input.status {
                format!("Update task [{}]: → {}", &input.id[..input.id.len().min(8)], status).into()
            } else {
                format!("Update task [{}]", &input.id[..input.id.len().min(8)]).into()
            }
        } else {
            "Update task".into()
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

            let mut guard = store
                .lock()
                .map_err(|e| format!("Task store lock poisoned: {e}"))?;

            let entry = guard
                .get_mut(&input.id)
                .ok_or_else(|| format!("Task '{}' not found", input.id))?;

            if let Some(status) = input.status {
                entry.status = status;
            }
            if let Some(description) = input.description {
                entry.description = Some(description);
            }
            if let Some(blocks) = input.add_blocks {
                for id in blocks {
                    if !entry.blocks.contains(&id) {
                        entry.blocks.push(id);
                    }
                }
            }
            if let Some(blocked_by) = input.add_blocked_by {
                for id in blocked_by {
                    if !entry.blocked_by.contains(&id) {
                        entry.blocked_by.push(id);
                    }
                }
            }

            Ok(format!("Updated task:\n{}", entry.format()))
        })
    }
}
