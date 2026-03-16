use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, Entity, SharedString, Task};
use project::{Project, Worktree};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

/// Adds an additional working directory to the agent's current workspace mid-session.
///
/// Use this when the user references files in a directory outside the current project roots,
/// or when they ask to include an additional repository or directory in the workspace.
///
/// <example>
/// { "path": "/home/user/other-project" }
/// </example>
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AddDirToolInput {
    /// Absolute path to the directory to add to the workspace.
    pub path: String,
}

pub struct AddDirTool {
    project: Entity<Project>,
}

impl AddDirTool {
    pub fn new(project: Entity<Project>) -> Self {
        Self { project }
    }
}

impl AgentTool for AddDirTool {
    type Input = AddDirToolInput;
    type Output = String;

    const NAME: &'static str = "add_dir";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Other
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        if let Ok(input) = input {
            format!("Add directory: {}", input.path).into()
        } else {
            "Add directory to workspace".into()
        }
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        _event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        let project = self.project.clone();
        cx.spawn(async move |cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive tool input: {e}"))?;

            let path = std::path::PathBuf::from(&input.path);

            // project.update returns the Task directly (no Result wrapper at the update level)
            let worktree_task = project.update(cx, |project, cx| {
                project.find_or_create_worktree(&path, true, cx)
            });

            let (worktree, _) = worktree_task
                .await
                .map_err(|e| format!("Failed to add directory '{}': {}", input.path, e))?;

            let name =
                worktree.read_with(cx, |wt: &Worktree, _| wt.root_name().as_unix_str().to_string());

            Ok(format!(
                "Added directory '{}' (root: '{}') to the workspace.",
                input.path, name
            ))
        })
    }
}
