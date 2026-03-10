pub mod spawn;

use anyhow::{Result, anyhow};
use futures::StreamExt;
use prism_client::PrismClient;
use prism_types::{ChatCompletionRequest, Message};
use serde_json::json;
use std::collections::HashMap;
use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

use crate::common::{
    self, ToolCallBuilder, accumulate_tool_call_deltas, build_system_prompt,
    reconstruct_tool_calls, truncate_tool_output,
};
use crate::config::Config;
use crate::mcp::McpRegistry;
use crate::memory::MemoryManager;
use crate::session::Session;
use crate::tools;

pub struct Agent {
    client: PrismClient,
    config: Config,
    session: Session,
    messages: Vec<Message>,
    memory: MemoryManager,
    mcp_registry: Option<McpRegistry>,
}

impl Agent {
    pub fn new(client: PrismClient, config: Config, task: &str, mcp_registry: Option<McpRegistry>) -> Self {
        let episode_id = Uuid::new_v4();
        let session = Session::new(episode_id, task, &config.prism_model);
        let memory_dir = crate::config::prism_home().join("memory");
        let memory = MemoryManager::new(&memory_dir, config.memory_window_size);
        Self {
            client,
            config,
            session,
            messages: Vec::new(),
            memory,
            mcp_registry,
        }
    }

    pub fn from_session(client: PrismClient, config: Config, session: Session, mcp_registry: Option<McpRegistry>) -> Self {
        let messages = session.messages.clone();
        let memory_dir = crate::config::prism_home().join("memory");
        let memory = MemoryManager::new(&memory_dir, config.memory_window_size);
        Self {
            client,
            config,
            session,
            messages,
            memory,
            mcp_registry,
        }
    }

    pub async fn run(&mut self, task: &str) -> Result<()> {
        tracing::info!(episode_id = %self.session.episode_id, model = %self.config.prism_model, "starting agent session");

        let memory_content = self.memory.load();
        let mcp_section = self
            .mcp_registry
            .as_ref()
            .map(|r| r.system_prompt_section())
            .unwrap_or("");

        let full_system = build_system_prompt(
            self.config.system_prompt.as_deref(),
            &memory_content,
            "",
            mcp_section,
        );

        self.messages.push(common::system_message(full_system));
        self.messages.push(Message {
            role: "user".into(),
            content: Some(json!(task)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        });

        self.inner_run().await
    }

    pub async fn resume(&mut self, task: &str) -> Result<()> {
        tracing::info!(
            episode_id = %self.session.episode_id,
            model = %self.config.prism_model,
            turns_so_far = %self.session.turns,
            "resuming agent session"
        );

        if !task.is_empty() {
            self.messages.push(Message {
                role: "user".into(),
                content: Some(json!(task)),
                name: None,
                tool_calls: None,
                tool_call_id: None,
                extra: Default::default(),
            });
        }

        self.inner_run().await
    }

    async fn inner_run(&mut self) -> Result<()> {
        let interrupted = Arc::new(AtomicBool::new(false));
        let flag = interrupted.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            flag.store(true, Ordering::SeqCst);
        });

        let mut total_prompt: u32 = 0;
        let mut total_completion: u32 = 0;
        let mut total_cost_usd: f64 = 0.0;
        let mut turns: u32 = 0;
        let mut model_name = self.config.prism_model.clone();
        let mut stop_reason: Option<String> = None;

        for _turn in 0..self.config.max_turns {
            if interrupted.load(Ordering::Relaxed) {
                eprintln!("\n[interrupt] Ctrl+C — stopping");
                stop_reason = Some("interrupt".to_string());
                break;
            }

            let req = ChatCompletionRequest {
                model: self.config.prism_model.clone(),
                messages: self.messages.clone(),
                tools: Some(tools::all_tool_definitions(self.mcp_registry.as_ref())),
                tool_choice: Some(json!("auto")),
                ..Default::default()
            };

            let mut stream = self
                .client
                .stream_chat_completion(&req)
                .await
                .map_err(|e| anyhow!("stream_chat_completion failed: {e}"))?;

            turns += 1;

            let mut content_buf = String::new();
            let mut tc_builders: HashMap<usize, ToolCallBuilder> = HashMap::new();
            let mut finish_reason: Option<String> = None;

            while let Some(chunk_result) = stream.next().await {
                if interrupted.load(Ordering::Relaxed) {
                    eprintln!("\n[interrupt] Ctrl+C — stopping");
                    stop_reason = Some("interrupt".to_string());
                    break;
                }

                let chunk = chunk_result.map_err(|e| anyhow!("stream error: {e}"))?;

                if !chunk.delta.is_empty() {
                    print!("{}", chunk.delta);
                    let _ = std::io::stdout().flush();
                    content_buf.push_str(&chunk.delta);
                }

                if let Some(tc_arr) = chunk
                    .tool_calls
                    .as_ref()
                    .and_then(|v: &serde_json::Value| v.as_array())
                {
                    accumulate_tool_call_deltas(tc_arr, &mut tc_builders);
                }

                if chunk.finish_reason.is_some() {
                    finish_reason = chunk.finish_reason;
                }

                if let Some(u) = &chunk.usage {
                    total_prompt += u.prompt_tokens;
                    total_completion += u.completion_tokens;
                    model_name = self.config.prism_model.clone();

                    let (in_rate, out_rate): (f64, f64) = match model_name.as_str() {
                        m if m.contains("claude-opus-4") => (5.50, 27.50),
                        m if m.contains("claude-sonnet-4") => (3.30, 16.50),
                        m if m.contains("claude-haiku-4") => (1.00, 5.00),
                        m if m.contains("gpt-5") && m.contains("codex") => (1.75, 14.0),
                        m if m.contains("gpt-5") => (1.75, 14.0),
                        m if m.contains("gpt-4o-mini") => (0.15, 0.6),
                        m if m.contains("gpt-4o") => (2.5, 10.0),
                        m if m.contains("qwen3") => (0.15, 1.20),
                        m if m.contains("kimi") => (0.60, 3.00),
                        m if m.contains("gpt-oss") => (0.15, 0.60),
                        m if m.contains("minimax") => (0.30, 1.20),
                        m if m.contains("gemma") => (0.23, 0.38),
                        m if m.contains("ministral") => (0.20, 0.20),
                        m if m.contains("nova") => (0.33, 2.75),
                        m if m.contains("gemini-1.5-pro") => (1.25, 5.0),
                        m if m.contains("gemini-1.5-flash") => (0.075, 0.3),
                        _ => (0.0, 0.0),
                    };
                    let turn_cost = (u.prompt_tokens as f64 * in_rate
                        + u.completion_tokens as f64 * out_rate)
                        / 1_000_000.0;
                    total_cost_usd += turn_cost;
                }
            }

            if stop_reason.as_deref() == Some("interrupt") {
                break;
            }

            let tool_calls_vec = reconstruct_tool_calls(&tc_builders);

            // Push assistant message
            self.messages.push(Message {
                role: "assistant".into(),
                content: if content_buf.is_empty() {
                    None
                } else {
                    Some(json!(content_buf))
                },
                name: None,
                tool_calls: tool_calls_vec.clone(),
                tool_call_id: None,
                extra: Default::default(),
            });

            // Update session and save after each turn
            self.session.messages = self.messages.clone();
            self.session.turns = turns;
            self.session.updated_at = chrono::Utc::now().to_rfc3339();
            if let Err(e) = self.session.save(&self.config.sessions_dir) {
                tracing::warn!("failed to save session: {e}");
            }

            // Check cost cap
            if let Some(cap) = self.config.max_cost_usd
                && total_cost_usd >= cap
            {
                eprintln!(
                    "\n[cost-cap] ${:.4} >= cap ${:.4} — stopping",
                    total_cost_usd, cap
                );
                stop_reason = Some("cost_cap".to_string());
                break;
            }

            match finish_reason.as_deref() {
                Some("stop") | None => {
                    if !content_buf.is_empty() && !content_buf.ends_with('\n') {
                        println!();
                    }
                    stop_reason = finish_reason;
                    break;
                }
                Some("tool_calls") => {
                    for tc in tool_calls_vec.unwrap_or_default() {
                        let id = tc["id"].as_str().unwrap_or("").to_string();
                        let name = tc["function"]["name"].as_str().unwrap_or("");
                        let args: serde_json::Value = tc["function"]["arguments"]
                            .as_str()
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(json!({}));

                        let args_preview = {
                            let s = args.to_string();
                            if s.len() > 120 {
                                format!("{}…", &s[..120.min(s.len())])
                            } else {
                                s
                            }
                        };
                        eprintln!("[tool] {name}  args={args_preview}");

                        let t0 = std::time::Instant::now();
                        let result = if name == "save_memory" {
                            let key = args["key"].as_str().unwrap_or("note").to_string();
                            let value = args["value"].as_str().unwrap_or("").to_string();
                            self.memory.append(key.clone(), value);
                            serde_json::json!({"saved": true, "key": key}).to_string()
                        } else if name == "spawn_agent" {
                            let task = args["task"].as_str().unwrap_or("").to_string();
                            let model = args["model"].as_str().map(str::to_string);
                            let cost_cap = args["cost_cap"].as_f64();
                            let timeout_secs = args["timeout_secs"].as_u64();
                            let spawn_config = spawn::SpawnConfig {
                                task,
                                model,
                                cost_cap,
                                tools: None,
                                timeout_secs,
                            };
                            match spawn::spawn_agent(
                                spawn_config,
                                &self.config.prism_url,
                                &self.config.prism_api_key,
                            )
                            .await
                            {
                                Ok(r) => serde_json::to_string(&r)
                                    .unwrap_or_else(|_| r.summary.clone()),
                                Err(e) => format!("{{\"status\":\"error\",\"summary\":\"{e}\"}}"),
                            }
                        } else {
                            tools::dispatch(name, &args, None, self.mcp_registry.as_ref()).await
                        };
                        let result =
                            truncate_tool_output(name, &result, self.config.max_tool_output);
                        let elapsed_ms = t0.elapsed().as_millis();

                        let result_preview = {
                            let trimmed = result.trim_start();
                            if trimmed.len() > 80 {
                                format!("{}…", &trimmed[..80.min(trimmed.len())])
                            } else {
                                trimmed.to_string()
                            }
                        };
                        eprintln!(
                            "[tool] {name}  {}ms  {} bytes  {result_preview}",
                            elapsed_ms,
                            result.len()
                        );

                        self.messages.push(Message {
                            role: "tool".into(),
                            content: Some(json!(result)),
                            name: None,
                            tool_calls: None,
                            tool_call_id: Some(id),
                            extra: Default::default(),
                        });
                    }
                }
                Some("cost_cap") | Some("interrupt") => {
                    break;
                }
                Some(other) => anyhow::bail!("unexpected finish_reason: {other}"),
            }
        }

        // Flush any pending memory entries to disk
        if let Err(e) = self.memory.flush() {
            tracing::warn!("memory flush failed: {e}");
        }

        let cost_str = if total_cost_usd > 0.0 {
            format!("  ~${:.4}", total_cost_usd)
        } else {
            String::new()
        };
        eprintln!(
            "[session] {}  {} turns  {} in / {} out tokens{}",
            model_name, turns, total_prompt, total_completion, cost_str
        );
        eprintln!(
            "[session] episode {}",
            &self.session.episode_id.to_string()[..8]
        );

        // Final session update
        self.session.messages = self.messages.clone();
        self.session.turns = turns;
        self.session.total_prompt_tokens = total_prompt;
        self.session.total_completion_tokens = total_completion;
        self.session.total_cost_usd = total_cost_usd;
        self.session.stop_reason = stop_reason.clone();
        self.session.updated_at = chrono::Utc::now().to_rfc3339();
        if let Err(e) = self.session.save(&self.config.sessions_dir) {
            tracing::warn!("failed to save session: {e}");
        }

        match stop_reason.as_deref() {
            Some("cost_cap") | Some("interrupt") | Some("stop") => Ok(()),
            Some(_) => Ok(()),
            None => anyhow::bail!("exceeded max_turns ({})", self.config.max_turns),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_output_unchanged() {
        let s = "hello world";
        assert_eq!(truncate_tool_output("read_file", s, 100), s);
    }

    #[test]
    fn long_output_truncated() {
        let s = "a".repeat(1000);
        let result = truncate_tool_output("read_file", &s, 100);
        assert!(result.len() < 1000);
        assert!(result.contains("chars omitted"));
        assert!(result.starts_with('a'));
        assert!(result.ends_with('a'));
    }

    #[test]
    fn run_command_json_stays_valid() {
        let stdout = "x".repeat(40_000);
        let stderr = "e".repeat(40_000);
        let input = serde_json::json!({
            "exit_code": 0,
            "stdout": stdout,
            "stderr": stderr,
        })
        .to_string();

        let result = truncate_tool_output("run_command", &input, 32_768);
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("must be valid JSON");
        assert_eq!(parsed["exit_code"], 0);
        assert!(parsed["stdout"].as_str().unwrap().contains("chars omitted"));
        assert!(parsed["stderr"].as_str().unwrap().contains("chars omitted"));
    }

    #[test]
    fn non_ascii_no_panic() {
        let s = "🦀".repeat(10_000);
        let result = truncate_tool_output("read_file", &s, 100);
        assert!(result.contains("chars omitted"));
    }
}
