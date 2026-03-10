pub mod client;
pub mod config;
pub mod transport;
pub mod types;

use anyhow::Result;
use prism_types::{Tool, ToolFunction};
use std::collections::HashMap;

use self::client::McpClient;
use self::config::McpServersConfig;

const MCP_SEPARATOR: &str = "__";

fn namespaced_tool_name(server: &str, tool: &str) -> String {
    format!("{server}{MCP_SEPARATOR}{tool}")
}

/// Registry of connected MCP servers and their tools.
pub struct McpRegistry {
    clients: HashMap<String, McpClient>,
    /// Cached tool definitions — static once constructed.
    cached_tools: Vec<Tool>,
    /// Cached system prompt section — static once constructed.
    cached_prompt_section: String,
}

impl McpRegistry {
    /// Spawn and connect to all configured MCP servers concurrently.
    /// Failures are logged and skipped — the registry contains only successful connections.
    pub async fn connect_all(config: &McpServersConfig) -> Self {
        let futures: Vec<_> = config
            .mcp_servers
            .iter()
            .map(|(name, entry)| async move {
                match McpClient::connect(name, entry).await {
                    Ok(client) => Some((name.clone(), client)),
                    Err(e) => {
                        tracing::warn!(server = %name, "MCP server failed to connect: {e}");
                        None
                    }
                }
            })
            .collect();

        let results = futures::future::join_all(futures).await;
        let clients: HashMap<String, McpClient> = results.into_iter().flatten().collect();

        if !clients.is_empty() {
            let total_tools: usize = clients.values().map(|c| c.tools().len()).sum();
            tracing::info!(
                servers = clients.len(),
                tools = total_tools,
                "MCP registry initialized"
            );
        }

        let cached_tools = Self::build_tool_definitions(&clients);
        let cached_prompt_section = Self::build_prompt_section(&clients);

        Self {
            clients,
            cached_tools,
            cached_prompt_section,
        }
    }

    fn build_tool_definitions(clients: &HashMap<String, McpClient>) -> Vec<Tool> {
        let mut tools = Vec::new();
        for (server_name, client) in clients {
            for mcp_tool in client.tools() {
                tools.push(Tool {
                    r#type: "function".to_string(),
                    function: ToolFunction {
                        name: namespaced_tool_name(server_name, &mcp_tool.name),
                        description: mcp_tool.description.clone(),
                        parameters: mcp_tool.input_schema.clone(),
                    },
                });
            }
        }
        tools
    }

    fn build_prompt_section(clients: &HashMap<String, McpClient>) -> String {
        if clients.is_empty() {
            return String::new();
        }

        let mut section = String::from("\n\n## MCP Tools\n\nYou also have access to external tools from MCP servers:\n\n");

        for (server_name, client) in clients {
            for mcp_tool in client.tools() {
                let namespaced = namespaced_tool_name(server_name, &mcp_tool.name);
                let desc = mcp_tool
                    .description
                    .as_deref()
                    .unwrap_or("(no description)");
                section.push_str(&format!("- **{namespaced}**: {desc}\n"));
            }
        }

        section.push_str(
            "\nCall MCP tools by their full name (e.g. `server__tool_name`). \
             They accept JSON arguments as defined by their schemas.\n",
        );
        section
    }

    /// Return cached MCP tool definitions.
    pub fn tool_definitions(&self) -> &[Tool] {
        &self.cached_tools
    }

    /// Check if a tool name is an MCP-namespaced tool.
    pub fn is_mcp_tool(name: &str) -> bool {
        name.contains(MCP_SEPARATOR)
    }

    /// Dispatch a tool call to the appropriate MCP server.
    pub async fn dispatch(&self, name: &str, args: &serde_json::Value) -> Result<String> {
        let (server_name, tool_name) = name
            .split_once(MCP_SEPARATOR)
            .ok_or_else(|| anyhow::anyhow!("invalid MCP tool name (missing `{MCP_SEPARATOR}`): {name}"))?;

        let client = self
            .clients
            .get(server_name)
            .ok_or_else(|| anyhow::anyhow!("MCP server `{server_name}` not connected"))?;

        client.call_tool(tool_name, args.clone()).await
    }

    /// Return cached system prompt section documenting available MCP tools.
    pub fn system_prompt_section(&self) -> &str {
        &self.cached_prompt_section
    }

    /// Shut down all connected MCP servers concurrently.
    pub async fn shutdown_all(&self) {
        let futures: Vec<_> = self
            .clients
            .iter()
            .map(|(name, client)| async move {
                tracing::debug!(server = %name, "shutting down MCP server");
                client.shutdown().await;
            })
            .collect();

        futures::future::join_all(futures).await;
    }

    /// Whether any servers are connected.
    pub fn is_empty(&self) -> bool {
        self.clients.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_mcp_tool_detection() {
        assert!(McpRegistry::is_mcp_tool("datadog__search_logs"));
        assert!(McpRegistry::is_mcp_tool("fs__read_file"));
        assert!(!McpRegistry::is_mcp_tool("read_file"));
        assert!(!McpRegistry::is_mcp_tool("bash"));
    }

    #[test]
    fn name_split() {
        let name = "datadog__search_logs";
        let (server, tool) = name.split_once(MCP_SEPARATOR).unwrap();
        assert_eq!(server, "datadog");
        assert_eq!(tool, "search_logs");
    }

    #[test]
    fn namespaced_name_format() {
        assert_eq!(namespaced_tool_name("dd", "search"), "dd__search");
    }

    #[test]
    fn empty_registry_produces_no_tools() {
        let registry = McpRegistry {
            clients: HashMap::new(),
            cached_tools: Vec::new(),
            cached_prompt_section: String::new(),
        };
        assert!(registry.tool_definitions().is_empty());
        assert!(registry.system_prompt_section().is_empty());
        assert!(registry.is_empty());
    }
}
