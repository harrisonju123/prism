use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct HooksConfig {
    #[serde(default)]
    pub pre_tool_use: Vec<HookEntry>,
    #[serde(default)]
    pub post_tool_use: Vec<HookEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HookEntry {
    pub command: String,
    /// Glob-style pattern to match tool names (e.g. "bash", "write_*").
    /// If None, the hook runs for all tools.
    pub tool_pattern: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// When true (default), hook errors are treated as Allow.
    /// When false, hook errors are treated as Deny (fail-closed).
    #[serde(default = "default_true")]
    pub fail_open: bool,
}

fn default_timeout() -> u64 {
    10
}

fn default_true() -> bool {
    true
}

/// Action returned by a pre-tool-use hook.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PreToolAction {
    Allow,
    Deny { message: String },
    Modify { args: serde_json::Value },
}

/// Action returned by a post-tool-use hook.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum PostToolAction {
    Passthrough,
    Modify { result: String },
}

impl HooksConfig {
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(config) => config,
                Err(e) => {
                    tracing::warn!("failed to parse hooks config {}: {e}", path.display());
                    Self::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!("failed to read hooks config {}: {e}", path.display());
                Self::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_config() {
        let json = r#"{
            "pre_tool_use": [
                { "command": "echo pre", "tool_pattern": "bash", "timeout_secs": 5 }
            ],
            "post_tool_use": [
                { "command": "echo post", "timeout_secs": 15 }
            ]
        }"#;
        let config: HooksConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.pre_tool_use.len(), 1);
        assert_eq!(config.post_tool_use.len(), 1);
        assert_eq!(config.pre_tool_use[0].tool_pattern.as_deref(), Some("bash"));
        assert_eq!(config.post_tool_use[0].tool_pattern, None);
    }

    #[test]
    fn parse_empty_config() {
        let config: HooksConfig = serde_json::from_str("{}").unwrap();
        assert!(config.pre_tool_use.is_empty());
        assert!(config.post_tool_use.is_empty());
    }

    #[test]
    fn default_timeout_is_10() {
        let json = r#"{ "command": "echo hi" }"#;
        let entry: HookEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.timeout_secs, 10);
    }

    #[test]
    fn parse_pre_tool_actions() {
        let allow: PreToolAction = serde_json::from_str(r#"{"action":"allow"}"#).unwrap();
        assert!(matches!(allow, PreToolAction::Allow));

        let deny: PreToolAction =
            serde_json::from_str(r#"{"action":"deny","message":"nope"}"#).unwrap();
        assert!(matches!(deny, PreToolAction::Deny { message } if message == "nope"));

        let modify: PreToolAction =
            serde_json::from_str(r#"{"action":"modify","args":{"x":1}}"#).unwrap();
        assert!(matches!(modify, PreToolAction::Modify { .. }));
    }

    #[test]
    fn parse_post_tool_actions() {
        let pass: PostToolAction = serde_json::from_str(r#"{"action":"passthrough"}"#).unwrap();
        assert!(matches!(pass, PostToolAction::Passthrough));

        let modify: PostToolAction =
            serde_json::from_str(r#"{"action":"modify","result":"new output"}"#).unwrap();
        assert!(matches!(modify, PostToolAction::Modify { result } if result == "new output"));
    }

    #[test]
    fn load_missing_file_returns_default() {
        let config = HooksConfig::load(Path::new("/nonexistent/hooks.json"));
        assert!(config.pre_tool_use.is_empty());
    }
}
