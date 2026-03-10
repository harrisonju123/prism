use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// OpenAI-compatible request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Extra fields we pass through without interpreting
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamOptions {
    #[serde(default)]
    pub include_usage: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
    #[serde(other)]
    Unknown,
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::System => f.write_str("system"),
            Self::User => f.write_str("user"),
            Self::Assistant => f.write_str("assistant"),
            Self::Tool => f.write_str("tool"),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    pub r#type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: u32,
    pub message: Message,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    /// Anthropic prompt cache: tokens read from cache (90% cheaper).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_read_input_tokens: u32,
    /// Anthropic prompt cache: tokens written to cache (25% more expensive).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_creation_input_tokens: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

// ---------------------------------------------------------------------------
// Streaming chunk types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: serde_json::Value,
    pub finish_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Embedding types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingRequest {
    pub model: String,
    pub input: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding_format: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingResponse {
    pub object: String,
    pub data: Vec<EmbeddingData>,
    pub model: String,
    pub usage: EmbeddingUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingData {
    pub object: String,
    pub index: u32,
    pub embedding: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingUsage {
    pub prompt_tokens: u32,
    pub total_tokens: u32,
}

// ---------------------------------------------------------------------------
// Internal event types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    Success,
    Failure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    CodeGeneration,
    CodeReview,
    Summarization,
    Classification,
    Extraction,
    Translation,
    QuestionAnswering,
    CreativeWriting,
    Reasoning,
    Conversation,
    ToolUse,
    Search,
    Embedding,
    Architecture,
    Debugging,
    Refactoring,
    Documentation,
    Testing,
    ToolSelection,
    FillInTheMiddle,
    Unknown,
}

impl TaskType {
    /// All task types used for routing, fitness scoring, and waste detection.
    /// Excludes `Embedding` and `Unknown` which are not routable.
    pub const ALL_ROUTABLE: &[TaskType] = &[
        TaskType::CodeGeneration,
        TaskType::CodeReview,
        TaskType::Summarization,
        TaskType::Classification,
        TaskType::Extraction,
        TaskType::Translation,
        TaskType::QuestionAnswering,
        TaskType::CreativeWriting,
        TaskType::Reasoning,
        TaskType::Conversation,
        TaskType::ToolUse,
        TaskType::Search,
        TaskType::Architecture,
        TaskType::Debugging,
        TaskType::Refactoring,
        TaskType::Documentation,
        TaskType::Testing,
        TaskType::ToolSelection,
    ];

    /// Parse a task type from a snake_case string, returning Unknown for unrecognized values.
    pub fn from_str_loose(s: &str) -> Self {
        serde_json::from_value(serde_json::Value::String(s.to_string()))
            .unwrap_or(TaskType::Unknown)
    }
}

impl std::fmt::Display for TaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_value(self)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "unknown".into());
        f.write_str(&s)
    }
}

// ---------------------------------------------------------------------------
// Text completion types (OpenAI legacy completions, used for FIM / edit predictions)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TextCompletionRequest {
    pub model: String,
    pub prompt: String,
    #[serde(default)]
    pub suffix: Option<String>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub stop: Vec<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct TextCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<TextCompletionChoice>,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
pub struct TextCompletionChoice {
    pub text: String,
    pub index: u32,
    pub finish_reason: Option<String>,
}

/// A captured inference event, written to ClickHouse.
#[derive(Debug, Clone, Serialize)]
pub struct InferenceEvent {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub provider: String,
    pub model: String,
    pub status: EventStatus,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
    pub cache_read_input_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub estimated_cost_usd: f64,
    pub latency_ms: u32,
    pub prompt_hash: String,
    pub completion_hash: String,
    pub task_type: Option<TaskType>,
    pub routing_decision: Option<String>,
    pub variant_name: Option<String>,
    pub virtual_key_hash: Option<String>,
    pub team_id: Option<String>,
    pub end_user_id: Option<String>,
    pub episode_id: Option<Uuid>,
    pub metadata: String,
    // --- Phase 1.3: Richer observability fields ---
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub agent_framework: Option<String>,
    pub tool_calls_json: Option<String>,
    pub ttft_ms: Option<u32>,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub provider_attempted: Option<String>,
}

/// Response from a provider — either complete or streaming.
pub enum ProviderResponse {
    Complete(ChatCompletionResponse),
    Stream(
        std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<bytes::Bytes, PrismStreamError>> + Send>,
        >,
    ),
}

#[derive(Debug)]
pub enum PrismStreamError {
    Reqwest(reqwest::Error),
    Other(String),
}

impl std::fmt::Display for PrismStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrismStreamError::Reqwest(e) => write!(f, "{e}"),
            PrismStreamError::Other(s) => write!(f, "{s}"),
        }
    }
}
