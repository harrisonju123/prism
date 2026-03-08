use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

pub async fn bash(command: &str, timeout_secs: u64, cwd: Option<&str>) -> String {
    run_command(
        "sh",
        &["-c".to_string(), command.to_string()],
        timeout_secs,
        cwd,
    )
    .await
}

pub async fn run_command(
    command: &str,
    args: &[String],
    timeout_secs: u64,
    cwd: Option<&str>,
) -> String {
    let timeout_secs = timeout_secs.min(120);

    let mut cmd = Command::new(command);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let child = cmd.spawn();

    let child = match child {
        Ok(c) => c,
        Err(e) => {
            return serde_json::json!({ "error": format!("failed to spawn: {e}") }).to_string();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_command_no_cwd() {
        let result = run_command("echo", &["hello".to_string()], 10, None).await;
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["exit_code"], 0);
        assert!(v["stdout"].as_str().unwrap().contains("hello"));
    }

    #[tokio::test]
    async fn test_run_command_with_cwd() {
        let tmp = std::env::temp_dir();
        let tmp_str = tmp.to_str().unwrap();
        let result = run_command(
            "sh",
            &["-c".to_string(), "pwd".to_string()],
            10,
            Some(tmp_str),
        )
        .await;
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["exit_code"], 0);
        let stdout = v["stdout"].as_str().unwrap().trim().to_string();
        // Resolve symlinks on both sides for macOS /var -> /private/var
        let got =
            std::fs::canonicalize(&stdout).unwrap_or_else(|_| std::path::PathBuf::from(&stdout));
        let expected = std::fs::canonicalize(tmp_str).unwrap_or_else(|_| tmp.clone());
        assert_eq!(got, expected);
    }
}
