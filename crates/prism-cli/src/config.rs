use anyhow::Result;
use std::path::PathBuf;

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
pub struct Config {
    pub prism_url: String,
    pub prism_api_key: String,
    pub prism_model: String,
    pub max_turns: u32,
    pub max_cost_usd: Option<f64>,
    pub max_tool_output: usize,
    pub system_prompt: Option<String>,
    pub sessions_dir: PathBuf,
    pub memory_window_size: usize,
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
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let prism_url =
            std::env::var("PRISM_URL").unwrap_or_else(|_| "http://localhost:9100".to_string());

        let prism_api_key = std::env::var("PRISM_API_KEY").unwrap_or_default();

        let prism_model =
            std::env::var("PRISM_MODEL").unwrap_or_else(|_| "claude-sonnet-4-6".to_string());

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

        let memory_window_size = std::env::var("PRISM_MEMORY_WINDOW_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(4096);

        let show_cost = std::env::var("PRISM_SHOW_COST")
            .map(|s| s != "0" && s.to_lowercase() != "false")
            .unwrap_or(true);

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
            .unwrap_or_else(|| vec![".env".to_string(), "*.key".to_string(), "~/.ssh/**".to_string()]);

        let denied_commands = std::env::var("PRISM_DENIED_COMMANDS")
            .ok()
            .map(|s| s.split(',').map(|c| c.trim().to_string()).collect())
            .unwrap_or_else(|| vec!["rm -rf /".to_string(), "sudo".to_string()]);

        let sandbox_mode = std::env::var("PRISM_SANDBOX_MODE")
            .map(|s| SandboxMode::from_str(&s))
            .unwrap_or_default();

        Ok(Self {
            prism_url,
            prism_api_key,
            prism_model,
            max_turns,
            max_cost_usd,
            max_tool_output,
            system_prompt,
            sessions_dir,
            memory_window_size,
            show_cost,
            persona,
            allowed_tools,
            denied_tools,
            denied_paths,
            denied_commands,
            sandbox_mode,
        })
    }
}
