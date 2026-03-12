use std::sync::Arc;

use rand::Rng;

use crate::config::Config;
use crate::providers::ProviderRegistry;
use crate::proxy::cost::compute_cost;
use crate::types::{ChatCompletionRequest, Message, MessageRole, TaskType};

pub struct Judge {
    pub judge_model: String,
}

pub struct JudgeResult {
    pub original_score: f64,
    pub benchmark_score: f64,
    pub judge_cost: f64,
}

impl Judge {
    pub fn new(judge_model: String) -> Self {
        Self { judge_model }
    }

    pub async fn score(
        &self,
        providers: &Arc<ProviderRegistry>,
        config: &Config,
        task_type: Option<TaskType>,
        messages: &[Message],
        original_completion: &str,
        benchmark_completion: &str,
    ) -> anyhow::Result<JudgeResult> {
        // Randomly swap A/B position to prevent position bias
        let swap: bool = rand::rng().random();

        let (completion_a, completion_b) = if swap {
            (benchmark_completion, original_completion)
        } else {
            (original_completion, benchmark_completion)
        };

        let judge_messages = build_judge_prompt(task_type, messages, completion_a, completion_b);

        // Resolve judge model to provider
        let (provider_name, model_id) =
            crate::proxy::handler::resolve_model(config, &self.judge_model)?;
        let provider = providers.get(&provider_name)?;

        let judge_request = ChatCompletionRequest {
            model: self.judge_model.clone(),
            messages: judge_messages,
            temperature: Some(0.0),
            top_p: None,
            max_tokens: Some(100),
            stream: false,
            stream_options: None,
            stop: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            user: None,
            extra: serde_json::Map::new(),
        };

        let response = match provider.chat_completion(&judge_request, &model_id).await? {
            crate::types::ProviderResponse::Complete(resp) => resp,
            _ => anyhow::bail!("expected non-streaming response from judge model"),
        };

        // Extract judge response text
        let text: String = response
            .choices
            .iter()
            .filter_map(|c| c.message.content.as_ref().and_then(|v| v.as_str()))
            .collect();

        let usage = response.usage.unwrap_or_default();
        let judge_cost = compute_cost(&self.judge_model, &usage);

        let (score_a, score_b) = parse_judge_response(&text);

        // Unswap scores back to original/benchmark order
        let (original_score, benchmark_score) = if swap {
            (score_b, score_a)
        } else {
            (score_a, score_b)
        };

        Ok(JudgeResult {
            original_score,
            benchmark_score,
            judge_cost,
        })
    }

    /// Score a single completion absolutely (0.0–1.0) without needing a comparison pair.
    /// Used by the live judge to evaluate real completions from `completion_samples`.
    pub async fn score_absolute(
        &self,
        providers: &Arc<ProviderRegistry>,
        config: &Config,
        task_type: Option<TaskType>,
        messages: &[Message],
        completion: &str,
    ) -> anyhow::Result<f64> {
        let rubric = task_rubric(task_type);

        let context: String = messages
            .iter()
            .filter_map(|m| {
                let content = m.content.as_ref()?.as_str()?;
                Some(format!("[{}]: {}", m.role, content))
            })
            .collect::<Vec<_>>()
            .join("\n");

        let system = "You are an expert evaluator. Rate the following completion on a scale \
                      from 0.0 to 1.0. Respond ONLY with JSON: {\"score\": <float>}. No other text."
            .to_string();

        let user = format!(
            "## Evaluation Criteria\n{rubric}\n\n\
             ## Context\n{context}\n\n\
             ## Completion\n{completion}\n\n\
             Rate the completion. Respond with JSON only."
        );

        let judge_messages = vec![
            Message {
                role: MessageRole::System,
                content: Some(serde_json::Value::String(system)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: serde_json::Map::new(),
            },
            Message {
                role: MessageRole::User,
                content: Some(serde_json::Value::String(user)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: serde_json::Map::new(),
            },
        ];

        let (provider_name, model_id) =
            crate::proxy::handler::resolve_model(config, &self.judge_model)?;
        let provider = providers.get(&provider_name)?;

        let judge_request = ChatCompletionRequest {
            model: self.judge_model.clone(),
            messages: judge_messages,
            temperature: Some(0.0),
            top_p: None,
            max_tokens: Some(50),
            stream: false,
            stream_options: None,
            stop: None,
            tools: None,
            tool_choice: None,
            response_format: None,
            user: None,
            extra: serde_json::Map::new(),
        };

        let response = match provider.chat_completion(&judge_request, &model_id).await? {
            crate::types::ProviderResponse::Complete(resp) => resp,
            _ => anyhow::bail!("expected non-streaming response from judge model"),
        };

        let text: String = response
            .choices
            .iter()
            .filter_map(|c| c.message.content.as_ref().and_then(|v| v.as_str()))
            .collect();

        Ok(parse_absolute_response(&text).unwrap_or(0.5))
    }
}

pub fn parse_absolute_response(text: &str) -> Option<f64> {
    let trimmed = text.trim();

    if let Some(score) = try_parse_score_from_json(trimmed) {
        return Some(score);
    }

    // Try to extract JSON object from surrounding text
    if let Some(start) = trimmed.find('{')
        && let Some(end) = trimmed[start..].find('}')
    {
        return try_parse_score_from_json(&trimmed[start..=start + end]);
    }

    None
}

fn try_parse_score_from_json(text: &str) -> Option<f64> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let score = v.get("score")?.as_f64()?;
    Some(score.clamp(0.0, 1.0))
}

fn build_judge_prompt(
    task_type: Option<TaskType>,
    messages: &[Message],
    completion_a: &str,
    completion_b: &str,
) -> Vec<Message> {
    let rubric = task_rubric(task_type);

    // Build context from the conversation messages
    let context: String = messages
        .iter()
        .filter_map(|m| {
            let content = m.content.as_ref()?.as_str()?;
            Some(format!("[{}]: {}", m.role, content))
        })
        .collect::<Vec<_>>()
        .join("\n");

    let system = "You are an expert evaluator. You will be given a conversation context \
                  and two completions (A and B). Score each completion on a scale from 0.0 to 1.0. \
                  Respond ONLY with a JSON object: {\"score_a\": <float>, \"score_b\": <float>}. \
                  No other text."
        .to_string();

    let user = format!(
        "## Evaluation Criteria\n{rubric}\n\n\
         ## Context\n{context}\n\n\
         ## Completion A\n{completion_a}\n\n\
         ## Completion B\n{completion_b}\n\n\
         Score each completion. Respond with JSON only."
    );

    vec![
        Message {
            role: MessageRole::System,
            content: Some(serde_json::Value::String(system)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: serde_json::Map::new(),
        },
        Message {
            role: MessageRole::User,
            content: Some(serde_json::Value::String(user)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: serde_json::Map::new(),
        },
    ]
}

pub fn parse_judge_response(text: &str) -> (f64, f64) {
    // Try to find JSON in the response (may have surrounding text)
    let trimmed = text.trim();

    // Try direct parse first
    if let Some(scores) = try_parse_scores(trimmed) {
        return scores;
    }

    // Try to find JSON object in the text
    if let Some(start) = trimmed.find('{')
        && let Some(end) = trimmed[start..].find('}')
        && let Some(scores) = try_parse_scores(&trimmed[start..=start + end])
    {
        return scores;
    }

    // Fallback
    (0.5, 0.5)
}

fn try_parse_scores(text: &str) -> Option<(f64, f64)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let a = v.get("score_a")?.as_f64()?;
    let b = v.get("score_b")?.as_f64()?;
    Some((a.clamp(0.0, 1.0), b.clamp(0.0, 1.0)))
}

fn task_rubric(task_type: Option<TaskType>) -> &'static str {
    match task_type {
        Some(TaskType::CodeGeneration) => {
            "Evaluate for: code correctness, efficiency, style, and completeness"
        }
        Some(TaskType::CodeReview) => {
            "Evaluate for: issue identification, actionable feedback, and accuracy"
        }
        Some(TaskType::Summarization) => {
            "Evaluate for: coverage of key points, conciseness, and factual accuracy"
        }
        Some(TaskType::Classification) => {
            "Evaluate for: correct categorization, confidence calibration, and explanation quality"
        }
        Some(TaskType::Extraction) => {
            "Evaluate for: completeness, accuracy of extracted data, and proper formatting"
        }
        Some(TaskType::Translation) => {
            "Evaluate for: accuracy, fluency, preservation of meaning, and natural phrasing"
        }
        Some(TaskType::QuestionAnswering) => {
            "Evaluate for: factual accuracy, completeness, and relevance to the question"
        }
        Some(TaskType::CreativeWriting) => {
            "Evaluate for: creativity, coherence, engagement, and adherence to constraints"
        }
        Some(TaskType::Reasoning) => {
            "Evaluate for: logical soundness, step-by-step clarity, and correctness of conclusion"
        }
        Some(TaskType::Conversation) => {
            "Evaluate for: helpfulness, coherence, and natural conversational flow"
        }
        Some(TaskType::ToolUse | TaskType::ToolSelection) => {
            "Evaluate for: correct tool selection, proper parameter usage, and result interpretation"
        }
        Some(TaskType::Architecture) => {
            "Evaluate for: design soundness, scalability considerations, and practical feasibility"
        }
        Some(TaskType::Debugging) => {
            "Evaluate for: root cause identification, fix correctness, and explanation clarity"
        }
        Some(TaskType::Refactoring) => {
            "Evaluate for: code improvement, maintained functionality, and readability gains"
        }
        Some(TaskType::Documentation) => {
            "Evaluate for: clarity, completeness, accuracy, and proper formatting"
        }
        Some(TaskType::Testing) => {
            "Evaluate for: test coverage, edge case handling, and assertion quality"
        }
        _ => "Evaluate for: relevance, accuracy, completeness, and clarity",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_json() {
        let (a, b) = parse_judge_response(r#"{"score_a": 0.85, "score_b": 0.72}"#);
        assert!((a - 0.85).abs() < f64::EPSILON);
        assert!((b - 0.72).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_invalid_falls_back_to_half() {
        let (a, b) = parse_judge_response("I think A is better");
        assert!((a - 0.5).abs() < f64::EPSILON);
        assert!((b - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_clamps_out_of_range() {
        let (a, b) = parse_judge_response(r#"{"score_a": 1.5, "score_b": -0.3}"#);
        assert!((a - 1.0).abs() < f64::EPSILON);
        assert!((b - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn judge_prompt_includes_rubric() {
        let messages = vec![Message {
            role: MessageRole::User,
            content: Some(serde_json::Value::String("Write hello world".to_string())),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: serde_json::Map::new(),
        }];

        let prompt = build_judge_prompt(
            Some(TaskType::CodeGeneration),
            &messages,
            "print('hello')",
            "console.log('hello')",
        );

        let user_content = prompt[1].content.as_ref().unwrap().as_str().unwrap();
        assert!(user_content.contains("code correctness"));
        assert!(user_content.contains("Completion A"));
        assert!(user_content.contains("Completion B"));
    }

    #[test]
    fn parse_absolute_valid() {
        let score = parse_absolute_response(r#"{"score": 0.75}"#);
        assert!(score.is_some());
        assert!((score.unwrap() - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_absolute_invalid_falls_back() {
        assert!(parse_absolute_response("not json at all").is_none());
        assert!(parse_absolute_response("{}").is_none());
    }

    #[test]
    fn parse_absolute_clamps_out_of_range() {
        let score = parse_absolute_response(r#"{"score": 1.8}"#);
        assert_eq!(score, Some(1.0));
        let score = parse_absolute_response(r#"{"score": -0.5}"#);
        assert_eq!(score, Some(0.0));
    }

    #[test]
    fn parse_absolute_extracts_from_surrounding_text() {
        let text = r#"Here is my evaluation: {"score": 0.9} That's my score."#;
        let score = parse_absolute_response(text);
        assert!(score.is_some());
        assert!((score.unwrap() - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn position_debiasing() {
        // Verify the unswap logic is correct

        // Case 1: swap = false (no swap)
        let (original_score, benchmark_score): (f64, f64) = {
            let swap = false;
            let (completion_a, completion_b) = if swap {
                ("benchmark", "original")
            } else {
                ("original", "benchmark")
            };
            assert_eq!(completion_a, "original");
            assert_eq!(completion_b, "benchmark");

            let (score_a, score_b): (f64, f64) = (0.9, 0.7);
            if swap {
                (score_b, score_a)
            } else {
                (score_a, score_b)
            }
        };
        assert!((original_score - 0.9).abs() < f64::EPSILON);
        assert!((benchmark_score - 0.7).abs() < f64::EPSILON);

        // Case 2: swap = true
        let (original_score, benchmark_score): (f64, f64) = {
            let swap = true;
            let (completion_a, completion_b) = if swap {
                ("benchmark", "original")
            } else {
                ("original", "benchmark")
            };
            assert_eq!(completion_a, "benchmark");
            assert_eq!(completion_b, "original");

            let (score_a, score_b): (f64, f64) = (0.8, 0.6);
            // After unswap: original = score_b, benchmark = score_a
            if swap {
                (score_b, score_a)
            } else {
                (score_a, score_b)
            }
        };
        assert!((original_score - 0.6).abs() < f64::EPSILON);
        assert!((benchmark_score - 0.8).abs() < f64::EPSILON);
    }
}
