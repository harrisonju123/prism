use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

use uglyhat::model::{HandoffConstraints, HandoffMode};

#[derive(Debug, Serialize, Deserialize)]
pub struct SpawnConfig {
    pub task: String,
    pub model: Option<String>,
    pub cost_cap: Option<f64>,
    pub tools: Option<Vec<String>>,
    pub timeout_secs: Option<u64>,
    /// Thread to assign the child agent to
    #[serde(default)]
    pub thread: Option<String>,
    /// Handoff constraints forwarded from the parent
    #[serde(default)]
    pub constraints: Option<HandoffConstraints>,
    /// Handoff mode (delegate-and-await vs fire-and-forget)
    #[serde(default)]
    pub handoff_mode: Option<HandoffMode>,
    /// Handoff ID assigned by the parent — passed to child via PRISM_HANDOFF_ID.
    #[serde(default, skip)]
    pub handoff_id: Option<Uuid>,
}

impl SpawnConfig {
    /// Build from tool call args JSON, using the given task string.
    pub fn from_args(args: &serde_json::Value, task: String) -> Self {
        let constraints = if args["constraints"].is_object() {
            serde_json::from_value(args["constraints"].clone()).ok()
        } else {
            None
        };
        let handoff_mode = args["handoff_mode"]
            .as_str()
            .and_then(HandoffMode::from_str);
        Self {
            task,
            model: args["model"].as_str().map(str::to_string),
            cost_cap: args["cost_cap"].as_f64(),
            tools: None,
            timeout_secs: args["timeout_secs"].as_u64(),
            thread: args["thread"].as_str().map(str::to_string),
            constraints,
            handoff_mode,
            handoff_id: None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AgentResult {
    pub status: String,
    pub summary: String,
    pub cost: f64,
    pub turns: u32,
}

pub async fn spawn_agent(
    config: SpawnConfig,
    prism_url: &str,
    api_key: &str,
) -> Result<AgentResult> {
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

    // Forward thread and handoff context to the child
    if let Some(ref thread) = config.thread {
        cmd.env(super::UH_THREAD_ENV, thread);
    }
    if let Some(ref constraints) = config.constraints
        && let Ok(json) = serde_json::to_string(constraints)
    {
        cmd.env("UH_CONSTRAINTS", json);
    }
    // Forward handoff id so the child can accept it on startup
    if let Some(hid) = config.handoff_id {
        cmd.env("PRISM_HANDOFF_ID", hid.to_string());
    }
    // Forward plan mode so children don't escape to default/auto
    if let Ok(mode) = std::env::var("PRISM_PERMISSION_MODE") {
        cmd.env("PRISM_PERMISSION_MODE", mode);
    }
    if let Ok(plan_file) = std::env::var("PRISM_PLAN_FILE") {
        cmd.env("PRISM_PLAN_FILE", plan_file);
    }

    let timeout = std::time::Duration::from_secs(config.timeout_secs.unwrap_or(300));

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("failed to spawn child: {e}"))?;

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
