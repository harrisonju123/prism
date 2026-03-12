use crate::{AgentTool, ToolCallEventStream, ToolInput};
use agent_client_protocol as acp;
use anyhow::Result;
use futures::FutureExt as _;
use gpui::{App, Task};
use language_model::LanguageModelProviderId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use semantic_index::SemanticIndex;

/// Semantic similarity search over the indexed codebase.
///
/// Use this tool when you need to find code related to a concept, pattern, or behaviour
/// but don't know the exact file path or symbol name. The search embeds your query and
/// returns the most semantically similar chunks from the index.
///
/// Prefer `grep` when you have a known identifier or regex pattern;
/// prefer `codebase_search` when you have a description of what the code should *do*.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CodebaseSearchToolInput {
    /// Natural-language description of the code you're looking for.
    /// Examples: "authentication middleware", "rate limiter sliding window",
    /// "database connection pooling", "error handling for HTTP 429"
    pub query: String,
    /// Maximum number of results to return (default 10, max 25).
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_limit() -> u32 {
    10
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CodebaseSearchToolOutput {
    Results { results: Vec<CodebaseSearchResult> },
    NotIndexed { message: String },
    Error { error: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CodebaseSearchResult {
    pub file: String,
    pub start_line: u32,
    pub end_line: u32,
    pub symbol: Option<String>,
    pub score: f32,
}

impl From<CodebaseSearchToolOutput> for language_model::LanguageModelToolResultContent {
    fn from(output: CodebaseSearchToolOutput) -> Self {
        serde_json::to_string(&output)
            .unwrap_or_else(|e| format!("{{\"error\": \"serialization failed: {e}\"}}"))
            .into()
    }
}

pub struct CodebaseSearchTool;

impl AgentTool for CodebaseSearchTool {
    type Input = CodebaseSearchToolInput;
    type Output = CodebaseSearchToolOutput;

    const NAME: &'static str = "codebase_search";

    fn kind() -> acp::ToolKind {
        acp::ToolKind::Read
    }

    fn supports_provider(_provider: &LanguageModelProviderId) -> bool {
        true
    }

    fn initial_title(
        &self,
        input: Result<Self::Input, serde_json::Value>,
        _cx: &mut App,
    ) -> gpui::SharedString {
        match input {
            Ok(i) => format!("Searching codebase: {}", i.query).into(),
            Err(_) => "Searching codebase".into(),
        }
    }

    fn run(
        self: std::sync::Arc<Self>,
        input: ToolInput<Self::Input>,
        event_stream: ToolCallEventStream,
        cx: &mut App,
    ) -> Task<Result<Self::Output, Self::Output>> {
        cx.spawn(async move |cx| {
            let input = input.recv().await.map_err(|e| CodebaseSearchToolOutput::Error {
                error: format!("failed to receive input: {e}"),
            })?;

            let limit = (input.limit.min(25) as usize).max(1);
            let query = input.query.clone();

            let search_task = cx
                .update(|cx| {
                    let Some(index) = cx.try_global::<SemanticIndex>() else {
                        return Err(CodebaseSearchToolOutput::NotIndexed {
                            message: "Codebase index is not available. Open a project and wait \
                                      for indexing to complete before using this tool."
                                .into(),
                        });
                    };

                    let index = index.clone();

                    Ok(cx.background_executor().spawn(async move {
                        index.search(&query, limit).await
                    }))
                })?;

            let results = futures::select! {
                result = search_task.fuse() => {
                    result.map_err(|e| CodebaseSearchToolOutput::Error {
                        error: format!("search failed: {e}"),
                    })?
                }
                _ = event_stream.cancelled_by_user().fuse() => {
                    return Err(CodebaseSearchToolOutput::Error {
                        error: "search cancelled".into(),
                    });
                }
            };

            let n = results.len();
            event_stream.update_fields(
                acp::ToolCallUpdateFields::new()
                    .title(format!("Found {} result{}", n, if n == 1 { "" } else { "s" })),
            );

            let output_results: Vec<CodebaseSearchResult> = results
                .into_iter()
                .map(|r| CodebaseSearchResult {
                    file: r.file_path.to_string_lossy().into_owned(),
                    start_line: r.start_line,
                    end_line: r.end_line,
                    symbol: r.symbol_name,
                    score: r.score,
                })
                .collect();

            Ok(CodebaseSearchToolOutput::Results { results: output_results })
        })
    }

    fn replay(
        &self,
        _input: Self::Input,
        output: Self::Output,
        event_stream: ToolCallEventStream,
        _cx: &mut App,
    ) -> Result<()> {
        if let CodebaseSearchToolOutput::Results { results } = &output {
            let n = results.len();
            event_stream.update_fields(
                acp::ToolCallUpdateFields::new()
                    .title(format!("Found {} result{}", n, if n == 1 { "" } else { "s" })),
            );
        }
        Ok(())
    }
}
