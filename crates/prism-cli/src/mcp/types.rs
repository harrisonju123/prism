use serde::{Deserialize, Serialize};

// --- JSON-RPC envelope ---

#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: &'static str,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[allow(dead_code)]
    pub data: Option<serde_json::Value>,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON-RPC error {}: {}", self.code, self.message)
    }
}

// --- MCP initialize ---

#[derive(Debug, Serialize)]
pub struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    #[serde(rename = "clientInfo")]
    pub client_info: Implementation,
}

#[derive(Debug, Serialize)]
pub struct ClientCapabilities {}

#[derive(Debug, Serialize)]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    #[allow(dead_code)]
    pub protocol_version: String,
    #[allow(dead_code)]
    pub capabilities: Option<ServerCapabilities>,
    #[serde(rename = "serverInfo")]
    #[allow(dead_code)]
    pub server_info: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ServerCapabilities {
    #[allow(dead_code)]
    pub tools: Option<serde_json::Value>,
}

// --- MCP tools ---

#[derive(Debug, Clone, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ListToolsResult {
    pub tools: Vec<McpTool>,
}

#[derive(Debug, Serialize)]
pub struct CallToolParams {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct CallToolResult {
    pub content: Vec<ToolContent>,
    #[serde(rename = "isError")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ToolContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: Option<String>,
}

impl CallToolResult {
    /// Extract all text content blocks, joined by newline.
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| c.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_request() {
        let req = JsonRpcRequest::new(1, "initialize", Some(serde_json::json!({"foo": "bar"})));
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"jsonrpc\":\"2.0\""));
        assert!(s.contains("\"id\":1"));
        assert!(s.contains("\"method\":\"initialize\""));
    }

    #[test]
    fn deserialize_response() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn deserialize_error_response() {
        let json =
            r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32600,"message":"invalid request"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
    }

    #[test]
    fn deserialize_mcp_tool() {
        let json = r#"{"name":"search","description":"Search things","inputSchema":{"type":"object","properties":{}}}"#;
        let tool: McpTool = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "search");
        assert!(tool.input_schema.is_some());
    }

    #[test]
    fn deserialize_call_tool_result() {
        let json = r#"{"content":[{"type":"text","text":"hello"}],"isError":false}"#;
        let result: CallToolResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].text.as_deref(), Some("hello"));
        assert_eq!(result.is_error, Some(false));
    }
}
