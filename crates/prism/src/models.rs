use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub provider: &'static str,
    pub model_id: &'static str,
    pub display_name: &'static str,
    pub context_window: u32,
    pub max_output_tokens: u32,
    pub input_cost_per_1m: f64,  // USD per 1M input tokens
    pub output_cost_per_1m: f64, // USD per 1M output tokens
    pub tier: u8,                // 1=premium, 2=standard, 3=economy
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    /// Per-task quality scores (0.0–1.0). Key is TaskType snake_case string.
    #[serde(skip)]
    pub task_qualities: &'static [(&'static str, f64)],
    /// Next-cheaper model to downgrade to when budget is tight.
    #[serde(skip)]
    pub downgrade_to: Option<&'static str>,
}

impl ModelInfo {
    pub fn input_cost_per_token(&self) -> f64 {
        self.input_cost_per_1m / 1_000_000.0
    }

    pub fn output_cost_per_token(&self) -> f64 {
        self.output_cost_per_1m / 1_000_000.0
    }

    pub fn estimate_cost(&self, input_tokens: u32, output_tokens: u32) -> f64 {
        (input_tokens as f64 * self.input_cost_per_token())
            + (output_tokens as f64 * self.output_cost_per_token())
    }

    /// Get quality score for a specific task type, falling back to tier-based default.
    pub fn quality_for_task(&self, task_type: &str) -> f64 {
        for &(t, q) in self.task_qualities {
            if t == task_type {
                return q;
            }
        }
        // Tier-based defaults
        match self.tier {
            1 => 0.90,
            2 => 0.75,
            _ => 0.60,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-model task quality scores (ported from BlockWorks)
// ---------------------------------------------------------------------------

static OPUS_4_QUALITIES: &[(&str, f64)] = &[
    ("classification", 0.96),
    ("code_generation", 0.93),
    ("code_review", 0.91),
    ("conversation", 0.95),
    ("extraction", 0.94),
    ("reasoning", 0.93),
    ("summarization", 0.94),
    ("tool_selection", 0.92),
    ("architecture", 0.95),
    ("debugging", 0.94),
    ("refactoring", 0.90),
    ("documentation", 0.88),
    ("testing", 0.89),
];

static SONNET_4_QUALITIES: &[(&str, f64)] = &[
    ("classification", 0.88),
    ("code_generation", 0.85),
    ("code_review", 0.83),
    ("conversation", 0.87),
    ("extraction", 0.86),
    ("reasoning", 0.82),
    ("summarization", 0.86),
    ("tool_selection", 0.84),
    ("architecture", 0.83),
    ("debugging", 0.84),
    ("refactoring", 0.82),
    ("documentation", 0.80),
    ("testing", 0.81),
];

static HAIKU_35_QUALITIES: &[(&str, f64)] = &[
    ("classification", 0.78),
    ("code_generation", 0.70),
    ("code_review", 0.68),
    ("conversation", 0.76),
    ("extraction", 0.75),
    ("reasoning", 0.65),
    ("summarization", 0.74),
    ("tool_selection", 0.70),
    ("architecture", 0.62),
    ("debugging", 0.66),
    ("refactoring", 0.64),
    ("documentation", 0.72),
    ("testing", 0.65),
];

static GPT4O_QUALITIES: &[(&str, f64)] = &[
    ("classification", 0.87),
    ("code_generation", 0.84),
    ("code_review", 0.82),
    ("conversation", 0.86),
    ("extraction", 0.85),
    ("reasoning", 0.83),
    ("summarization", 0.85),
    ("tool_selection", 0.83),
    ("architecture", 0.82),
    ("debugging", 0.83),
    ("refactoring", 0.80),
    ("documentation", 0.79),
    ("testing", 0.80),
];

static GPT4O_MINI_QUALITIES: &[(&str, f64)] = &[
    ("classification", 0.76),
    ("code_generation", 0.68),
    ("code_review", 0.66),
    ("conversation", 0.74),
    ("extraction", 0.73),
    ("reasoning", 0.62),
    ("summarization", 0.72),
    ("tool_selection", 0.68),
    ("architecture", 0.60),
    ("debugging", 0.64),
    ("refactoring", 0.62),
    ("documentation", 0.70),
    ("testing", 0.63),
];

static O1_QUALITIES: &[(&str, f64)] = &[
    ("classification", 0.92),
    ("code_generation", 0.94),
    ("code_review", 0.90),
    ("conversation", 0.85),
    ("extraction", 0.88),
    ("reasoning", 0.97),
    ("summarization", 0.87),
    ("tool_selection", 0.86),
    ("architecture", 0.93),
    ("debugging", 0.95),
    ("refactoring", 0.91),
    ("documentation", 0.84),
    ("testing", 0.92),
];

static GEMINI_25_PRO_QUALITIES: &[(&str, f64)] = &[
    ("classification", 0.86),
    ("code_generation", 0.83),
    ("code_review", 0.80),
    ("conversation", 0.84),
    ("extraction", 0.85),
    ("reasoning", 0.84),
    ("summarization", 0.84),
    ("tool_selection", 0.81),
    ("architecture", 0.81),
    ("debugging", 0.82),
    ("refactoring", 0.79),
    ("documentation", 0.78),
    ("testing", 0.79),
];

static GEMINI_20_FLASH_QUALITIES: &[(&str, f64)] = &[
    ("classification", 0.74),
    ("code_generation", 0.66),
    ("code_review", 0.64),
    ("conversation", 0.72),
    ("extraction", 0.71),
    ("reasoning", 0.60),
    ("summarization", 0.70),
    ("tool_selection", 0.66),
    ("architecture", 0.58),
    ("debugging", 0.62),
    ("refactoring", 0.60),
    ("documentation", 0.68),
    ("testing", 0.61),
];

static MISTRAL_LARGE_QUALITIES: &[(&str, f64)] = &[
    ("classification", 0.84),
    ("code_generation", 0.80),
    ("code_review", 0.78),
    ("conversation", 0.82),
    ("extraction", 0.83),
    ("reasoning", 0.79),
    ("summarization", 0.82),
    ("tool_selection", 0.78),
    ("architecture", 0.77),
    ("debugging", 0.78),
    ("refactoring", 0.76),
    ("documentation", 0.76),
    ("testing", 0.76),
];

static MISTRAL_SMALL_QUALITIES: &[(&str, f64)] = &[
    ("classification", 0.72),
    ("code_generation", 0.64),
    ("code_review", 0.62),
    ("conversation", 0.70),
    ("extraction", 0.69),
    ("reasoning", 0.58),
    ("summarization", 0.68),
    ("tool_selection", 0.64),
    ("architecture", 0.56),
    ("debugging", 0.60),
    ("refactoring", 0.58),
    ("documentation", 0.66),
    ("testing", 0.59),
];

/// Semantic aliases map human-friendly names to concrete models.
pub static SEMANTIC_ALIASES: &[(&str, &str)] = &[
    ("fast", "claude-3-5-haiku-latest"),
    ("smart", "claude-sonnet-4-6"),
    ("cheap", "claude-3-5-haiku-latest"),
    ("balanced", "claude-sonnet-4-6"),
    ("powerful", "claude-opus-4-6"),
];

pub fn resolve_alias(name: &str) -> Option<&'static str> {
    SEMANTIC_ALIASES
        .iter()
        .find(|(k, _)| *k == name)
        .map(|(_, v)| *v)
}

/// Static model catalog with pricing and capabilities.
/// Pricing as of early 2026 — update as needed.
pub static MODEL_CATALOG: LazyLock<HashMap<&'static str, ModelInfo>> = LazyLock::new(|| {
    let models = vec![
        // =====================================================================
        // Anthropic
        // =====================================================================
        (
            "claude-opus-4",
            ModelInfo {
                provider: "anthropic",
                model_id: "claude-opus-4-20250514",
                display_name: "Claude Opus 4",
                context_window: 200_000,
                max_output_tokens: 32_000,
                input_cost_per_1m: 15.0,
                output_cost_per_1m: 75.0,
                tier: 1,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: OPUS_4_QUALITIES,
                downgrade_to: Some("claude-sonnet-4"),
            },
        ),
        (
            "claude-sonnet-4",
            ModelInfo {
                provider: "anthropic",
                model_id: "claude-sonnet-4-20250514",
                display_name: "Claude Sonnet 4",
                context_window: 200_000,
                max_output_tokens: 16_000,
                input_cost_per_1m: 3.0,
                output_cost_per_1m: 15.0,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: SONNET_4_QUALITIES,
                downgrade_to: Some("claude-haiku-3.5"),
            },
        ),
        (
            "claude-haiku-3.5",
            ModelInfo {
                provider: "anthropic",
                model_id: "claude-3-5-haiku-20241022",
                display_name: "Claude 3.5 Haiku",
                context_window: 200_000,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.80,
                output_cost_per_1m: 4.0,
                tier: 3,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: HAIKU_35_QUALITIES,
                downgrade_to: None,
            },
        ),
        (
            "claude-3.5-sonnet",
            ModelInfo {
                provider: "anthropic",
                model_id: "claude-3-5-sonnet-20241022",
                display_name: "Claude 3.5 Sonnet",
                context_window: 200_000,
                max_output_tokens: 8_192,
                input_cost_per_1m: 3.0,
                output_cost_per_1m: 15.0,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: SONNET_4_QUALITIES, // similar capabilities
                downgrade_to: Some("claude-haiku-3.5"),
            },
        ),
        // =====================================================================
        // OpenAI
        // =====================================================================
        (
            "gpt-4o",
            ModelInfo {
                provider: "openai",
                model_id: "gpt-4o",
                display_name: "GPT-4o",
                context_window: 128_000,
                max_output_tokens: 16_384,
                input_cost_per_1m: 2.50,
                output_cost_per_1m: 10.0,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: GPT4O_QUALITIES,
                downgrade_to: Some("gpt-4o-mini"),
            },
        ),
        (
            "gpt-4o-mini",
            ModelInfo {
                provider: "openai",
                model_id: "gpt-4o-mini",
                display_name: "GPT-4o Mini",
                context_window: 128_000,
                max_output_tokens: 16_384,
                input_cost_per_1m: 0.15,
                output_cost_per_1m: 0.60,
                tier: 3,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: GPT4O_MINI_QUALITIES,
                downgrade_to: None,
            },
        ),
        (
            "o1",
            ModelInfo {
                provider: "openai",
                model_id: "o1",
                display_name: "o1",
                context_window: 200_000,
                max_output_tokens: 100_000,
                input_cost_per_1m: 15.0,
                output_cost_per_1m: 60.0,
                tier: 1,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: O1_QUALITIES,
                downgrade_to: Some("gpt-4o"),
            },
        ),
        (
            "o1-mini",
            ModelInfo {
                provider: "openai",
                model_id: "o1-mini",
                display_name: "o1 Mini",
                context_window: 128_000,
                max_output_tokens: 65_536,
                input_cost_per_1m: 3.0,
                output_cost_per_1m: 12.0,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                task_qualities: &[
                    ("reasoning", 0.90),
                    ("code_generation", 0.86),
                    ("debugging", 0.87),
                    ("testing", 0.85),
                ],
                downgrade_to: Some("gpt-4o-mini"),
            },
        ),
        (
            "o3-mini",
            ModelInfo {
                provider: "openai",
                model_id: "o3-mini",
                display_name: "o3 Mini",
                context_window: 200_000,
                max_output_tokens: 100_000,
                input_cost_per_1m: 1.10,
                output_cost_per_1m: 4.40,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                task_qualities: &[
                    ("reasoning", 0.92),
                    ("code_generation", 0.88),
                    ("debugging", 0.89),
                    ("testing", 0.87),
                ],
                downgrade_to: Some("gpt-4o-mini"),
            },
        ),
        (
            "gpt-4-turbo",
            ModelInfo {
                provider: "openai",
                model_id: "gpt-4-turbo",
                display_name: "GPT-4 Turbo",
                context_window: 128_000,
                max_output_tokens: 4_096,
                input_cost_per_1m: 10.0,
                output_cost_per_1m: 30.0,
                tier: 1,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: &[],
                downgrade_to: Some("gpt-4o"),
            },
        ),
        // =====================================================================
        // Google (Gemini)
        // =====================================================================
        (
            "gemini-2.0-flash",
            ModelInfo {
                provider: "google",
                model_id: "gemini-2.0-flash",
                display_name: "Gemini 2.0 Flash",
                context_window: 1_048_576,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.10,
                output_cost_per_1m: 0.40,
                tier: 3,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: GEMINI_20_FLASH_QUALITIES,
                downgrade_to: None,
            },
        ),
        (
            "gemini-2.5-pro",
            ModelInfo {
                provider: "google",
                model_id: "gemini-2.5-pro-preview-03-25",
                display_name: "Gemini 2.5 Pro",
                context_window: 1_048_576,
                max_output_tokens: 65_536,
                input_cost_per_1m: 1.25,
                output_cost_per_1m: 10.0,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: GEMINI_25_PRO_QUALITIES,
                downgrade_to: Some("gemini-2.0-flash"),
            },
        ),
        (
            "gemini-2.5-flash",
            ModelInfo {
                provider: "google",
                model_id: "gemini-2.5-flash-preview-04-17",
                display_name: "Gemini 2.5 Flash",
                context_window: 1_048_576,
                max_output_tokens: 65_536,
                input_cost_per_1m: 0.15,
                output_cost_per_1m: 0.60,
                tier: 3,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: &[
                    ("classification", 0.76),
                    ("code_generation", 0.70),
                    ("conversation", 0.74),
                    ("reasoning", 0.68),
                    ("summarization", 0.72),
                ],
                downgrade_to: None,
            },
        ),
        (
            "gemini-1.5-pro",
            ModelInfo {
                provider: "google",
                model_id: "gemini-1.5-pro",
                display_name: "Gemini 1.5 Pro",
                context_window: 2_097_152,
                max_output_tokens: 8_192,
                input_cost_per_1m: 1.25,
                output_cost_per_1m: 5.0,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: &[],
                downgrade_to: Some("gemini-2.0-flash"),
            },
        ),
        (
            "gemini-1.5-flash",
            ModelInfo {
                provider: "google",
                model_id: "gemini-1.5-flash",
                display_name: "Gemini 1.5 Flash",
                context_window: 1_048_576,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.075,
                output_cost_per_1m: 0.30,
                tier: 3,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: &[],
                downgrade_to: None,
            },
        ),
        // =====================================================================
        // Mistral
        // =====================================================================
        (
            "mistral-large",
            ModelInfo {
                provider: "mistral",
                model_id: "mistral-large-latest",
                display_name: "Mistral Large",
                context_window: 128_000,
                max_output_tokens: 8_192,
                input_cost_per_1m: 2.0,
                output_cost_per_1m: 6.0,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                task_qualities: MISTRAL_LARGE_QUALITIES,
                downgrade_to: Some("mistral-small"),
            },
        ),
        (
            "mistral-small",
            ModelInfo {
                provider: "mistral",
                model_id: "mistral-small-latest",
                display_name: "Mistral Small",
                context_window: 128_000,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.10,
                output_cost_per_1m: 0.30,
                tier: 3,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                task_qualities: MISTRAL_SMALL_QUALITIES,
                downgrade_to: None,
            },
        ),
        (
            "codestral",
            ModelInfo {
                provider: "mistral",
                model_id: "codestral-latest",
                display_name: "Codestral",
                context_window: 32_000,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.30,
                output_cost_per_1m: 0.90,
                tier: 2,
                supports_streaming: true,
                supports_tools: false,
                supports_vision: false,
                task_qualities: &[
                    ("code_generation", 0.86),
                    ("code_review", 0.82),
                    ("debugging", 0.83),
                    ("refactoring", 0.80),
                    ("testing", 0.81),
                ],
                downgrade_to: Some("mistral-small"),
            },
        ),
        (
            "mistral-nemo",
            ModelInfo {
                provider: "mistral",
                model_id: "open-mistral-nemo",
                display_name: "Mistral Nemo",
                context_window: 128_000,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.15,
                output_cost_per_1m: 0.15,
                tier: 3,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                task_qualities: &[],
                downgrade_to: None,
            },
        ),
        // =====================================================================
        // Groq (hosted open-source, ultra-low latency)
        // =====================================================================
        (
            "groq-llama-3.3-70b",
            ModelInfo {
                provider: "groq",
                model_id: "llama-3.3-70b-versatile",
                display_name: "Llama 3.3 70B (Groq)",
                context_window: 128_000,
                max_output_tokens: 32_768,
                input_cost_per_1m: 0.59,
                output_cost_per_1m: 0.79,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                task_qualities: &[
                    ("classification", 0.82),
                    ("code_generation", 0.78),
                    ("conversation", 0.80),
                    ("reasoning", 0.76),
                    ("summarization", 0.80),
                ],
                downgrade_to: Some("groq-llama-3.1-8b"),
            },
        ),
        (
            "groq-llama-3.1-8b",
            ModelInfo {
                provider: "groq",
                model_id: "llama-3.1-8b-instant",
                display_name: "Llama 3.1 8B (Groq)",
                context_window: 128_000,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.05,
                output_cost_per_1m: 0.08,
                tier: 3,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                task_qualities: &[
                    ("classification", 0.68),
                    ("conversation", 0.66),
                    ("summarization", 0.64),
                ],
                downgrade_to: None,
            },
        ),
        (
            "groq-mixtral-8x7b",
            ModelInfo {
                provider: "groq",
                model_id: "mixtral-8x7b-32768",
                display_name: "Mixtral 8x7B (Groq)",
                context_window: 32_768,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.24,
                output_cost_per_1m: 0.24,
                tier: 3,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                task_qualities: &[],
                downgrade_to: None,
            },
        ),
        (
            "groq-gemma2-9b",
            ModelInfo {
                provider: "groq",
                model_id: "gemma2-9b-it",
                display_name: "Gemma 2 9B (Groq)",
                context_window: 8_192,
                max_output_tokens: 4_096,
                input_cost_per_1m: 0.20,
                output_cost_per_1m: 0.20,
                tier: 3,
                supports_streaming: true,
                supports_tools: false,
                supports_vision: false,
                task_qualities: &[],
                downgrade_to: None,
            },
        ),
        // =====================================================================
        // DeepSeek (OpenAI-compatible)
        // =====================================================================
        (
            "deepseek-chat",
            ModelInfo {
                provider: "deepseek",
                model_id: "deepseek-chat",
                display_name: "DeepSeek V3",
                context_window: 128_000,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.27,
                output_cost_per_1m: 1.10,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                task_qualities: &[
                    ("code_generation", 0.82),
                    ("reasoning", 0.80),
                    ("debugging", 0.80),
                    ("conversation", 0.78),
                ],
                downgrade_to: None,
            },
        ),
        (
            "deepseek-reasoner",
            ModelInfo {
                provider: "deepseek",
                model_id: "deepseek-reasoner",
                display_name: "DeepSeek R1",
                context_window: 128_000,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.55,
                output_cost_per_1m: 2.19,
                tier: 2,
                supports_streaming: true,
                supports_tools: false,
                supports_vision: false,
                task_qualities: &[
                    ("reasoning", 0.92),
                    ("code_generation", 0.86),
                    ("debugging", 0.88),
                    ("architecture", 0.85),
                ],
                downgrade_to: Some("deepseek-chat"),
            },
        ),
        // =====================================================================
        // Together AI (hosted open-source, OpenAI-compatible)
        // =====================================================================
        (
            "together-qwen-72b",
            ModelInfo {
                provider: "together",
                model_id: "Qwen/Qwen2.5-72B-Instruct-Turbo",
                display_name: "Qwen 2.5 72B (Together)",
                context_window: 32_768,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.60,
                output_cost_per_1m: 0.60,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                task_qualities: &[
                    ("code_generation", 0.80),
                    ("reasoning", 0.78),
                    ("conversation", 0.76),
                ],
                downgrade_to: None,
            },
        ),
        (
            "together-llama-3.3-70b",
            ModelInfo {
                provider: "together",
                model_id: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
                display_name: "Llama 3.3 70B (Together)",
                context_window: 128_000,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.88,
                output_cost_per_1m: 0.88,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                task_qualities: &[
                    ("classification", 0.82),
                    ("code_generation", 0.78),
                    ("conversation", 0.80),
                ],
                downgrade_to: None,
            },
        ),
        // =====================================================================
        // Additional models for broader coverage
        // =====================================================================
        (
            "claude-3-opus",
            ModelInfo {
                provider: "anthropic",
                model_id: "claude-3-opus-20240229",
                display_name: "Claude 3 Opus",
                context_window: 200_000,
                max_output_tokens: 4_096,
                input_cost_per_1m: 15.0,
                output_cost_per_1m: 75.0,
                tier: 1,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: &[],
                downgrade_to: Some("claude-sonnet-4"),
            },
        ),
        (
            "groq-llama-3.3-70b-specdec",
            ModelInfo {
                provider: "groq",
                model_id: "llama-3.3-70b-specdec",
                display_name: "Llama 3.3 70B SpecDec (Groq)",
                context_window: 8_192,
                max_output_tokens: 4_096,
                input_cost_per_1m: 0.59,
                output_cost_per_1m: 0.99,
                tier: 2,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: false,
                task_qualities: &[],
                downgrade_to: Some("groq-llama-3.1-8b"),
            },
        ),
        (
            "gemini-2.0-flash-lite",
            ModelInfo {
                provider: "google",
                model_id: "gemini-2.0-flash-lite",
                display_name: "Gemini 2.0 Flash Lite",
                context_window: 1_048_576,
                max_output_tokens: 8_192,
                input_cost_per_1m: 0.075,
                output_cost_per_1m: 0.30,
                tier: 3,
                supports_streaming: true,
                supports_tools: true,
                supports_vision: true,
                task_qualities: &[],
                downgrade_to: None,
            },
        ),
    ];

    models.into_iter().collect()
});

/// Look up model info. First checks the static catalog, then falls back
/// to matching by model_id prefix for versioned model names.
pub fn lookup_model(name: &str) -> Option<&'static ModelInfo> {
    MODEL_CATALOG.get(name).or_else(|| {
        MODEL_CATALOG
            .values()
            .find(|m| name.starts_with(m.model_id))
    })
}

/// Infer provider name from a model string.
pub fn infer_provider(model: &str) -> &'static str {
    if model.starts_with("claude") || model.starts_with("anthropic") {
        "anthropic"
    } else if model.starts_with("gpt") || model.starts_with("o1") || model.starts_with("o3") {
        "openai"
    } else if model.starts_with("gemini") {
        "google"
    } else if model.starts_with("mistral")
        || model.starts_with("codestral")
        || model.starts_with("open-mistral")
    {
        "mistral"
    } else if model.starts_with("groq-")
        || model.starts_with("llama")
        || model.starts_with("mixtral")
        || model.starts_with("gemma")
    {
        "groq"
    } else if model.starts_with("deepseek") {
        "deepseek"
    } else if model.starts_with("together-")
        || model.contains("Qwen")
        || model.contains("meta-llama")
    {
        "together"
    } else if model.contains("bedrock") || model.contains("amazon") {
        "bedrock"
    } else {
        "openai" // default: assume OpenAI-compatible
    }
}

/// Estimate cost for a model by name.
pub fn estimate_cost(model: &str, input_tokens: u32, output_tokens: u32) -> f64 {
    lookup_model(model)
        .map(|m| m.estimate_cost(input_tokens, output_tokens))
        .unwrap_or(0.0)
}

/// Get the downgrade chain for a model (for budget-triggered fallbacks).
pub fn downgrade_chain(model: &str) -> Vec<&'static str> {
    let mut chain = Vec::new();
    let mut current = model;
    while let Some(info) = lookup_model(current) {
        if let Some(next) = info.downgrade_to {
            chain.push(next);
            current = next;
        } else {
            break;
        }
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_30_plus_models() {
        assert!(
            MODEL_CATALOG.len() >= 30,
            "catalog has {} models",
            MODEL_CATALOG.len()
        );
    }

    #[test]
    fn all_models_have_valid_tiers() {
        for (name, info) in MODEL_CATALOG.iter() {
            assert!(
                info.tier >= 1 && info.tier <= 3,
                "model {name} has invalid tier {}",
                info.tier
            );
        }
    }

    #[test]
    fn downgrade_chains_are_valid() {
        for (name, info) in MODEL_CATALOG.iter() {
            if let Some(target) = info.downgrade_to {
                assert!(
                    lookup_model(target).is_some(),
                    "model {name} downgrades to unknown model {target}"
                );
            }
        }
    }

    #[test]
    fn downgrade_chain_opus_to_haiku() {
        let chain = downgrade_chain("claude-opus-4");
        assert_eq!(chain, vec!["claude-sonnet-4", "claude-haiku-3.5"]);
    }

    #[test]
    fn quality_for_task_returns_specific_score() {
        let opus = lookup_model("claude-opus-4").unwrap();
        let q = opus.quality_for_task("reasoning");
        assert!((q - 0.93).abs() < f64::EPSILON);
    }

    #[test]
    fn quality_for_task_returns_tier_default() {
        let opus = lookup_model("claude-opus-4").unwrap();
        let q = opus.quality_for_task("nonexistent_task");
        assert!((q - 0.90).abs() < f64::EPSILON); // tier 1 default
    }

    #[test]
    fn infer_provider_groq() {
        assert_eq!(infer_provider("groq-llama-3.3-70b"), "groq");
        assert_eq!(infer_provider("llama-3.1-8b-instant"), "groq");
    }

    #[test]
    fn infer_provider_deepseek() {
        assert_eq!(infer_provider("deepseek-chat"), "deepseek");
    }

    #[test]
    fn lookup_by_model_id_prefix() {
        let info = lookup_model("claude-opus-4-20250514").unwrap();
        assert_eq!(info.display_name, "Claude Opus 4");
    }

    #[test]
    fn test_resolve_alias() {
        assert_eq!(resolve_alias("fast"), Some("claude-3-5-haiku-latest"));
        assert_eq!(resolve_alias("smart"), Some("claude-sonnet-4-6"));
        assert_eq!(resolve_alias("cheap"), Some("claude-3-5-haiku-latest"));
        assert_eq!(resolve_alias("balanced"), Some("claude-sonnet-4-6"));
        assert_eq!(resolve_alias("powerful"), Some("claude-opus-4-6"));
        assert_eq!(resolve_alias("unknown"), None);
    }
}
