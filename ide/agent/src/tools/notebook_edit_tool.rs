use std::sync::Arc;

use agent_client_protocol as acp;
use gpui::{App, Entity, SharedString, Task};
use project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{AgentTool, ToolCallEventStream, ToolInput};

/// Edit a Jupyter notebook (.ipynb) by modifying, inserting, or deleting cells.
///
/// Notebooks are parsed as JSON (nbformat v4). Changes are written back to disk.
///
/// <example>
/// Replace cell 2 with new Python code:
/// { "notebook_path": "analysis/demo.ipynb", "edit_mode": "replace", "cell_index": 2, "new_source": "x = 42\nprint(x)" }
///
/// Insert a markdown cell before index 1:
/// { "notebook_path": "demo.ipynb", "edit_mode": "insert", "cell_index": 1, "new_source": "## Section 2", "cell_type": "markdown" }
///
/// Delete cell 3:
/// { "notebook_path": "demo.ipynb", "edit_mode": "delete", "cell_index": 3 }
/// </example>
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct NotebookEditToolInput {
    /// Path to the .ipynb file relative to a project root.
    pub notebook_path: String,
    /// The edit operation to perform.
    pub edit_mode: NotebookEditMode,
    /// 0-indexed cell position. For `replace`/`delete`, targets this cell.
    /// For `insert`, the new cell is placed before this index (use the current cell count to append).
    pub cell_index: usize,
    /// New cell source (required for `replace` and `insert`).
    pub new_source: Option<String>,
    /// Cell type for `insert` (defaults to "code"). Ignored for `replace`/`delete`.
    pub cell_type: Option<NotebookCellType>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotebookEditMode {
    /// Replace the source of an existing cell.
    Replace,
    /// Insert a new cell before the given index.
    Insert,
    /// Delete the cell at the given index.
    Delete,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotebookCellType {
    Code,
    Markdown,
    Raw,
}

impl NotebookCellType {
    fn as_str(&self) -> &'static str {
        match self {
            NotebookCellType::Code => "code",
            NotebookCellType::Markdown => "markdown",
            NotebookCellType::Raw => "raw",
        }
    }
}

pub struct NotebookEditTool {
    project: Entity<Project>,
}

impl NotebookEditTool {
    pub fn new(project: Entity<Project>) -> Self {
        Self { project }
    }
}

impl AgentTool for NotebookEditTool {
    type Input = NotebookEditToolInput;
    type Output = String;

    const NAME: &'static str = "notebook_edit";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Edit
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        if let Ok(input) = input {
            let op = match input.edit_mode {
                NotebookEditMode::Replace => "Edit",
                NotebookEditMode::Insert => "Insert",
                NotebookEditMode::Delete => "Delete",
            };
            format!("{} notebook cell: {}", op, input.notebook_path).into()
        } else {
            "Edit notebook".into()
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

            // Validate extension on the input string before doing any project lookup.
            if !input.notebook_path.ends_with(".ipynb") {
                return Err(format!(
                    "'{}' does not appear to be a Jupyter notebook (.ipynb)",
                    input.notebook_path
                ));
            }

            // Resolve absolute path and grab fs handle in one read.
            let (abs_path, fs) = project
                .read_with(cx, |project, cx| {
                    let pp = project.find_project_path(&input.notebook_path, cx)?;
                    let wt = project.worktree_for_id(pp.worktree_id, cx)?;
                    let abs_path = wt.read(cx).absolutize(&pp.path);
                    let fs = project.fs().clone();
                    Some((abs_path, fs))
                })
                .ok_or_else(|| {
                    format!("Notebook '{}' not found in project", input.notebook_path)
                })?;

            // Read and parse the notebook using the project's async fs abstraction.
            let raw = fs
                .load(&abs_path)
                .await
                .map_err(|e| format!("Failed to read notebook: {e}"))?;
            let mut notebook: serde_json::Value =
                serde_json::from_str(&raw).map_err(|e| format!("Invalid notebook JSON: {e}"))?;

            let cells = notebook["cells"]
                .as_array_mut()
                .ok_or("Notebook has no 'cells' array")?;

            match input.edit_mode {
                NotebookEditMode::Replace => {
                    let source = input
                        .new_source
                        .ok_or("new_source is required for replace")?;
                    let num_cells = cells.len();
                    let cell = cells.get_mut(input.cell_index).ok_or_else(|| {
                        format!(
                            "Cell index {} out of range (have {})",
                            input.cell_index, num_cells
                        )
                    })?;
                    cell["source"] = serde_json::Value::String(source);
                    // Clear previous outputs on replace.
                    if cell["cell_type"].as_str() == Some("code") {
                        cell["outputs"] = serde_json::json!([]);
                        cell["execution_count"] = serde_json::Value::Null;
                    }
                }
                NotebookEditMode::Insert => {
                    let source = input
                        .new_source
                        .ok_or("new_source is required for insert")?;
                    let cell_type = input
                        .cell_type
                        .unwrap_or(NotebookCellType::Code)
                        .as_str()
                        .to_string();
                    let new_cell = if cell_type == "code" {
                        serde_json::json!({
                            "cell_type": cell_type,
                            "metadata": {},
                            "source": source,
                            "outputs": [],
                            "execution_count": null
                        })
                    } else {
                        serde_json::json!({
                            "cell_type": cell_type,
                            "metadata": {},
                            "source": source
                        })
                    };
                    let idx = input.cell_index.min(cells.len());
                    cells.insert(idx, new_cell);
                }
                NotebookEditMode::Delete => {
                    let num_cells = cells.len();
                    if input.cell_index >= num_cells {
                        return Err(format!(
                            "Cell index {} out of range (have {})",
                            input.cell_index, num_cells
                        ));
                    }
                    cells.remove(input.cell_index);
                }
            }

            // Track cell count from the already-mutated slice rather than re-indexing the JSON.
            let cell_count = cells.len();

            let updated = serde_json::to_string_pretty(&notebook)
                .map_err(|e| format!("Failed to serialize notebook: {e}"))?;
            fs.atomic_write(abs_path, updated)
                .await
                .map_err(|e| format!("Failed to write notebook: {e}"))?;

            Ok(format!(
                "Notebook '{}' updated successfully ({} cells).",
                input.notebook_path, cell_count
            ))
        })
    }
}
