use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use super::taxonomy::{ClassificationResult, ClassifierInput, OutputFormatHint, TASK_KEYWORDS};
use crate::types::TaskType;

/// Pre-compiled regexes for keyword matching.
/// Each entry is `(Regex, is_single_word)` -- single-word keywords use `\b` word
/// boundaries while multi-word keywords use a plain case-insensitive contains.
static KEYWORD_REGEXES: LazyLock<HashMap<TaskType, Vec<(Regex, bool)>>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for (task_type, keywords) in TASK_KEYWORDS.iter() {
        let regexes: Vec<(Regex, bool)> = keywords
            .iter()
            .map(|kw| {
                let is_single = !kw.contains(' ');
                if is_single {
                    let pattern = format!(r"(?i)\b{}\b", regex::escape(kw));
                    (Regex::new(&pattern).unwrap(), true)
                } else {
                    let pattern = format!(r"(?i){}", regex::escape(kw));
                    (Regex::new(&pattern).unwrap(), false)
                }
            })
            .collect();
        map.insert(*task_type, regexes);
    }
    map
});

pub struct RulesClassifier;

impl RulesClassifier {
    pub fn classify(input: &ClassifierInput) -> ClassificationResult {
        let mut scores: HashMap<TaskType, f64> = HashMap::new();
        let mut signals: Vec<String> = Vec::new();

        // Signal: tool_array_present
        if input.has_tools {
            *scores.entry(TaskType::ToolSelection).or_default() += 0.15;
            signals.push("tool_array_present".into());
        }

        // Signal: many_tools
        if input.tool_count > 3 {
            *scores.entry(TaskType::ToolSelection).or_default() += 0.05;
            signals.push("many_tools".into());
        }

        // Signal: model_used_tools
        if input.has_tool_calls {
            *scores.entry(TaskType::ToolSelection).or_default() += 0.40;
            signals.push("model_used_tools".into());
        }

        // Signal: code_fence_in_system
        if input.has_code_fence_in_system {
            *scores.entry(TaskType::CodeGeneration).or_default() += 0.40;
            signals.push("code_fence_in_system".into());
        }

        // Signal: json_schema_output
        if input.has_json_schema {
            *scores.entry(TaskType::Classification).or_default() += 0.30;
            *scores.entry(TaskType::Extraction).or_default() += 0.30;
            signals.push("json_schema_output".into());
        }

        // Signal: low_token_ratio (classification/extraction pattern)
        if input.token_ratio < 0.1 && input.completion_tokens < 50 {
            *scores.entry(TaskType::Classification).or_default() += 0.30;
            signals.push("low_token_ratio".into());
        }

        // Signal: high_token_ratio (code gen / reasoning pattern)
        if input.token_ratio > 2.0 {
            *scores.entry(TaskType::CodeGeneration).or_default() += 0.20;
            *scores.entry(TaskType::Reasoning).or_default() += 0.20;
            signals.push("high_token_ratio".into());
        }

        // Signal: output format hints
        match &input.output_format_hint {
            Some(OutputFormatHint::Json) => {
                *scores.entry(TaskType::Classification).or_default() += 0.15;
                *scores.entry(TaskType::Extraction).or_default() += 0.15;
                signals.push("output_hint_json".into());
            }
            Some(OutputFormatHint::Code) => {
                *scores.entry(TaskType::CodeGeneration).or_default() += 0.20;
                signals.push("output_hint_code".into());
            }
            Some(OutputFormatHint::Markdown) => {
                *scores.entry(TaskType::Documentation).or_default() += 0.10;
                *scores.entry(TaskType::Summarization).or_default() += 0.10;
                signals.push("output_hint_markdown".into());
            }
            None => {}
        }

        // Signal: system_prompt_keywords
        let boost_types = [
            TaskType::CodeReview,
            TaskType::Reasoning,
            TaskType::Summarization,
            TaskType::Architecture,
            TaskType::Debugging,
        ];
        if let Some(system_text) = &input.system_prompt_text {
            let lower = system_text.to_lowercase();
            for (task_type, regexes) in KEYWORD_REGEXES.iter() {
                for (re, _) in regexes {
                    if re.is_match(&lower) {
                        let boost = if boost_types.contains(task_type) {
                            0.30
                        } else {
                            0.15
                        };
                        *scores.entry(*task_type).or_default() += boost;
                        signals.push(format!("system_kw_{}", task_type));
                        break; // only count once per task type for system prompt
                    }
                }
            }
        }

        // Signal: user_prompt_keywords (+0.50 per match)
        let user_lower = input.last_user_message.to_lowercase();
        for (task_type, regexes) in KEYWORD_REGEXES.iter() {
            for (re, _) in regexes {
                if re.is_match(&user_lower) {
                    *scores.entry(*task_type).or_default() += 0.50;
                    signals.push(format!("user_kw_{}", task_type));
                    break; // once per task type
                }
            }
        }

        // Signal: review_keywords_with_code_fence
        if input.has_code_fence_in_system {
            let review_kws = ["review", "audit", "inspect", "critique"];
            if review_kws.iter().any(|kw| user_lower.contains(kw)) {
                *scores.entry(TaskType::CodeReview).or_default() += 0.30;
                *scores.entry(TaskType::CodeGeneration).or_default() -= 0.20;
                signals.push("review_keywords_with_code_fence".into());
            }
        }

        // Signal: inline_code_edit (short prompt + edit keyword → CodeEdit)
        let edit_keywords = [
            "fix this",
            "refactor this",
            "rename this",
            "rewrite this",
            "update this code",
            "change this",
            "modify this",
            "edit this",
            "replace this",
            "update this function",
            "update this method",
            "update this class",
            "change the function",
            "modify the code",
            "fix the function",
            "rename the variable",
            "replace the implementation",
            "inline edit",
        ];
        if input.prompt_tokens < 500
            && edit_keywords
                .iter()
                .any(|kw| user_lower.contains(kw))
        {
            *scores.entry(TaskType::CodeEdit).or_default() += 0.60;
            signals.push("inline_code_edit".into());
        }

        // Signal: no_strong_signals_fallback
        let max_score = scores.values().copied().fold(0.0_f64, f64::max);
        if max_score < 0.3 {
            *scores.entry(TaskType::Conversation).or_default() += 0.35;
            signals.push("no_strong_signals_fallback".into());
        }

        // Pick the best task type
        let (best_type, best_score) = scores
            .iter()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(t, s)| (*t, *s))
            .unwrap_or((TaskType::Unknown, 0.0));

        // Confidence: min(best_score / 1.5, 1.0)
        let confidence = (best_score / 1.5).min(1.0);

        // Return UNKNOWN if confidence too low
        let task_type = if confidence < 0.2 {
            TaskType::Unknown
        } else {
            best_type
        };

        ClassificationResult {
            task_type,
            confidence,
            signals,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_with_user_message(msg: &str) -> ClassifierInput {
        ClassifierInput {
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
            last_user_message: msg.into(),
            system_prompt_text: None,
        }
    }

    #[test]
    fn classify_code_generation() {
        let input = input_with_user_message("write a function to sort a list");
        let result = RulesClassifier::classify(&input);
        assert_eq!(result.task_type, TaskType::CodeGeneration);
        assert!(result.confidence > 0.3, "confidence={}", result.confidence);
    }

    #[test]
    fn classify_code_review() {
        let input = input_with_user_message("review this PR");
        let result = RulesClassifier::classify(&input);
        assert_eq!(result.task_type, TaskType::CodeReview);
    }

    #[test]
    fn classify_summarization() {
        let input = input_with_user_message("summarize this document for me");
        let result = RulesClassifier::classify(&input);
        assert_eq!(result.task_type, TaskType::Summarization);
    }

    #[test]
    fn classify_debugging() {
        let input = input_with_user_message("debug this error traceback");
        let result = RulesClassifier::classify(&input);
        assert_eq!(result.task_type, TaskType::Debugging);
    }

    #[test]
    fn classify_conversation_fallback() {
        let input = input_with_user_message("hello how are you");
        let result = RulesClassifier::classify(&input);
        assert_eq!(result.task_type, TaskType::Conversation);
    }

    #[test]
    fn classify_tool_signals() {
        let mut input = input_with_user_message("help me with something");
        input.has_tools = true;
        input.tool_count = 5;
        input.has_tool_calls = true;
        let result = RulesClassifier::classify(&input);
        assert_eq!(result.task_type, TaskType::ToolSelection);
    }

    #[test]
    fn classify_code_edit() {
        let mut input = input_with_user_message("refactor this to use an iterator instead");
        input.prompt_tokens = 120;
        let result = RulesClassifier::classify(&input);
        assert_eq!(result.task_type, TaskType::CodeEdit);
        assert!(result.confidence > 0.3, "confidence={}", result.confidence);
    }

    #[test]
    fn classify_code_fence_system_prompt() {
        let mut input = input_with_user_message("help me");
        input.has_code_fence_in_system = true;
        input.system_prompt_text = Some("You are a coding assistant. Use ``` for code.".into());
        let result = RulesClassifier::classify(&input);
        assert_eq!(result.task_type, TaskType::CodeGeneration);
    }

    #[test]
    fn classify_json_schema_output() {
        let mut input = input_with_user_message("categorize this text");
        input.has_json_schema = true;
        let result = RulesClassifier::classify(&input);
        // Should lean towards Classification or Extraction
        assert!(
            result.task_type == TaskType::Classification
                || result.task_type == TaskType::Extraction,
            "got {:?}",
            result.task_type
        );
    }
}
