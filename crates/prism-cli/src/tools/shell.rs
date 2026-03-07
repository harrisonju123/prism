use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

pub async fn run_command(command: &str, args: &[String], timeout_secs: u64) -> String {
    let timeout_secs = timeout_secs.min(120);

    let child = Command::new(command)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let child = match child {
        Ok(c) => c,
        Err(e) => {
            return serde_json::json!({ "error": format!("failed to spawn: {e}") }).to_string()
        }
    };

    match tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await {
        Ok(Ok(output)) => serde_json::json!({
            "exit_code": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        })
        .to_string(),
        Ok(Err(e)) => serde_json::json!({ "error": format!("io error: {e}") }).to_string(),
        Err(_) => {
            serde_json::json!({ "error": format!("timed out after {timeout_secs}s") }).to_string()
        }
    }
}
