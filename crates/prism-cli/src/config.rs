use std::path::PathBuf;

use anyhow::Result;

/// Returns the agent name from the environment.
/// Reads `PRISM_AGENT_NAME`, falling back to `UH_AGENT_NAME` (backward compat), then `"claude"`.
pub fn agent_name_from_env() -> String {
    std::env::var("PRISM_AGENT_NAME")
        .or_else(|_| std::env::var("UH_AGENT_NAME"))
        .unwrap_or_else(|_| "claude".to_string())
}

/// Returns `~/.prism`, the base directory for all prism-cli state.
pub fn prism_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".prism")
}

/// Controls which tools are available to the agent (retained for persona deserialization).
#[derive(Debug, Clone, Default, PartialEq)]
pub enum SandboxMode {
    #[default]
    None,
    ReadOnly,
    Restricted,
}

impl SandboxMode {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "read-only" | "readonly" | "read_only" => Self::ReadOnly,
            "restricted" => Self::Restricted,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub url: String,
    pub api_key: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub gateway: GatewayConfig,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let url =
            std::env::var("PRISM_URL").unwrap_or_else(|_| "http://localhost:9100".to_string());
        let api_key = std::env::var("PRISM_API_KEY").unwrap_or_default();

        Ok(Self {
            gateway: GatewayConfig { url, api_key },
        })
    }
}
