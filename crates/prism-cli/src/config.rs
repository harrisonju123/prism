use anyhow::Result;
use std::path::PathBuf;

use crate::compression::ContextCompressor;
use crate::hooks::HookRunner;
use crate::hooks::config::HooksConfig;
use crate::permissions::PermissionMode;

/// Parse a boolean env var. Treats "0" and "false" (case-insensitive) as false; anything else as true.
fn parse_bool_env(var: &str, default: bool) -> bool {
    std::env::var(var)
        .map(|s| s != "0" && s.to_lowercase() != "false")
        .unwrap_or(default)
}

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

/// Controls which tools are available to the agent.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum SandboxMode {
    /// All tools available (default).
    #[default]
    None,
    /// No write/edit/shell tools; read-only file access only.
    ReadOnly,
    /// Enforce allowed_tools/denied_tools/denied_paths/denied_commands lists.
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
    pub max_retries: u32,
}

#[derive(Debug, Clone)]
pub struct ModelConfig {
    pub model: String,
    pub max_turns: u32,
    pub max_cost_usd: Option<f64>,
    pub max_tool_output: usize,
    pub system_prompt: Option<String>,
    /// Inject a convergence nudge after this many consecutive read-only turns. 0 = disabled.
    pub exploration_nudge_turns: u32,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub sessions_dir: PathBuf,
    pub show_cost: bool,
    /// Persona name to load from ~/.prism/personas/<name>.toml
    pub persona: Option<String>,
    /// Explicit allow-list of tool names (None = all allowed).
    pub allowed_tools: Option<Vec<String>>,
    /// Explicit deny-list of tool names.
    pub denied_tools: Vec<String>,
    /// Path patterns that agents cannot read/write (glob syntax).
    pub denied_paths: Vec<String>,
    /// Command prefixes that bash/run_command cannot execute.
    pub denied_commands: Vec<String>,
    /// Sandbox mode controlling overall tool availability.
    pub sandbox_mode: SandboxMode,
    pub max_session_messages: usize,
    pub max_sessions: usize,
    /// Shell command to run after each turn that touches code files. None = disabled.
    pub compile_check_command: Option<String>,
    /// Timeout in seconds for the compile check command.
    pub compile_check_timeout: u64,
    /// When true, agent pauses after clean completion and waits for human review.
    pub await_review: bool,
    /// When true, pre-write claim checking and auto-claiming are active.
    pub file_claim_enforcement: bool,
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
    /// When set alongside Plan mode, enables structural guardrail enforcement:
    /// only this file may be written; bash/run_command are blocked entirely.
    pub plan_file: Option<String>,
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

        let max_retries = std::env::var("PRISM_MAX_RETRIES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5);

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

        let exploration_nudge_turns = std::env::var("PRISM_EXPLORATION_NUDGE_TURNS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4);

        let sessions_dir = std::env::var("PRISM_SESSIONS_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| prism_home().join("sessions"));

        let show_cost = parse_bool_env("PRISM_SHOW_COST", true);

        let persona = std::env::var("PRISM_PERSONA").ok();

        let allowed_tools = std::env::var("PRISM_ALLOWED_TOOLS")
            .ok()
            .map(|s| s.split(',').map(|t| t.trim().to_string()).collect());

        let denied_tools = std::env::var("PRISM_DENIED_TOOLS")
            .ok()
            .map(|s| s.split(',').map(|t| t.trim().to_string()).collect())
            .unwrap_or_default();

        let denied_paths = std::env::var("PRISM_DENIED_PATHS")
            .ok()
            .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
            .unwrap_or_else(|| {
                vec![
                    ".env".to_string(),
                    "*.key".to_string(),
                    "~/.ssh/**".to_string(),
                ]
            });

        let denied_commands = std::env::var("PRISM_DENIED_COMMANDS")
            .ok()
            .map(|s| s.split(',').map(|c| c.trim().to_string()).collect())
            .unwrap_or_else(|| vec!["rm -rf /".to_string(), "sudo".to_string()]);

        let sandbox_mode = std::env::var("PRISM_SANDBOX_MODE")
            .map(|s| SandboxMode::from_str(&s))
            .unwrap_or_default();

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

        let plan_file = std::env::var("PRISM_PLAN_FILE").ok();

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
            gateway: GatewayConfig {
                url,
                api_key,
                max_retries,
            },
            model: ModelConfig {
                model,
                max_turns,
                max_cost_usd,
                max_tool_output,
                system_prompt,
                exploration_nudge_turns,
            },
            session: SessionConfig {
                sessions_dir,
                show_cost,
                persona,
                allowed_tools,
                denied_tools,
                denied_paths,
                denied_commands,
                sandbox_mode,
                max_session_messages,
                max_sessions,
                compile_check_command: std::env::var("PRISM_COMPILE_CHECK").ok(),
                compile_check_timeout: std::env::var("PRISM_COMPILE_CHECK_TIMEOUT")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(30),
                await_review: parse_bool_env("PRISM_AWAIT_REVIEW", false),
                file_claim_enforcement: parse_bool_env("PRISM_FILE_CLAIM_ENFORCEMENT", true),
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
                plan_file,
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
