/// Computer use tools: screenshot, click, type_text, key_press.
/// All implementations are macOS-only, using built-in screencapture and osascript.
use prism_types::{Tool, ToolFunction};
use serde_json::json;
use std::process::Command;
use uuid::Uuid;

use super::ToolResult;

pub fn computer_tool_definitions() -> Vec<Tool> {
    vec![
        make_tool(
            "screenshot",
            "Take a screenshot of the screen. Returns an image. Optionally capture a region.",
            json!({ "type": "object", "properties": {
                "region": {
                    "type": "object",
                    "description": "Optional screen region to capture",
                    "properties": {
                        "x": { "type": "integer" },
                        "y": { "type": "integer" },
                        "width": { "type": "integer" },
                        "height": { "type": "integer" }
                    }
                }
            }, "required": [] }),
        ),
        make_tool(
            "click",
            "Click at the given screen coordinates.",
            json!({ "type": "object", "properties": {
                "x": { "type": "integer", "description": "X coordinate" },
                "y": { "type": "integer", "description": "Y coordinate" }
            }, "required": ["x", "y"] }),
        ),
        make_tool(
            "type_text",
            "Type text using the keyboard.",
            json!({ "type": "object", "properties": {
                "text": { "type": "string", "description": "Text to type" }
            }, "required": ["text"] }),
        ),
        make_tool(
            "key_press",
            "Press a key combination (e.g. cmd+s, ctrl+z).",
            json!({ "type": "object", "properties": {
                "key": { "type": "string", "description": "Key name (e.g. 'return', 'escape', 's')" },
                "modifiers": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["cmd", "ctrl", "alt", "shift"] },
                    "description": "Modifier keys to hold"
                }
            }, "required": ["key"] }),
        ),
    ]
}

/// Take a full-screen or region screenshot, return as multimodal content.
pub async fn screenshot(region: Option<&serde_json::Value>) -> ToolResult {
    let tmp_path = format!("/tmp/prism_screenshot_{}.png", Uuid::new_v4());

    // Build screencapture command
    let mut cmd = Command::new("screencapture");
    cmd.arg("-x"); // no sound

    if let Some(r) = region {
        let x = r["x"].as_i64().unwrap_or(0);
        let y = r["y"].as_i64().unwrap_or(0);
        let w = r["width"].as_i64().unwrap_or(800);
        let h = r["height"].as_i64().unwrap_or(600);
        cmd.arg("-R").arg(format!("{x},{y},{w},{h}"));
    }

    cmd.arg(&tmp_path);

    let status = match cmd.output() {
        Ok(o) => o.status,
        Err(e) => {
            return ToolResult::Text(format!("{{\"error\": \"screencapture failed: {e}\"}}"));
        }
    };

    if !status.success() {
        return ToolResult::Text(
            "{\"error\": \"screencapture exited with non-zero status\"}".to_string(),
        );
    }

    // Downscale to 1280px wide using sips (built-in macOS)
    let _ = Command::new("sips")
        .args(["--resampleWidth", "1280", &tmp_path])
        .output();

    // Read and base64-encode the image
    let image_bytes = match std::fs::read(&tmp_path) {
        Ok(b) => b,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            return ToolResult::Text(format!("{{\"error\": \"failed to read screenshot: {e}\"}}"));
        }
    };
    let _ = std::fs::remove_file(&tmp_path);

    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);
    let data_url = format!("data:image/png;base64,{b64}");

    ToolResult::Multimodal(json!([
        { "type": "text", "text": "Screenshot captured (1280px wide)." },
        { "type": "image_url", "image_url": { "url": data_url } }
    ]))
}

/// Click at screen coordinates using osascript.
pub async fn click(x: i32, y: i32) -> String {
    let script = format!("tell application \"System Events\" to click at {{{x}, {y}}}");
    run_osascript(&script)
}

/// Type text using osascript keystroke.
pub async fn type_text(text: &str) -> String {
    // Escape backslashes and quotes for AppleScript string literal
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!("tell application \"System Events\" to keystroke \"{escaped}\"");
    run_osascript(&script)
}

/// Press a key combination using osascript.
pub async fn key_press(key: &str, modifiers: &[String]) -> String {
    let using_clause = if modifiers.is_empty() {
        String::new()
    } else {
        let mods: Vec<String> = modifiers
            .iter()
            .map(|m| match m.as_str() {
                "cmd" | "command" => "command down",
                "ctrl" | "control" => "control down",
                "alt" | "option" => "option down",
                "shift" => "shift down",
                other => other,
            })
            .map(str::to_string)
            .collect();
        format!(" using {{{}}}", mods.join(", "))
    };

    // Escape key name for AppleScript
    let escaped_key = key.replace('"', "\\\"");
    let script =
        format!("tell application \"System Events\" to keystroke \"{escaped_key}\"{using_clause}");
    run_osascript(&script)
}

fn run_osascript(script: &str) -> String {
    match Command::new("osascript").arg("-e").arg(script).output() {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if stdout.is_empty() {
                "{\"ok\": true}".to_string()
            } else {
                format!(
                    "{{\"ok\": true, \"output\": {}}}",
                    serde_json::json!(stdout)
                )
            }
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
            format!("{{\"error\": {}}}", serde_json::json!(stderr))
        }
        Err(e) => format!("{{\"error\": \"osascript not available: {e}\"}}"),
    }
}

fn make_tool(name: &str, description: &str, parameters: serde_json::Value) -> Tool {
    Tool {
        r#type: "function".to_string(),
        function: ToolFunction {
            name: name.to_string(),
            description: Some(description.to_string()),
            parameters: Some(parameters),
        },
    }
}
