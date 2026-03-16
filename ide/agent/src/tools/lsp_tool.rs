use std::{fmt::Write as _, sync::Arc};

use agent_client_protocol as acp;
use futures::FutureExt as _;
use gpui::{App, AsyncApp, Entity, SharedString, Task};
use language::{Buffer, PointUtf16};
use project::{DocumentSymbol, Project, lsp_store::SymbolLocation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use text::ToPoint as _;

use crate::{AgentTool, ToolCallEventStream, ToolInput};

/// Code intelligence operations via the IDE's language server integration.
///
/// Provides symbol navigation and lookup powered by the Language Server Protocol.
///
/// Operations:
/// - `go_to_definition`: Jump to where a symbol is defined.
/// - `find_references`: Find all usages of a symbol.
/// - `hover`: Get type/documentation info at a position.
/// - `document_symbols`: List all symbols (functions, types, etc.) in a file.
/// - `workspace_symbols`: Search for symbols across the whole workspace.
///
/// Positions are 0-indexed (line 0 = first line, character 0 = first column).
///
/// <example>
/// { "operation": "go_to_definition", "file_path": "src/main.rs", "line": 10, "character": 5 }
/// { "operation": "workspace_symbols", "query": "HttpClient" }
/// { "operation": "document_symbols", "file_path": "src/lib.rs" }
/// </example>
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct LspToolInput {
    /// The LSP operation to perform.
    pub operation: LspOperation,
    /// Path to the file (required for all operations except workspace_symbols).
    /// Should be relative to a project root, e.g. "src/main.rs".
    pub file_path: Option<String>,
    /// 0-indexed line number (required for go_to_definition, find_references, hover).
    pub line: Option<u32>,
    /// 0-indexed character offset (required for go_to_definition, find_references, hover).
    pub character: Option<u32>,
    /// Search query (required for workspace_symbols).
    pub query: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LspOperation {
    /// Go to the definition of the symbol at the given position.
    GoToDefinition,
    /// Find all references to the symbol at the given position.
    FindReferences,
    /// Get hover information (type, documentation) at the given position.
    Hover,
    /// List all symbols defined in the given file.
    DocumentSymbols,
    /// Search for symbols matching a query across the workspace.
    WorkspaceSymbols,
}

pub struct LspTool {
    project: Entity<Project>,
}

impl LspTool {
    pub fn new(project: Entity<Project>) -> Self {
        Self { project }
    }
}

fn format_document_symbols(symbols: &[DocumentSymbol], indent: usize) -> String {
    let mut out = String::new();
    for sym in symbols {
        let pad = "  ".repeat(indent);
        writeln!(
            out,
            "{}[{}] {} (line {})",
            pad,
            symbol_kind_str(sym.kind),
            sym.name,
            sym.selection_range.start.0.row + 1,
        )
        .ok();
        if !sym.children.is_empty() {
            out.push_str(&format_document_symbols(&sym.children, indent + 1));
        }
    }
    out
}

fn symbol_kind_str(kind: impl std::fmt::Debug) -> String {
    format!("{:?}", kind).to_lowercase()
}

fn symbol_location_path(loc: &SymbolLocation) -> String {
    match loc {
        SymbolLocation::InProject(pp) => pp.path.as_unix_str().to_string(),
        SymbolLocation::OutsideProject { abs_path, .. } => {
            abs_path.to_string_lossy().into_owned()
        }
    }
}

fn buffer_file_path(buffer: &Buffer) -> String {
    buffer
        .file()
        .map(|f| f.path().as_unix_str().to_string())
        .unwrap_or_else(|| "<unknown>".to_string())
}

/// Extract and validate the position fields (path, line, character) required by position-based operations.
fn parse_position_input(input: &LspToolInput) -> Result<(&str, PointUtf16), String> {
    let path = input
        .file_path
        .as_deref()
        .ok_or("file_path is required")?;
    let line = input.line.ok_or("line is required")?;
    let character = input.character.ok_or("character is required")?;
    Ok((path, PointUtf16::new(line, character)))
}

/// Open the buffer for `path`, cancelling if the user cancels the operation.
async fn open_buffer_for_path(
    project: &Entity<Project>,
    path: &str,
    event_stream: &ToolCallEventStream,
    cx: &mut AsyncApp,
) -> Result<Entity<Buffer>, String> {
    let open_task = project.update(cx, |project, cx| {
        let project_path = project
            .find_project_path(path, cx)
            .ok_or_else(|| format!("Path not found: {}", path))?;
        Ok::<_, String>(project.open_buffer(project_path, cx))
    })?;

    futures::select! {
        result = open_task.fuse() => result.map_err(|e| e.to_string()),
        _ = event_stream.cancelled_by_user().fuse() => {
            Err("LSP query cancelled by user".to_string())
        }
    }
}

impl AgentTool for LspTool {
    type Input = LspToolInput;
    type Output = String;

    const NAME: &'static str = "lsp";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> SharedString {
        if let Ok(input) = input {
            let op = match input.operation {
                LspOperation::GoToDefinition => "Go to definition",
                LspOperation::FindReferences => "Find references",
                LspOperation::Hover => "Hover",
                LspOperation::DocumentSymbols => "Document symbols",
                LspOperation::WorkspaceSymbols => "Workspace symbols",
            };
            if let Some(path) = &input.file_path {
                format!("{}: {}", op, path).into()
            } else {
                op.into()
            }
        } else {
            "LSP operation".into()
        }
    }

    fn run(
        self: Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<String, String>> {
        let project = self.project.clone();
        cx.spawn(async move |cx| {
            let input = input
                .recv()
                .await
                .map_err(|e| format!("Failed to receive tool input: {e}"))?;

            match input.operation {
                LspOperation::WorkspaceSymbols => {
                    let query = input.query.unwrap_or_default();
                    let task = project.update(cx, |project, cx| project.symbols(&query, cx));

                    let symbols = futures::select! {
                        result = task.fuse() => result.map_err(|e| e.to_string())?,
                        _ = event_stream.cancelled_by_user().fuse() => {
                            return Err("LSP query cancelled by user".to_string());
                        }
                    };

                    if symbols.is_empty() {
                        return Ok(format!("No symbols found matching '{}'.", query));
                    }
                    let mut out = String::new();
                    for sym in &symbols {
                        let path = symbol_location_path(&sym.path);
                        writeln!(
                            out,
                            "[{}] {} — {} (line {})",
                            symbol_kind_str(sym.kind),
                            sym.name,
                            path,
                            sym.range.start.0.row + 1,
                        )
                        .ok();
                    }
                    Ok(format!("{} symbol(s) found:\n{}", symbols.len(), out))
                }

                LspOperation::DocumentSymbols => {
                    let path = input
                        .file_path
                        .as_deref()
                        .ok_or("file_path is required for document_symbols")?;

                    let buffer =
                        open_buffer_for_path(&project, path, &event_stream, cx).await?;

                    let sym_task =
                        project.update(cx, |project, cx| project.document_symbols(&buffer, cx));

                    let symbols = futures::select! {
                        result = sym_task.fuse() => result.map_err(|e| e.to_string())?,
                        _ = event_stream.cancelled_by_user().fuse() => {
                            return Err("LSP query cancelled by user".to_string());
                        }
                    };

                    if symbols.is_empty() {
                        return Ok(format!("No symbols found in '{}'.", path));
                    }
                    Ok(format!(
                        "{} symbol(s) in '{}':\n{}",
                        symbols.len(),
                        path,
                        format_document_symbols(&symbols, 0)
                    ))
                }

                LspOperation::GoToDefinition => {
                    let (path, position) = parse_position_input(&input)?;
                    let buffer =
                        open_buffer_for_path(&project, path, &event_stream, cx).await?;

                    let task = project.update(cx, |project, cx| {
                        project.definitions(&buffer, position, cx)
                    });

                    let locations = futures::select! {
                        result = task.fuse() => result.map_err(|e| e.to_string())?,
                        _ = event_stream.cancelled_by_user().fuse() => {
                            return Err("LSP query cancelled by user".to_string());
                        }
                    };

                    let Some(locations) = locations else {
                        return Ok("No definition found.".to_string());
                    };
                    if locations.is_empty() {
                        return Ok("No definition found.".to_string());
                    }

                    let mut out = String::new();
                    for loc in &locations {
                        let anchor = loc.target.range.start;
                        let (file, row) = loc.target.buffer.read_with(cx, |b: &Buffer, _| {
                            let row = anchor.to_point(&b.snapshot()).row;
                            (buffer_file_path(b), row)
                        });
                        writeln!(out, "{}:{}", file, row + 1).ok();
                    }
                    Ok(format!("Definition(s):\n{}", out))
                }

                LspOperation::FindReferences => {
                    let (path, position) = parse_position_input(&input)?;
                    let buffer =
                        open_buffer_for_path(&project, path, &event_stream, cx).await?;

                    let task = project.update(cx, |project, cx| {
                        project.references(&buffer, position, cx)
                    });

                    let locations = futures::select! {
                        result = task.fuse() => result.map_err(|e| e.to_string())?,
                        _ = event_stream.cancelled_by_user().fuse() => {
                            return Err("LSP query cancelled by user".to_string());
                        }
                    };

                    let Some(locations) = locations else {
                        return Ok("No references found.".to_string());
                    };
                    if locations.is_empty() {
                        return Ok("No references found.".to_string());
                    }

                    let mut out = String::new();
                    for loc in &locations {
                        let anchor = loc.range.start;
                        let (file, row) = loc.buffer.read_with(cx, |b: &Buffer, _| {
                            let row = anchor.to_point(&b.snapshot()).row;
                            (buffer_file_path(b), row)
                        });
                        writeln!(out, "{}:{}", file, row + 1).ok();
                    }
                    Ok(format!("{} reference(s):\n{}", locations.len(), out))
                }

                LspOperation::Hover => {
                    let (path, position) = parse_position_input(&input)?;
                    let buffer =
                        open_buffer_for_path(&project, path, &event_stream, cx).await?;

                    let task =
                        project.update(cx, |project, cx| project.hover(&buffer, position, cx));

                    let hovers = futures::select! {
                        result = task.fuse() => result,
                        _ = event_stream.cancelled_by_user().fuse() => {
                            return Err("LSP query cancelled by user".to_string());
                        }
                    };

                    let Some(hovers) = hovers else {
                        return Ok("No hover information available.".to_string());
                    };
                    if hovers.is_empty() {
                        return Ok("No hover information available.".to_string());
                    }

                    let mut out = String::new();
                    for hover in &hovers {
                        for block in &hover.contents {
                            out.push_str(&block.text);
                            out.push('\n');
                        }
                    }
                    Ok(out.trim().to_string())
                }
            }
        })
    }
}
