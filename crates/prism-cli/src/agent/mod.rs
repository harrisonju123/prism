use anyhow::Result;
use prism_client::PrismClient;
use prism_types::Message;
use uuid::Uuid;

use crate::config::Config;

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

        // TODO(observability): prism-client has no per-request custom header support yet.
        // x-episode-id and x-agent-framework: prism-cli cannot be sent until PrismClient
        // gains a with_header() builder or the loop constructs its own reqwest client.

        // Phase 1: Plan (stub)
        println!("Planning: {task}");

        // Phase 2: Execute (stub — empty loop)

        // Phase 3: Reflect (stub)
        println!("Done.");

        Ok(())
    }
}
