use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Debug, Serialize, Deserialize)]
pub struct SpawnConfig {
    pub task: String,
    pub model: Option<String>,
    pub cost_cap: Option<f64>,
    pub tools: Option<Vec<String>>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentResult {
    pub status: String,
    pub summary: String,
    pub cost: f64,
    pub turns: u32,
}

pub async fn spawn_agent(config: SpawnConfig, prism_url: &str, api_key: &str) -> Result<AgentResult> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.arg("run")
        .arg(&config.task)
        .env("PRISM_URL", prism_url)
        .env("PRISM_API_KEY", api_key)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    if let Some(model) = &config.model {
        cmd.arg("--model").arg(model);
    }
    if let Some(cap) = config.cost_cap {
        cmd.arg("--cost-cap").arg(cap.to_string());
    }

    let timeout = std::time::Duration::from_secs(config.timeout_secs.unwrap_or(300));

    let mut child = cmd.spawn().map_err(|e| anyhow!("failed to spawn child: {e}"))?;

    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let mut lines = BufReader::new(stdout).lines();
    let mut last_line = String::new();

    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        tokio::select! {
            line = lines.next_line() => {
                match line? {
                    Some(l) => { last_line = l; }
                    None => break,
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                child.kill().await.ok();
                return Err(anyhow!("sub-agent timed out after {}s", timeout.as_secs()));
            }
        }
    }

    let exit_status = child.wait().await?;

    // Try parsing last line as JSON result first
    if let Ok(result) = serde_json::from_str::<AgentResult>(&last_line) {
        return Ok(result);
    }

    if !exit_status.success() {
        return Err(anyhow!(
            "sub-agent exited with {exit_status}; last output: {last_line}"
        ));
    }

    // Zero exit but no JSON result — treat as done with last line as summary
    Ok(AgentResult {
        status: "done".to_string(),
        summary: last_line,
        cost: 0.0,
        turns: 0,
    })
}
