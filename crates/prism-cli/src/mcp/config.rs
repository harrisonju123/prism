use anyhow::Result;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize, Default)]
pub struct McpServersConfig {
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: HashMap<String, McpServerEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpServerEntry {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub timeout_secs: Option<u64>,
}

pub fn load_mcp_config(path: &Path) -> Result<McpServersConfig> {
    if !path.exists() {
        return Ok(McpServersConfig::default());
    }
    let content = std::fs::read_to_string(path)?;
    let config: McpServersConfig = serde_json::from_str(&content)?;
    Ok(config)
}

/// Resolve the MCP config file path from env or default.
pub fn mcp_config_path() -> std::path::PathBuf {
    std::env::var("PRISM_MCP_CONFIG")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| crate::config::prism_home().join("mcp_servers.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_missing_file_returns_empty() {
        let config = load_mcp_config(Path::new("/nonexistent/mcp_servers.json")).unwrap();
        assert!(config.mcp_servers.is_empty());
    }

    #[test]
    fn load_valid_config() {
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        write!(
            tmpfile,
            r#"{{
                "mcpServers": {{
                    "filesystem": {{
                        "command": "npx",
                        "args": ["-y", "@anthropic/mcp-server-filesystem", "/tmp"],
                        "env": {{}},
                        "timeout_secs": 10
                    }}
                }}
            }}"#
        )
        .unwrap();

        let config = load_mcp_config(tmpfile.path()).unwrap();
        assert_eq!(config.mcp_servers.len(), 1);
        let fs_entry = &config.mcp_servers["filesystem"];
        assert_eq!(fs_entry.command, "npx");
        assert_eq!(fs_entry.args.len(), 3);
        assert_eq!(fs_entry.timeout_secs, Some(10));
    }

    #[test]
    fn load_config_with_env_vars() {
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        write!(
            tmpfile,
            r#"{{
                "mcpServers": {{
                    "datadog": {{
                        "command": "npx",
                        "args": ["-y", "@anthropic/mcp-server-datadog"],
                        "env": {{ "DD_API_KEY": "test123" }}
                    }}
                }}
            }}"#
        )
        .unwrap();

        let config = load_mcp_config(tmpfile.path()).unwrap();
        let dd = &config.mcp_servers["datadog"];
        assert_eq!(dd.env.get("DD_API_KEY").unwrap(), "test123");
    }
}
