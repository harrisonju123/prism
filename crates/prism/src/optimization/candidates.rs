use crate::optimization::types::PromptCandidate;

/// Build the meta-prompt that asks the optimizer model to generate candidate prompts.
pub fn build_generation_prompt(
    original_prompt: &str,
    failure_patterns: &[String],
    num_candidates: usize,
) -> String {
    let failures = if failure_patterns.is_empty() {
        "No specific failure patterns identified.".to_string()
    } else {
        failure_patterns
            .iter()
            .enumerate()
            .map(|(i, f)| format!("{}. {}", i + 1, f))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        r#"You are an expert prompt engineer. Your task is to improve the following prompt template.

ORIGINAL PROMPT:
{original_prompt}

IDENTIFIED ISSUES:
{failures}

Generate exactly {num_candidates} improved versions of this prompt. For each version:
1. Address the identified issues
2. Maintain the original intent
3. Be specific and clear in instructions

Return your response as a JSON array where each element has:
- "content": the improved prompt text
- "rationale": why this version is better

Example format:
[
  {{"content": "improved prompt...", "rationale": "addresses X by..."}}
]"#
    )
}

/// Parse candidate prompts from the optimizer model's JSON response.
pub fn parse_candidates(response: &str) -> Vec<PromptCandidate> {
    // Try to extract JSON array from the response
    let json_str = extract_json_array(response);

    let parsed: Vec<serde_json::Value> = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    parsed
        .into_iter()
        .enumerate()
        .map(|(i, v)| PromptCandidate {
            index: i,
            content: v
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .to_string(),
            rationale: v
                .get("rationale")
                .and_then(|r| r.as_str())
                .unwrap_or("")
                .to_string(),
            score: None,
            eval_count: 0,
        })
        .filter(|c| !c.content.is_empty())
        .collect()
}

/// Build an evaluation prompt to score a candidate.
pub fn build_eval_prompt(candidate: &str, test_input: &str) -> String {
    format!(
        r#"Evaluate the quality of this prompt when applied to the given input.

PROMPT TEMPLATE:
{candidate}

TEST INPUT:
{test_input}

Rate the prompt on a scale of 1-10 considering:
- Clarity of instructions
- Specificity
- Likely output quality
- Edge case handling

Return ONLY a JSON object: {{"score": <number>, "reasoning": "<brief explanation>"}}"#
    )
}

/// Extract a JSON array from a response that may contain markdown code fences or extra text.
fn extract_json_array(text: &str) -> String {
    // Try to find JSON array between code fences
    if let Some(start) = text.find("```json") {
        let after_fence = &text[start + 7..];
        if let Some(end) = after_fence.find("```") {
            return after_fence[..end].trim().to_string();
        }
    }
    if let Some(start) = text.find("```") {
        let after_fence = &text[start + 3..];
        if let Some(end) = after_fence.find("```") {
            return after_fence[..end].trim().to_string();
        }
    }

    // Try to find raw JSON array
    if let Some(start) = text.find('[') {
        if let Some(end) = text.rfind(']') {
            return text[start..=end].to_string();
        }
    }

    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_generation_prompt_includes_original() {
        let prompt = build_generation_prompt("Summarize this: {{input}}", &[], 3);
        assert!(prompt.contains("Summarize this: {{input}}"));
        assert!(prompt.contains("3"));
    }

    #[test]
    fn build_generation_prompt_includes_failures() {
        let failures = vec!["Too verbose".into(), "Misses key points".into()];
        let prompt = build_generation_prompt("Summarize", &failures, 2);
        assert!(prompt.contains("Too verbose"));
        assert!(prompt.contains("Misses key points"));
    }

    #[test]
    fn parse_candidates_from_json() {
        let response = r#"[
            {"content": "Version 1", "rationale": "Better clarity"},
            {"content": "Version 2", "rationale": "More specific"}
        ]"#;
        let candidates = parse_candidates(response);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].content, "Version 1");
        assert_eq!(candidates[1].rationale, "More specific");
    }

    #[test]
    fn parse_candidates_from_fenced_json() {
        let response = "Here are the candidates:\n```json\n[\n{\"content\": \"V1\", \"rationale\": \"R1\"}\n]\n```";
        let candidates = parse_candidates(response);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].content, "V1");
    }

    #[test]
    fn parse_candidates_invalid_json() {
        let candidates = parse_candidates("not json at all");
        assert!(candidates.is_empty());
    }

    #[test]
    fn parse_candidates_skips_empty_content() {
        let response =
            r#"[{"content": "", "rationale": "empty"}, {"content": "good", "rationale": "ok"}]"#;
        let candidates = parse_candidates(response);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].content, "good");
    }

    #[test]
    fn build_eval_prompt_includes_candidate() {
        let prompt = build_eval_prompt("Test prompt", "sample input");
        assert!(prompt.contains("Test prompt"));
        assert!(prompt.contains("sample input"));
    }
}
