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
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cache_read_input_tokens: u32,
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
        TaskType::FillInTheMiddle,
    ];

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
// Stats API response types (shared between server and client)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryResponse {
    pub period_days: u32,
    pub total_requests: u64,
    pub total_cost_usd: f64,
    pub total_tokens: u64,
    pub failure_rate: f64,
    pub groups: Vec<StatGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatGroup {
    pub key: String,
    pub request_count: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub avg_cost_per_request_usd: f64,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub failure_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasteScoreResponse {
    pub period_days: u32,
    pub waste_score: f64,
    pub total_cost_usd: f64,
    pub estimated_waste_usd: f64,
}

/// Agent name used when sending waste nudge messages via the context store.
/// Matched in the IDE agent's message polling loop to apply session dedup.
pub const WASTE_DETECTOR_AGENT: &str = "prism-waste-detector";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasteNudge {
    pub category: String,
    pub severity: String,
    pub message: String,
    pub savings_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasteNudgesResponse {
    pub nudges: Vec<WasteNudge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTypeStatsResponse {
    pub period_days: u32,
    pub task_types: Vec<TaskTypeStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTypeStat {
    pub task_type: String,
    pub request_count: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
}

// ---------------------------------------------------------------------------
// Agent metrics types (shared between server and client)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetricStat {
    pub agent_name: String,
    pub request_count: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
    pub total_tokens: u64,
    pub failure_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetricsResponse {
    pub period_days: u32,
    pub agents: Vec<AgentMetricStat>,
}

// ---------------------------------------------------------------------------
// Thread cost types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadCostResponse {
    pub thread_id: String,
    pub total_cost_usd: f64,
    pub request_count: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
}

// ---------------------------------------------------------------------------
// Routing types (shared between server and client)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionCriteria {
    CheapestAboveQuality,
    FastestAboveQuality,
    HighestQualityUnderCost,
    BestValue,
}

impl Default for SelectionCriteria {
    fn default() -> Self {
        Self::CheapestAboveQuality
    }
}

/// A single entry in a provider fallback chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FallbackEntry {
    pub model: String,
    /// Provider to use; inferred from model name if None.
    #[serde(default)]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRule {
    pub task_type: String,
    #[serde(default)]
    pub criteria: SelectionCriteria,
    #[serde(default = "default_min_quality")]
    pub min_quality: f64,
    pub max_cost_per_1k: Option<f64>,
    pub max_latency_ms: Option<u32>,
    pub fallback: Option<String>,
    /// Ordered fallback chain used by the proxy when the primary fails.
    #[serde(default)]
    pub fallback_chain: Vec<FallbackEntry>,
}

fn default_min_quality() -> f64 {
    0.55
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyResponse {
    pub version: u32,
    pub rule_count: usize,
    pub rules: Vec<RoutingRule>,
    pub valid: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitnessEntry {
    pub task_type: TaskType,
    pub model: String,
    pub avg_quality: f64,
    pub avg_cost_per_1k: f64,
    pub avg_latency_ms: f64,
    pub sample_size: u32,
}

// ---------------------------------------------------------------------------
// Analytics types (shared between server, client, and dashboard)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityTrendsResponse {
    pub data: Vec<QualityTrendPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityTrendPoint {
    pub day: String,
    pub model: String,
    pub task_type: Option<String>,
    pub avg_quality: f64,
    pub sample_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingSavingsResponse {
    pub actual_cost: f64,
    pub counterfactual_cost: f64,
    pub savings: f64,
    pub routed_requests: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEfficiencyResponse {
    pub data: Vec<SessionEfficiencyStat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEfficiencyStat {
    pub task_type: String,
    pub avg_turns: f64,
    pub avg_cost: f64,
    pub session_count: u64,
}

// ---------------------------------------------------------------------------
// Internal event types
// ---------------------------------------------------------------------------

/// A captured inference event.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub parent_span_id: Option<String>,
    pub agent_framework: Option<String>,
    pub tool_calls_json: Option<String>,
    pub ttft_ms: Option<u32>,
    pub session_id: Option<String>,
    pub provider_attempted: Option<String>,
}
