use anyhow::{anyhow, Result};
use prism_client::PrismClient;
use prism_types::{ChatCompletionRequest, Message};
use serde_json::json;
use uuid::Uuid;

use crate::config::Config;
use crate::tools;

const SYSTEM_PROMPT: &str = "You are PrisM Code Agent. Use tools to complete the coding task. \
When done, provide a clear summary.";

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

        let mut total_prompt: u32 = 0;
        let mut total_completion: u32 = 0;
        let mut turns: u32 = 0;
        let mut model_name = self.config.prism_model.clone();
        let mut stop_reason: Option<String> = None;

        for _turn in 0..self.config.max_turns {
            let req = ChatCompletionRequest {
                model: self.config.prism_model.clone(),
                messages: self.messages.clone(),
                tools: Some(tools::tool_definitions()),
                tool_choice: Some(json!("auto")),
                ..Default::default()
            };

            let resp = self.client.chat_completion(&req).await
                .map_err(|e| anyhow!("chat_completion failed: {e}"))?;
            turns += 1;
            model_name = resp.model.clone();
            if let Some(u) = &resp.usage {
                total_prompt += u.prompt_tokens;
                total_completion += u.completion_tokens;
            }
            let choice = resp.choices.into_iter().next().ok_or_else(|| anyhow!("no choices"))?;

            self.messages.push(choice.message.clone());

            match choice.finish_reason.as_deref() {
                Some("stop") | None => {
                    if let Some(c) = &choice.message.content {
                        println!("{}", c.as_str().unwrap_or(&c.to_string()));
                    }
                    stop_reason = choice.finish_reason.clone();
                    break;
                }
                Some("tool_calls") => {
                    for tc in choice.message.tool_calls.unwrap_or_default() {
                        let id = tc["id"].as_str().unwrap_or("").to_string();
                        let name = tc["function"]["name"].as_str().unwrap_or("");
                        let args: serde_json::Value = tc["function"]["arguments"]
                            .as_str()
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(json!({}));

                        let args_preview = {
                            let s = args.to_string();
                            if s.len() > 120 { format!("{}…", &s[..120.min(s.len())]) } else { s }
                        };
                        eprintln!("[tool] {name}  args={args_preview}");

                        let t0 = std::time::Instant::now();
                        let result = tools::dispatch(name, &args).await;
                        let elapsed_ms = t0.elapsed().as_millis();

                        let result_preview = {
                            let trimmed = result.trim_start();
                            if trimmed.len() > 80 { format!("{}…", &trimmed[..80.min(trimmed.len())]) } else { trimmed.to_string() }
                        };
                        eprintln!("[tool] {name}  {}ms  {} bytes  {result_preview}", elapsed_ms, result.len());

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
                Some(other) => anyhow::bail!("unexpected finish_reason: {other}"),
            }
        }

        let cost_usd = {
            let (in_rate, out_rate): (f64, f64) = match model_name.as_str() {
                m if m.contains("claude-opus-4")     => (15.0,  75.0),
                m if m.contains("claude-sonnet-4")   => (3.0,   15.0),
                m if m.contains("claude-haiku-4")    => (0.8,    4.0),
                m if m.contains("gpt-4o-mini")       => (0.15,   0.6),
                m if m.contains("gpt-4o")            => (2.5,   10.0),
                m if m.contains("gemini-1.5-pro")    => (1.25,   5.0),
                m if m.contains("gemini-1.5-flash")  => (0.075,  0.3),
                _                                    => (0.0,    0.0),
            };
            (total_prompt as f64 * in_rate + total_completion as f64 * out_rate) / 1_000_000.0
        };
        let cost_str = if cost_usd > 0.0 { format!("  ~${:.4}", cost_usd) } else { String::new() };
        eprintln!(
            "[session] {}  {} turns  {} in / {} out tokens{}",
            model_name, turns, total_prompt, total_completion, cost_str
        );

        if stop_reason.is_some() {
            Ok(())
        } else {
            anyhow::bail!("exceeded max_turns ({})", self.config.max_turns)
        }
    }
}
