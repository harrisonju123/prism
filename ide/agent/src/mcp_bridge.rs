/// MCP bridge tools: expose PrisM IDE capabilities over the WebSocket MCP server.
///
/// These are standalone `McpServerTool` implementations that query the IDE's
/// `Project` entity directly.
use anyhow::Result;
use context_server::listener::{McpServerTool, ToolResponse};
use context_server::types::{ToolAnnotations, ToolResponseContent};
use gpui::{AsyncApp, Entity};
use language::{DiagnosticSeverity, OffsetRangeExt as _};
use project::Project;
use schemars::JsonSchema;
use serde::Deserialize;
use std::fmt::Write as _;

// ── getDiagnostics ────────────────────────────────────────────────────────────

/// Get errors and warnings for the project or a specific file path.
///
/// When `path` is omitted, returns a summary (file → error/warning counts) for
/// all files in the project. When `path` is provided, returns each diagnostic
/// with its severity, line, and message.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetDiagnosticsInput {
    /// Relative path within the project (e.g. "src/main.rs"). Omit for a
    /// project-wide summary.
    pub path: Option<String>,
}

#[derive(Clone)]
pub struct McpGetDiagnostics {
    pub project: Entity<Project>,
}

impl McpServerTool for McpGetDiagnostics {
    type Input = GetDiagnosticsInput;
    type Output = ();

    const NAME: &'static str = "getDiagnostics";

    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations {
            read_only_hint: Some(true),
            title: None,
            destructive_hint: None,
            idempotent_hint: None,
            open_world_hint: None,
        }
    }

    fn run(
        &self,
        input: Self::Input,
        cx: &mut AsyncApp,
    ) -> impl Future<Output = Result<ToolResponse<Self::Output>>> {
        let project = self.project.clone();

        cx.spawn(async move |cx| -> Result<ToolResponse<()>> {
            let output = match input.path.as_deref() {
                Some(path) if !path.is_empty() => {
                    // entity.update(cx: &mut AsyncApp, closure) returns the closure's return
                    // type directly (not Result-wrapped).
                    let task_opt: Option<_> = project.update(cx, |project, cx| {
                        project
                            .find_project_path(path, cx)
                            .map(|pp| project.open_buffer(pp, cx))
                    });
                    let buffer_task =
                        task_opt.ok_or_else(|| anyhow::anyhow!("path not found: {path}"))?;
                    let buffer = buffer_task.await?;

                    // entity.read_with(cx: &mut AsyncApp, closure) also returns R directly.
                    let snapshot =
                        buffer.read_with(cx, |b: &language::Buffer, _| b.snapshot());
                    let mut out = String::new();

                    for (_, group) in snapshot.diagnostic_groups(None) {
                        let entry = &group.entries[group.primary_ix];
                        let range = entry.range.to_point(&snapshot);
                        let severity = match entry.diagnostic.severity {
                            DiagnosticSeverity::ERROR => "error",
                            DiagnosticSeverity::WARNING => "warning",
                            _ => continue,
                        };
                        writeln!(
                            out,
                            "{} at line {}: {}",
                            severity,
                            range.start.row + 1,
                            entry.diagnostic.message
                        )
                        .ok();
                    }

                    if out.is_empty() {
                        "No errors or warnings.".to_string()
                    } else {
                        out
                    }
                }
                _ => {
                    let (out, has_diag) = project.read_with(cx, |project, cx| {
                        let mut out = String::new();
                        let mut has_diag = false;

                        for (project_path, _, summary) in project.diagnostic_summaries(true, cx) {
                            if summary.error_count > 0 || summary.warning_count > 0 {
                                has_diag = true;
                                if let Some(worktree) =
                                    project.worktree_for_id(project_path.worktree_id, cx)
                                {
                                    let abs =
                                        worktree.read(cx).absolutize(&project_path.path);
                                    writeln!(
                                        out,
                                        "{}: {} error(s), {} warning(s)",
                                        abs.display(),
                                        summary.error_count,
                                        summary.warning_count
                                    )
                                    .ok();
                                }
                            }
                        }
                        (out, has_diag)
                    });

                    if has_diag {
                        out
                    } else {
                        "No errors or warnings in the project.".to_string()
                    }
                }
            };

            Ok(ToolResponse {
                content: vec![ToolResponseContent::Text { text: output }],
                structured_content: (),
            })
        })
    }
}

// ── lsp ───────────────────────────────────────────────────────────────────────

/// Code intelligence operations via the IDE's language server integration.
///
/// Operations: `go_to_definition`, `find_references`, `hover`,
/// `document_symbols`, `workspace_symbols`. Positions are 0-indexed.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct McpLspInput {
    /// The LSP operation to perform.
    pub operation: McpLspOperation,
    /// Relative path to the file (required for all ops except workspace_symbols).
    pub file_path: Option<String>,
    /// 0-indexed line (required for go_to_definition, find_references, hover).
    pub line: Option<u32>,
    /// 0-indexed character offset (required for go_to_definition, find_references, hover).
    pub character: Option<u32>,
    /// Search query (required for workspace_symbols).
    pub query: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpLspOperation {
    GoToDefinition,
    FindReferences,
    Hover,
    DocumentSymbols,
    WorkspaceSymbols,
}

#[derive(Clone)]
pub struct McpLspTool {
    pub project: Entity<Project>,
}

impl McpServerTool for McpLspTool {
    type Input = McpLspInput;
    type Output = ();

    const NAME: &'static str = "lsp";

    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations {
            read_only_hint: Some(true),
            title: None,
            destructive_hint: None,
            idempotent_hint: None,
            open_world_hint: None,
        }
    }

    fn run(
        &self,
        input: Self::Input,
        cx: &mut AsyncApp,
    ) -> impl Future<Output = Result<ToolResponse<Self::Output>>> {
        let project = self.project.clone();

        cx.spawn(async move |cx| -> Result<ToolResponse<()>> {
            let output = run_lsp_operation(project, input, cx).await?;
            Ok(ToolResponse {
                content: vec![ToolResponseContent::Text { text: output }],
                structured_content: (),
            })
        })
    }
}

async fn run_lsp_operation(
    project: Entity<Project>,
    input: McpLspInput,
    cx: &mut AsyncApp,
) -> Result<String> {
    use project::lsp_store::SymbolLocation;

    match input.operation {
        McpLspOperation::WorkspaceSymbols => {
            let query = input.query.unwrap_or_default();
            // entity.update returns Task<Result<...>> directly
            let task = project.update(cx, |project, cx| project.symbols(&query, cx));
            let symbols = task.await?;

            if symbols.is_empty() {
                return Ok(format!("No symbols found matching '{query}'."));
            }

            let mut out = String::new();
            for sym in &symbols {
                let path = match &sym.path {
                    SymbolLocation::InProject(pp) => pp.path.as_unix_str().to_string(),
                    SymbolLocation::OutsideProject { abs_path, .. } => {
                        abs_path.to_string_lossy().into_owned()
                    }
                };
                writeln!(
                    out,
                    "[{:?}] {} — {} (line {})",
                    sym.kind,
                    sym.name,
                    path,
                    sym.range.start.0.row + 1,
                )
                .ok();
            }
            Ok(format!("{} symbol(s):\n{}", symbols.len(), out))
        }

        McpLspOperation::DocumentSymbols => {
            let path = input
                .file_path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("file_path required for document_symbols"))?;

            let buffer = open_buffer(&project, path, cx).await?;
            let task =
                project.update(cx, |project, cx| project.document_symbols(&buffer, cx));
            let symbols = task.await?;

            if symbols.is_empty() {
                return Ok(format!("No symbols in '{path}'."));
            }

            Ok(format!(
                "{} symbol(s) in '{path}':\n{}",
                symbols.len(),
                crate::tools::format_document_symbols(&symbols, 0)
            ))
        }

        McpLspOperation::GoToDefinition => {
            let (path, position) = parse_position(&input)?;
            let buffer = open_buffer(&project, path, cx).await?;
            let task = project
                .update(cx, |project, cx| project.definitions(&buffer, position, cx));
            let locs_opt = task.await?;

            let Some(locs) = locs_opt else {
                return Ok("No definition found.".to_string());
            };
            if locs.is_empty() {
                return Ok("No definition found.".to_string());
            }

            let mut out = String::new();
            for loc in &locs {
                let anchor = loc.target.range.start;
                let (file, row) =
                    loc.target
                        .buffer
                        .read_with(cx, |b: &language::Buffer, _| {
                            use text::ToPoint as _;
                            let row = anchor.to_point(&b.snapshot()).row;
                            let file = b
                                .file()
                                .map(|f| f.path().as_unix_str().to_string())
                                .unwrap_or_else(|| "<unknown>".to_string());
                            (file, row)
                        });
                writeln!(out, "{file}:{}", row + 1).ok();
            }
            Ok(format!("Definition(s):\n{out}"))
        }

        McpLspOperation::FindReferences => {
            let (path, position) = parse_position(&input)?;
            let buffer = open_buffer(&project, path, cx).await?;
            let task = project
                .update(cx, |project, cx| project.references(&buffer, position, cx));
            let locs_opt = task.await?;

            let Some(locs) = locs_opt else {
                return Ok("No references found.".to_string());
            };
            if locs.is_empty() {
                return Ok("No references found.".to_string());
            }

            let mut out = String::new();
            for loc in &locs {
                let anchor = loc.range.start;
                let (file, row) = loc.buffer.read_with(cx, |b: &language::Buffer, _| {
                    use text::ToPoint as _;
                    let row = anchor.to_point(&b.snapshot()).row;
                    let file = b
                        .file()
                        .map(|f| f.path().as_unix_str().to_string())
                        .unwrap_or_else(|| "<unknown>".to_string());
                    (file, row)
                });
                writeln!(out, "{file}:{}", row + 1).ok();
            }
            Ok(format!("{} reference(s):\n{out}", locs.len()))
        }

        McpLspOperation::Hover => {
            let (path, position) = parse_position(&input)?;
            let buffer = open_buffer(&project, path, cx).await?;
            let task =
                project.update(cx, |project, cx| project.hover(&buffer, position, cx));
            // hover returns Task<Option<Vec<Hover>>> — no Result wrapper
            let hover = task.await;

            match hover {
                None => Ok("No hover info.".to_string()),
                Some(hovers) => {
                    let text = hovers
                        .iter()
                        .map(|h| h.contents.iter().map(|b| b.text.as_str()).collect::<Vec<_>>().join("\n"))
                        .collect::<Vec<_>>()
                        .join("\n---\n");
                    Ok(text)
                }
            }
        }
    }
}

fn parse_position(input: &McpLspInput) -> Result<(&str, language::PointUtf16)> {
    let path = input
        .file_path
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("file_path required"))?;
    let line = input.line.ok_or_else(|| anyhow::anyhow!("line required"))?;
    let character = input
        .character
        .ok_or_else(|| anyhow::anyhow!("character required"))?;
    Ok((path, language::PointUtf16::new(line, character)))
}

async fn open_buffer(
    project: &Entity<Project>,
    path: &str,
    cx: &mut AsyncApp,
) -> Result<Entity<language::Buffer>> {
    let task_opt: Option<_> = project.update(cx, |project, cx| {
        project
            .find_project_path(path, cx)
            .map(|pp| project.open_buffer(pp, cx))
    });
    task_opt
        .ok_or_else(|| anyhow::anyhow!("path not found: {path}"))?
        .await
        .map_err(Into::into)
}
