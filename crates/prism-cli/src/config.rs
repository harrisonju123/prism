use anyhow::Result;
use std::path::PathBuf;

use crate::compression::ContextCompressor;
use crate::hooks::HookRunner;
use crate::hooks::config::HooksConfig;
use crate::permissions::PermissionMode;

/// Returns `~/.prism`, the base directory for all prism-cli state.
pub fn prism_home() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".prism")
}

#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub url: String,
    pub api_key: String,
}

#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub model: String,
    pub max_turns: u32,
    pub max_cost_usd: Option<f64>,
    pub max_tool_output: usize,
    pub system_prompt: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub sessions_dir: PathBuf,
    pub max_session_messages: usize,
    pub max_sessions: usize,
}

#[derive(Debug, Clone)]
pub struct CompressionConfig {
    pub model: Option<String>,
    pub threshold: f64,
    pub preserve_recent: usize,
}

#[derive(Debug, Clone)]
pub struct ExtensionConfig {
    pub mcp_config_path: PathBuf,
    pub hooks_config_path: PathBuf,
    pub permission_mode: Option<PermissionMode>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub gateway: GatewayConfig,
    pub model: ModelConfig,
    pub session: SessionConfig,
    pub compression: CompressionConfig,
    pub extensions: ExtensionConfig,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let url =
            std::env::var("PRISM_URL").unwrap_or_else(|_| "http://localhost:9100".to_string());

        let api_key = std::env::var("PRISM_API_KEY").unwrap_or_default();

        let model = std::env::var("PRISM_MODEL").unwrap_or_else(|_| "gpt-5-2-codex".to_string());

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
            .unwrap_or_else(|_| prism_home().join("sessions"));

        let max_session_messages = std::env::var("PRISM_MAX_SESSION_MESSAGES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(200);

        let max_sessions = std::env::var("PRISM_MAX_SESSIONS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(64);

        let mcp_config_path = crate::mcp::config::mcp_config_path();

        let permission_mode = std::env::var("PRISM_PERMISSION_MODE").ok().and_then(|s| {
            use clap::ValueEnum;
            PermissionMode::from_str(&s, true).ok()
        });

        let hooks_config_path = std::env::var("PRISM_HOOKS_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|_| prism_home().join("hooks.json"));

        let compression_model = std::env::var("PRISM_COMPRESSION_MODEL").ok();

        let compression_threshold = std::env::var("PRISM_COMPRESSION_THRESHOLD")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.7);

        let compression_preserve_recent = std::env::var("PRISM_COMPRESSION_PRESERVE_RECENT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(20);

        Ok(Self {
            gateway: GatewayConfig { url, api_key },
            model: ModelConfig {
                model,
                max_turns,
                max_cost_usd,
                max_tool_output,
                system_prompt,
            },
            session: SessionConfig {
                sessions_dir,
                max_session_messages,
                max_sessions,
            },
            compression: CompressionConfig {
                model: compression_model,
                threshold: compression_threshold,
                preserve_recent: compression_preserve_recent,
            },
            extensions: ExtensionConfig {
                mcp_config_path,
                hooks_config_path,
                permission_mode,
            },
        })
    }

    pub fn build_hook_runner(&self) -> HookRunner {
        HookRunner::new(HooksConfig::load(&self.extensions.hooks_config_path))
    }

    pub fn build_compressor(&self) -> Option<ContextCompressor> {
        self.compression.model.as_ref().map(|model| {
            ContextCompressor::new(
                model.clone(),
                self.compression.threshold,
                self.compression.preserve_recent,
            )
        })
    }
}
