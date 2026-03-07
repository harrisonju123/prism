use serde::{Deserialize, Serialize};

/// Format for fine-tuning data export.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    /// OpenAI-compatible JSONL (messages array format).
    Openai,
    /// Anthropic-compatible JSONL.
    Anthropic,
    /// Raw JSONL with all fields.
    Raw,
}

impl ExportFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            ExportFormat::Openai => "openai",
            ExportFormat::Anthropic => "anthropic",
            ExportFormat::Raw => "raw",
        }
    }
}

/// Filters for selecting training data.
#[derive(Debug, Deserialize)]
pub struct ExportFilters {
    /// Only include events from these models.
    #[serde(default)]
    pub models: Vec<String>,
    /// Only include events with these task types.
    #[serde(default)]
    pub task_types: Vec<String>,
    /// Minimum quality score (from feedback).
    #[serde(default)]
    pub min_quality_score: Option<f64>,
    /// Maximum latency (filter out slow responses).
    #[serde(default)]
    pub max_latency_ms: Option<u32>,
    /// Number of days to look back.
    #[serde(default = "default_days")]
    pub days: u32,
    /// Maximum number of samples to export.
    #[serde(default = "default_max_samples")]
    pub max_samples: usize,
    /// Export format.
    #[serde(default = "default_format")]
    pub format: ExportFormat,
}

fn default_days() -> u32 {
    30
}
fn default_max_samples() -> usize {
    10000
}
fn default_format() -> ExportFormat {
    ExportFormat::Openai
}

/// A single training example in OpenAI format.
#[derive(Debug, Serialize)]
pub struct OpenAiTrainingExample {
    pub messages: Vec<TrainingMessage>,
}

/// A message in a training example.
#[derive(Debug, Serialize)]
pub struct TrainingMessage {
    pub role: String,
    pub content: String,
}

/// Response from the export endpoint.
#[derive(Debug, Serialize)]
pub struct ExportResponse {
    pub format: String,
    pub total_samples: usize,
    pub data: Vec<serde_json::Value>,
}

/// Request body for fine-tuning export.
#[derive(Debug, Deserialize)]
pub struct ExportRequest {
    #[serde(flatten)]
    pub filters: ExportFilters,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_format_str() {
        assert_eq!(ExportFormat::Openai.as_str(), "openai");
        assert_eq!(ExportFormat::Anthropic.as_str(), "anthropic");
        assert_eq!(ExportFormat::Raw.as_str(), "raw");
    }

    #[test]
    fn deserialize_export_filters_defaults() {
        let json = r#"{}"#;
        let filters: ExportFilters = serde_json::from_str(json).unwrap();
        assert_eq!(filters.days, 30);
        assert_eq!(filters.max_samples, 10000);
        assert_eq!(filters.format, ExportFormat::Openai);
    }

    #[test]
    fn deserialize_export_filters_custom() {
        let json =
            r#"{"models": ["gpt-4o"], "days": 7, "max_samples": 500, "format": "anthropic"}"#;
        let filters: ExportFilters = serde_json::from_str(json).unwrap();
        assert_eq!(filters.models, vec!["gpt-4o"]);
        assert_eq!(filters.days, 7);
        assert_eq!(filters.format, ExportFormat::Anthropic);
    }
}
