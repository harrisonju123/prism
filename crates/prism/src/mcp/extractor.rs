use sha2::{Digest, Sha256};

/// Parse MCP tool calls from tool_calls_json.
///
/// MCP tool names follow the pattern `{server}__{method}`.
/// Returns vec of (server, method, tool_name, args_hash).
pub fn extract_mcp_calls(tool_calls_json: &str) -> Vec<McpToolCall> {
    let Ok(calls) = serde_json::from_str::<Vec<serde_json::Value>>(tool_calls_json) else {
        return Vec::new();
    };

    let mut results = Vec::new();
    for call in &calls {
        let function = call.get("function").unwrap_or(call);
        let name = function.get("name").and_then(|v| v.as_str()).unwrap_or("");

        if let Some((server, method)) = parse_mcp_name(name) {
            let args_str = function
                .get("arguments")
                .map(|v| v.to_string())
                .unwrap_or_default();
            let args_hash = hash_string(&args_str);

            results.push(McpToolCall {
                server: server.to_string(),
                method: method.to_string(),
                tool_name: name.to_string(),
                args_hash,
            });
        }
    }

    results
}

#[derive(Debug, Clone)]
pub struct McpToolCall {
    pub server: String,
    pub method: String,
    pub tool_name: String,
    pub args_hash: String,
}

/// Parse `{server}__{method}` pattern from a tool name.
fn parse_mcp_name(name: &str) -> Option<(&str, &str)> {
    let parts: Vec<&str> = name.splitn(2, "__").collect();
    if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
        Some((parts[0], parts[1]))
    } else {
        None
    }
}

fn hash_string(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mcp_name_valid() {
        let (server, method) = parse_mcp_name("filesystem__read_file").unwrap();
        assert_eq!(server, "filesystem");
        assert_eq!(method, "read_file");
    }

    #[test]
    fn parse_mcp_name_no_separator() {
        assert!(parse_mcp_name("regular_tool").is_none());
    }

    #[test]
    fn parse_mcp_name_empty_parts() {
        assert!(parse_mcp_name("__method").is_none());
        assert!(parse_mcp_name("server__").is_none());
    }

    #[test]
    fn extract_from_json() {
        let json = r##"[
            {"function": {"name": "github__list_repos", "arguments": "{\"org\":\"test\"}"}},
            {"function": {"name": "regular_tool", "arguments": "{}"}},
            {"function": {"name": "slack__send_message", "arguments": "{\"channel\":\"#general\"}"}}
        ]"##;

        let calls = extract_mcp_calls(json);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].server, "github");
        assert_eq!(calls[0].method, "list_repos");
        assert_eq!(calls[1].server, "slack");
        assert_eq!(calls[1].method, "send_message");
    }

    #[test]
    fn extract_from_invalid_json() {
        let calls = extract_mcp_calls("not json");
        assert!(calls.is_empty());
    }

    #[test]
    fn extract_from_empty_array() {
        let calls = extract_mcp_calls("[]");
        assert!(calls.is_empty());
    }
}
