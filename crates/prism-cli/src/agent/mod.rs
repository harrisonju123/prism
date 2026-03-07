use anyhow::{anyhow, Result};
use futures::StreamExt;
use prism_client::PrismClient;
use prism_types::{ChatCompletionRequest, Message};
use serde_json::json;
use std::collections::HashMap;
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use uuid::Uuid;

use crate::config::Config;
use crate::tools;

const SYSTEM_PROMPT: &str = "You are PrisM Code Agent. Use tools to complete the coding task. \
When done, provide a clear summary.";

struct ToolCallBuilder {
    id: String,
    tc_type: String,
    name: String,
    arguments_buf: String,
}

pub struct Agent {
    client: PrismClient,
    config: Config,
    episode_id: Uuid,
    messages: Vec<Message>,
}

impl Agent {
    pub fn new(client: PrismClient, config: Config) -> Self {
        Self {
            client,
            config,
            episode_id: Uuid::new_v4(),
            messages: Vec::new(),
        }
    }

    pub async fn run(&mut self, task: &str) -> Result<()> {
        tracing::info!(episode_id = %self.episode_id, model = %self.config.prism_model, "starting agent session");

        self.messages.push(Message {
            role: "system".into(),
            content: Some(json!(SYSTEM_PROMPT)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        });
        self.messages.push(Message {
            role: "user".into(),
            content: Some(json!(task)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        });

        // SIGINT handler
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
                tools: Some(tools::tool_definitions()),
                tool_choice: Some(json!("auto")),
                ..Default::default()
            };

            let mut stream = self
                .client
                .stream_chat_completion(&req)
                .await
                .map_err(|e| anyhow!("stream_chat_completion failed: {e}"))?;

            turns += 1;

            // Accumulate streaming response
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

                // Print content delta immediately
                if !chunk.delta.is_empty() {
                    print!("{}", chunk.delta);
                    let _ = std::io::stdout().flush();
                    content_buf.push_str(&chunk.delta);
                }

                // Accumulate tool_call deltas
                if let Some(tc_arr) = chunk.tool_calls.as_ref().and_then(|v: &serde_json::Value| v.as_array()) {
                    for tc in tc_arr {
                        let tc: &serde_json::Value = tc;
                        let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
                        let builder = tc_builders.entry(idx).or_insert_with(|| ToolCallBuilder {
                            id: String::new(),
                            tc_type: "function".to_string(),
                            name: String::new(),
                            arguments_buf: String::new(),
                        });
                        if let Some(id) = tc.get("id").and_then(|v: &serde_json::Value| v.as_str()) {
                            if !id.is_empty() {
                                builder.id = id.to_string();
                            }
                        }
                        if let Some(t) = tc.get("type").and_then(|v: &serde_json::Value| v.as_str()) {
                            if !t.is_empty() {
                                builder.tc_type = t.to_string();
                            }
                        }
                        if let Some(fname) = tc
                            .get("function")
                            .and_then(|f: &serde_json::Value| f.get("name"))
                            .and_then(|v: &serde_json::Value| v.as_str())
                        {
                            if !fname.is_empty() {
                                builder.name.push_str(fname);
                            }
                        }
                        if let Some(args_frag) = tc
                            .get("function")
                            .and_then(|f: &serde_json::Value| f.get("arguments"))
                            .and_then(|v: &serde_json::Value| v.as_str())
                        {
                            builder.arguments_buf.push_str(args_frag);
                        }
                    }
                }

                if chunk.finish_reason.is_some() {
                    finish_reason = chunk.finish_reason;
                }

                // Capture usage from final chunk
                if let Some(u) = &chunk.usage {
                    total_prompt += u.prompt_tokens;
                    total_completion += u.completion_tokens;
                    model_name = self.config.prism_model.clone();

                    let (in_rate, out_rate): (f64, f64) = match model_name.as_str() {
                        m if m.contains("claude-opus-4")    => (15.0,  75.0),
                        m if m.contains("claude-sonnet-4")  => (3.0,   15.0),
                        m if m.contains("claude-haiku-4")   => (0.8,    4.0),
                        m if m.contains("gpt-4o-mini")      => (0.15,   0.6),
                        m if m.contains("gpt-4o")           => (2.5,   10.0),
                        m if m.contains("gemini-1.5-pro")   => (1.25,   5.0),
                        m if m.contains("gemini-1.5-flash") => (0.075,  0.3),
                        _                                   => (0.0,    0.0),
                    };
                    let turn_cost = (u.prompt_tokens as f64 * in_rate
                        + u.completion_tokens as f64 * out_rate)
                        / 1_000_000.0;
                    total_cost_usd += turn_cost;
                }
            }

            // If interrupted mid-stream, break out of turn loop
            if stop_reason.as_deref() == Some("interrupt") {
                break;
            }

            // Reconstruct tool_calls vec in index order
            let tool_calls_vec: Option<Vec<serde_json::Value>> = if tc_builders.is_empty() {
                None
            } else {
                let mut indices: Vec<usize> = tc_builders.keys().cloned().collect();
                indices.sort_unstable();
                Some(
                    indices
                        .iter()
                        .map(|i| {
                            let b = &tc_builders[i];
                            json!({
                                "id": b.id,
                                "type": b.tc_type,
                                "function": {
                                    "name": b.name,
                                    "arguments": b.arguments_buf
                                }
                            })
                        })
                        .collect(),
                )
            };

            // Push assistant message
            self.messages.push(Message {
                role: "assistant".into(),
                content: if content_buf.is_empty() { None } else { Some(json!(content_buf)) },
                name: None,
                tool_calls: tool_calls_vec.clone(),
                tool_call_id: None,
                extra: Default::default(),
            });

            // Check cost cap
            if let Some(cap) = self.config.max_cost_usd {
                if total_cost_usd >= cap {
                    eprintln!(
                        "\n[cost-cap] ${:.4} >= cap ${:.4} — stopping",
                        total_cost_usd, cap
                    );
                    stop_reason = Some("cost_cap".to_string());
                    break;
                }
            }

            match finish_reason.as_deref() {
                Some("stop") | None => {
                    // Content was already printed during streaming; add newline if needed
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
                        let result = tools::dispatch(name, &args).await;
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

        let cost_str = if total_cost_usd > 0.0 {
            format!("  ~${:.4}", total_cost_usd)
        } else {
            String::new()
        };
        eprintln!(
            "[session] {}  {} turns  {} in / {} out tokens{}",
            model_name, turns, total_prompt, total_completion, cost_str
        );

        match stop_reason.as_deref() {
            Some("cost_cap") | Some("interrupt") | Some("stop") => Ok(()),
            Some(_) => Ok(()),
            None => anyhow::bail!("exceeded max_turns ({})", self.config.max_turns),
        }
    }
}
