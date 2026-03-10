use anyhow::{Result, anyhow};
use serde_json::json;

use super::config::McpServerEntry;
use super::transport::StdioTransport;
use super::types::*;

/// MCP client for a single server.
pub struct McpClient {
    name: String,
    transport: StdioTransport,
    tools: Vec<McpTool>,
    timeout_secs: u64,
}

impl McpClient {
    /// Connect to an MCP server: spawn process, handshake, discover tools.
    pub async fn connect(name: &str, entry: &McpServerEntry) -> Result<Self> {
        let timeout_secs = entry.timeout_secs.unwrap_or(30);

        let transport = StdioTransport::spawn(&entry.command, &entry.args, &entry.env)?;

        // initialize handshake
        let init_params = InitializeParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities {},
            client_info: Implementation {
                name: "prism-cli".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let resp = transport
            .request(
                "initialize",
                Some(serde_json::to_value(&init_params)?),
                timeout_secs,
            )
            .await?;

        if let Some(err) = resp.error {
            return Err(anyhow!("MCP server `{name}` initialize failed: {err}"));
        }

        if let Some(result) = &resp.result {
            let _init: InitializeResult = serde_json::from_value(result.clone())
                .map_err(|e| anyhow!("MCP server `{name}` bad initialize response: {e}"))?;
        }

        transport.notify("notifications/initialized", None).await?;

        // Discover tools
        let tools_resp = transport
            .request("tools/list", Some(json!({})), timeout_secs)
            .await?;

        let tools = if let Some(err) = tools_resp.error {
            tracing::warn!("MCP server `{name}` tools/list failed: {err}");
            Vec::new()
        } else if let Some(result) = tools_resp.result {
            let list: ListToolsResult = serde_json::from_value(result)
                .map_err(|e| anyhow!("MCP server `{name}` bad tools/list response: {e}"))?;
            list.tools
        } else {
            Vec::new()
        };

        tracing::info!(
            server = name,
            tool_count = tools.len(),
            "MCP server connected"
        );

        Ok(Self {
            name: name.to_string(),
            transport,
            tools,
            timeout_secs,
        })
    }

    /// Get the cached tool list.
    pub fn tools(&self) -> &[McpTool] {
        &self.tools
    }

    /// Server name (used for logging).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Call a tool on this server.
    pub async fn call_tool(&self, tool_name: &str, args: serde_json::Value) -> Result<String> {
        let params = CallToolParams {
            name: tool_name.to_string(),
            arguments: args,
        };

        let resp = self
            .transport
            .request(
                "tools/call",
                Some(serde_json::to_value(&params)?),
                self.timeout_secs,
            )
            .await?;

        if let Some(err) = resp.error {
            return Err(anyhow!("MCP tool call `{tool_name}` failed: {err}"));
        }

        let result_val = resp
            .result
            .ok_or_else(|| anyhow!("MCP tool call `{tool_name}` returned no result"))?;

        let call_result: CallToolResult = serde_json::from_value(result_val)
            .map_err(|e| anyhow!("MCP tool call `{tool_name}` bad response: {e}"))?;

        let text = call_result.text_content();

        if call_result.is_error == Some(true) {
            return Err(anyhow!("MCP tool `{tool_name}` returned error: {text}"));
        }

        Ok(text)
    }

    /// Gracefully shut down the MCP server.
    pub async fn shutdown(&self) {
        self.transport.kill().await;
    }
}
