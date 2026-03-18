use prism_client::{ChatCompletionRequest, ChatCompletionResponse, PrismClient};
use prism_context::model::WorkPackageStatus;
use prism_types::{Message, MessageRole};
use uuid::Uuid;

pub enum FilterVerdict {
    Silent,
    Escalate { summary: String, options: Vec<String> },
    Relay { to_agent: String, message: String },
}

pub struct ProgressEvent {
    pub wp_id: Uuid,
    pub wp_intent: String,
    pub agent_name: String,
    pub progress_note: String,
    pub wp_status: WorkPackageStatus,
}

pub struct OrchestratorFilter {
    client: PrismClient,
    model: String,
}

impl OrchestratorFilter {
    pub fn new(base_url: String, api_key: Option<String>, model: String) -> Self {
        let client = match api_key {
            Some(key) => PrismClient::new(base_url).with_api_key(key),
            None => PrismClient::new(base_url),
        };
        Self { client, model }
    }

    pub async fn classify(&self, event: &ProgressEvent) -> FilterVerdict {
        let system = "You are a supervisor orchestrator. Classify agent progress notes.\n\
            Respond with exactly one of:\n\
            SILENT — routine progress, no human input needed.\n\
            ESCALATE:<summary>|<option1>|<option2> — requires human judgment.\n\
            RELAY:<agent>:<message> — forward info to another agent.\n\
            Only respond with the classification, nothing else.";

        let user = format!(
            "Agent: {}\nWork package: {}\nStatus: {:?}\nProgress note: {}",
            event.agent_name, event.wp_intent, event.wp_status, event.progress_note
        );

        let req = ChatCompletionRequest {
            model: self.model.clone(),
            messages: vec![
                Message {
                    role: MessageRole::System,
                    content: Some(serde_json::Value::String(system.to_string())),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
                Message {
                    role: MessageRole::User,
                    content: Some(serde_json::Value::String(user)),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                    extra: Default::default(),
                },
            ],
            max_tokens: Some(200),
            ..Default::default()
        };

        match self.client.chat_completion(&req).await {
            Ok(resp) => parse_verdict(&resp),
            Err(e) => {
                log::warn!("orchestrator filter LLM error: {e}");
                FilterVerdict::Silent
            }
        }
    }
}

fn parse_verdict(resp: &ChatCompletionResponse) -> FilterVerdict {
    let text = resp
        .choices
        .first()
        .and_then(|c| c.message.content.as_ref())
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim();

    if let Some(rest) = text.strip_prefix("ESCALATE:") {
        let mut parts = rest.splitn(3, '|');
        let summary = parts.next().unwrap_or("Needs review").trim().to_string();
        let options: Vec<String> = parts.map(|s| s.trim().to_string()).collect();
        let options = if options.is_empty() {
            vec!["Continue".to_string(), "Abort".to_string()]
        } else {
            options
        };
        return FilterVerdict::Escalate { summary, options };
    }

    if let Some(rest) = text.strip_prefix("RELAY:") {
        if let Some((agent, msg)) = rest.split_once(':') {
            return FilterVerdict::Relay {
                to_agent: agent.trim().to_string(),
                message: msg.trim().to_string(),
            };
        }
    }

    FilterVerdict::Silent
}
