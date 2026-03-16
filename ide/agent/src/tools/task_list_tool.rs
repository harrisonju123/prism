use std::sync::{Arc, Mutex};

use agent_client_protocol as acp;
use gpui::{App, SharedString, Task};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

use super::task_store::{TaskStatus, TaskStore};

/// List all tasks, optionally filtered by status.
///
/// <example>
/// {}
/// { "status_filter": "in_progress" }
/// </example>
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TaskListToolInput {
    /// Only return tasks with this status. If not provided, returns all tasks.
    pub status_filter: Option<TaskStatus>,
}

pub struct TaskListTool {
    store: Arc<Mutex<TaskStore>>,
}

impl TaskListTool {
    pub fn new(store: Arc<Mutex<TaskStore>>) -> Self {
        Self { store }
    }
}

impl AgentTool for TaskListTool {
    type Input = TaskListToolInput;
    type Output = String;

    const NAME: &'static str = "task_list";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        if let Ok(input) = input {
            if let Some(filter) = input.status_filter {
                format!("List tasks ({})", filter).into()
            } else {
                "List all tasks".into()
            }
        } else {
            "List tasks".into()
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

            let tasks: Vec<_> = guard
                .tasks()
                .iter()
                .filter(|t| {
                    input
                        .status_filter
                        .as_ref()
                        .map_or(true, |f| &t.status == f)
                })
                .collect();

            if tasks.is_empty() {
                return Ok("No tasks found.".to_string());
            }

            let output = tasks
                .iter()
                .map(|t| t.format())
                .collect::<Vec<_>>()
                .join("\n\n");

            Ok(format!("{} task(s):\n\n{}", tasks.len(), output))
        })
    }
}
