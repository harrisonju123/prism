use std::sync::Arc;

use crate::error::{PrismError, Result};
use crate::providers::Provider;
use crate::types::{ChatCompletionRequest, Message, MessageRole, TaskType};

use super::taxonomy::{ClassificationResult, ClassifierInput};

const SYSTEM_PROMPT: &str = "\
Given structural signals from an LLM request, determine the primary \
task the user is asking the model to perform.

Possible task types:
- code_generation: writing or implementing new code
- code_review: reviewing diffs, PRs, auditing code quality
- classification: labeling, categorizing, or detecting sentiment
- summarization: condensing or summarizing text
- extraction: parsing or pulling structured data from text
- translation: translating text between languages
- question_answering: answering questions, explaining concepts
- creative_writing: writing stories, poems, creative content
- reasoning: explaining, analyzing, or step-by-step thinking
- conversation: casual chat or dialogue
- tool_selection: choosing or invoking tools/functions
- tool_use: executing tool/function calls
- search: searching for information
- architecture: system design, planning architecture, high-level design
- debugging: investigating bugs, stack traces, root cause analysis
- refactoring: restructuring existing code without changing behavior
- documentation: writing docs, READMEs, API docs, docstrings
- testing: writing tests, test plans, test cases

Respond with ONLY the task type name, nothing else.";

const VALID_TYPES: &[&str] = &[
    "code_generation",
    "code_review",
    "classification",
    "summarization",
    "extraction",
    "translation",
    "question_answering",
    "creative_writing",
    "reasoning",
    "conversation",
    "tool_selection",
    "tool_use",
    "search",
    "embedding",
    "architecture",
    "debugging",
    "refactoring",
    "documentation",
    "testing",
];

fn build_prompt(input: &ClassifierInput) -> String {
    let msg = if input.last_user_message.is_empty() {
        "No user message available.".to_string()
    } else {
        let truncated: String = input.last_user_message.chars().take(500).collect();
        format!("User message:\n{truncated}")
    };
    msg
}

fn parse_response(raw: &str) -> (TaskType, f64) {
    let mut cleaned = raw.trim().to_lowercase();

    // Strip common prefixes the model might add (before replacing spaces)
    for prefix in ["category:", "type:", "task:"] {
        if let Some(rest) = cleaned.strip_prefix(prefix) {
            cleaned = rest.trim().to_string();
        }
    }

    // Normalize separators after prefix stripping
    cleaned = cleaned.replace('-', "_").replace(' ', "_");

    // Exact match
    if VALID_TYPES.contains(&cleaned.as_str()) {
        return (TaskType::from_str_loose(&cleaned), 0.85);
    }

    // Fuzzy: check if any valid type is a substring
    for valid in VALID_TYPES {
        if cleaned.contains(valid) {
            return (TaskType::from_str_loose(valid), 0.7);
        }
    }

    (TaskType::Unknown, 0.0)
}

pub async fn llm_classify(
    input: &ClassifierInput,
    provider: &Arc<dyn Provider>,
    model: &str,
    timeout_ms: u64,
) -> Result<ClassificationResult> {
    let prompt = build_prompt(input);

    let request = ChatCompletionRequest {
        model: model.to_string(),
        messages: vec![
            Message {
                role: MessageRole::System,
                content: Some(serde_json::Value::String(SYSTEM_PROMPT.to_string())),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: serde_json::Map::new(),
            },
            Message {
                role: MessageRole::User,
                content: Some(serde_json::Value::String(prompt)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: serde_json::Map::new(),
            },
        ],
        temperature: Some(0.0),
        max_tokens: Some(20),
        stream: false,
        ..Default::default()
    };

    // Extract the provider model_id (part after '/')
    let model_id = model.split('/').last().unwrap_or(model);

    let result = tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms),
        provider.chat_completion(&request, model_id),
    )
    .await
    .map_err(|_| PrismError::Internal("LLM classifier timed out".to_string()))?
    .map_err(|e| PrismError::Internal(format!("LLM classifier call failed: {e}")))?;

    let response = match result {
        crate::types::ProviderResponse::Complete(resp) => resp,
        crate::types::ProviderResponse::Stream(_) => {
            return Err(PrismError::Internal(
                "LLM classifier got unexpected stream response".to_string(),
            ));
        }
    };

    let raw_content = response
        .choices
        .first()
        .and_then(|c| c.message.content.as_ref())
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let raw = raw_content.trim();
    if raw.is_empty() {
        return Err(PrismError::Internal(
            "LLM classifier returned empty response".to_string(),
        ));
    }

    let (task_type, confidence) = parse_response(raw);

    Ok(ClassificationResult {
        task_type,
        confidence,
        signals: vec![
            format!("llm_classifier:{model}"),
            format!("llm_raw:{}", &raw[..raw.len().min(50)]),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exact_match() {
        let (tt, conf) = parse_response("code_generation");
        assert_eq!(tt, TaskType::CodeGeneration);
        assert!((conf - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_with_whitespace_and_casing() {
        let (tt, conf) = parse_response("  Code_Generation  ");
        assert_eq!(tt, TaskType::CodeGeneration);
        assert!((conf - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_with_dashes() {
        let (tt, conf) = parse_response("code-generation");
        assert_eq!(tt, TaskType::CodeGeneration);
        assert!((conf - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_with_prefix() {
        let (tt, conf) = parse_response("type: summarization");
        assert_eq!(tt, TaskType::Summarization);
        assert!((conf - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_fuzzy_substring() {
        let (tt, conf) = parse_response("I think this is debugging related");
        assert_eq!(tt, TaskType::Debugging);
        assert!((conf - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_unknown() {
        let (tt, conf) = parse_response("something completely unrelated");
        assert_eq!(tt, TaskType::Unknown);
        assert!((conf - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn build_prompt_truncates() {
        let long_msg: String = "a".repeat(600);
        let input = ClassifierInput {
            system_prompt_hash: None,
            has_tools: false,
            tool_count: 0,
            has_json_schema: false,
            has_code_fence_in_system: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            token_ratio: 0.0,
            model: "test".into(),
            has_tool_calls: false,
            output_format_hint: None,
            last_user_message: long_msg,
            system_prompt_text: None,
            has_fim: false,
        };
        let prompt = build_prompt(&input);
        // "User message:\n" = 15 chars + 500 chars = 515 chars
        assert!(prompt.len() <= 515);
        assert!(prompt.starts_with("User message:\n"));
    }

    #[test]
    fn build_prompt_empty_message() {
        let input = ClassifierInput {
            system_prompt_hash: None,
            has_tools: false,
            tool_count: 0,
            has_json_schema: false,
            has_code_fence_in_system: false,
            prompt_tokens: 0,
            completion_tokens: 0,
            token_ratio: 0.0,
            model: "test".into(),
            has_tool_calls: false,
            output_format_hint: None,
            last_user_message: String::new(),
            system_prompt_text: None,
            has_fim: false,
        };
        let prompt = build_prompt(&input);
        assert_eq!(prompt, "No user message available.");
    }
}
