use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A versioned prompt template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub id: Uuid,
    pub name: String,
    pub version: u32,
    pub content: String,
    pub model_hint: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub active: bool,
}

/// Request to create a new prompt template.
#[derive(Debug, Clone, Deserialize)]
pub struct CreatePromptRequest {
    pub name: String,
    pub content: String,
    pub model_hint: Option<String>,
    #[serde(default = "default_metadata")]
    pub metadata: serde_json::Value,
}

fn default_metadata() -> serde_json::Value {
    serde_json::Value::Object(serde_json::Map::new())
}

/// Response for listing prompts.
#[derive(Debug, Serialize)]
pub struct PromptListResponse {
    pub prompts: Vec<PromptTemplate>,
}
