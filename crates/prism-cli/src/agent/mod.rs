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
            let choice = resp.choices.into_iter().next().ok_or_else(|| anyhow!("no choices"))?;

            self.messages.push(choice.message.clone());

            match choice.finish_reason.as_deref() {
                Some("stop") | None => {
                    if let Some(c) = &choice.message.content {
                        println!("{}", c.as_str().unwrap_or(&c.to_string()));
                    }
                    return Ok(());
                }
                Some("tool_calls") => {
                    for tc in choice.message.tool_calls.unwrap_or_default() {
                        let id = tc["id"].as_str().unwrap_or("").to_string();
                        let name = tc["function"]["name"].as_str().unwrap_or("");
                        let args: serde_json::Value = tc["function"]["arguments"]
                            .as_str()
                            .and_then(|s| serde_json::from_str(s).ok())
                            .unwrap_or(json!({}));

                        eprintln!("[tool] {name}");
                        let result = tools::dispatch(name, &args).await;

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

        anyhow::bail!("exceeded max_turns ({})", self.config.max_turns)
    }
}
