use std::collections::HashMap;
use std::sync::LazyLock;

use crate::types::TaskType;

/// Input features extracted from a request for classification.
#[derive(Debug, Clone, Default)]
pub struct ClassifierInput {
    pub system_prompt_hash: Option<String>,
    pub has_tools: bool,
    pub tool_count: usize,
    pub has_json_schema: bool,
    pub has_code_fence_in_system: bool,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub token_ratio: f64,
    pub model: String,
    pub has_tool_calls: bool,
    pub output_format_hint: Option<OutputFormatHint>,
    pub last_user_message: String,
    pub system_prompt_text: Option<String>,
    /// True when the request carries a FIM `suffix` field (structural FIM signal).
    pub has_fim: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputFormatHint {
    Json,
    Code,
    Markdown,
}

/// Result of task classification.
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    pub task_type: TaskType,
    pub confidence: f64,
    pub signals: Vec<String>,
}

/// Keyword lists for each task type, used by the rules-based classifier.
pub static TASK_KEYWORDS: LazyLock<HashMap<TaskType, Vec<&'static str>>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    m.insert(
        TaskType::CodeGeneration,
        vec![
            "write a function",
            "implement",
            "create a class",
            "code to",
            "generate code",
            "write code",
            "build a",
            "program to",
            "develop a",
            "write a script",
            "coding",
            "scaffold",
        ],
    );

    m.insert(
        TaskType::CodeReview,
        vec![
            "review",
            "code review",
            "pull request",
            "PR review",
            "check this code",
            "audit",
            "inspect",
            "critique this",
            "review my code",
        ],
    );

    m.insert(
        TaskType::Summarization,
        vec![
            "summarize",
            "summary",
            "tldr",
            "brief overview",
            "condense",
            "key points",
            "main takeaways",
            "recap",
            "digest",
        ],
    );

    m.insert(
        TaskType::Classification,
        vec![
            "classify",
            "categorize",
            "label",
            "sort into",
            "which category",
            "determine the type",
            "identify the class",
        ],
    );

    m.insert(
        TaskType::Extraction,
        vec![
            "extract",
            "parse",
            "pull out",
            "find all",
            "identify entities",
            "get the",
            "list all",
            "named entity",
        ],
    );

    m.insert(
        TaskType::Translation,
        vec![
            "translate",
            "convert to",
            "in french",
            "in spanish",
            "to english",
            "localize",
            "i18n",
        ],
    );

    m.insert(
        TaskType::QuestionAnswering,
        vec![
            "what is",
            "how does",
            "why does",
            "explain",
            "tell me about",
            "what are",
            "describe",
        ],
    );

    m.insert(
        TaskType::CreativeWriting,
        vec![
            "write a story",
            "poem",
            "creative",
            "fiction",
            "narrative",
            "compose",
            "draft a",
            "write a blog",
        ],
    );

    m.insert(
        TaskType::Reasoning,
        vec![
            "reason",
            "think step by step",
            "analyze",
            "evaluate",
            "compare",
            "pros and cons",
            "trade-offs",
            "chain of thought",
            "logical",
        ],
    );

    m.insert(
        TaskType::Conversation,
        vec![
            "chat",
            "hello",
            "hi",
            "hey",
            "thanks",
            "thank you",
            "goodbye",
            "how are you",
        ],
    );

    m.insert(
        TaskType::Architecture,
        vec![
            "architect",
            "design system",
            "system design",
            "infrastructure",
            "high-level design",
            "architecture",
            "microservice",
            "distributed",
        ],
    );

    m.insert(
        TaskType::Debugging,
        vec![
            "debug",
            "fix this",
            "error",
            "bug",
            "traceback",
            "stack trace",
            "not working",
            "broken",
            "issue",
            "crash",
        ],
    );

    m.insert(
        TaskType::Refactoring,
        vec![
            "refactor",
            "clean up",
            "restructure",
            "simplify",
            "optimize code",
            "improve code",
            "rewrite",
        ],
    );

    m.insert(
        TaskType::Documentation,
        vec![
            "document",
            "docstring",
            "readme",
            "api docs",
            "documentation",
            "jsdoc",
            "rustdoc",
            "comments",
        ],
    );

    m.insert(
        TaskType::Testing,
        vec![
            "test",
            "unit test",
            "integration test",
            "test case",
            "spec",
            "assertion",
            "mock",
            "coverage",
        ],
    );

    m.insert(
        TaskType::ToolSelection,
        vec![
            "which tool",
            "select tool",
            "tool choice",
            "use tool",
            "call function",
        ],
    );

    m.insert(
        TaskType::Search,
        vec![
            "search",
            "look up",
            "find information",
            "google",
            "retrieve",
        ],
    );

    m.insert(
        TaskType::Embedding,
        vec!["embed", "embedding", "vector", "similarity"],
    );

    m.insert(
        TaskType::ToolUse,
        vec!["use tool", "function call", "tool_use", "run tool"],
    );

    m
});
