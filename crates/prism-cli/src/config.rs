use anyhow::{Context, Result};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub prism_url: String,
    pub prism_api_key: String,
    pub prism_model: String,
    pub max_turns: u32,
    pub max_cost_usd: Option<f64>,
    pub max_tool_output: usize,
    pub system_prompt: Option<String>,
    pub sessions_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let prism_url = std::env::var("PRISM_URL")
            .unwrap_or_else(|_| "http://localhost:4000".to_string());

        let prism_api_key = std::env::var("PRISM_API_KEY")
            .context("PRISM_API_KEY env var is required")?;

        let prism_model = std::env::var("PRISM_MODEL")
            .unwrap_or_else(|_| "claude-sonnet-4-6".to_string());

        let max_turns = std::env::var("PRISM_MAX_TURNS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);

        let max_cost_usd = std::env::var("PRISM_MAX_COST_USD")
            .ok()
            .and_then(|s| s.parse().ok());

        let max_tool_output = std::env::var("PRISM_MAX_TOOL_OUTPUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(32_768);

        let system_prompt = std::env::var("PRISM_SYSTEM_PROMPT").ok();

        let sessions_dir = std::env::var("PRISM_SESSIONS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".prism/sessions")
            });

        Ok(Self {
            prism_url,
            prism_api_key,
            prism_model,
            max_turns,
            max_cost_usd,
            max_tool_output,
            system_prompt,
            sessions_dir,
        })
    }
}
