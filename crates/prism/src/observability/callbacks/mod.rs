pub mod datadog;
pub mod helicone;
pub mod langfuse;

use async_trait::async_trait;

use crate::error::Result;
use crate::types::InferenceEvent;

#[async_trait]
pub trait ObservabilityCallback: Send + Sync {
    fn name(&self) -> &str;
    async fn on_inference_event(&self, event: &InferenceEvent) -> Result<()>;
    async fn on_batch(&self, events: &[InferenceEvent]) -> Result<()> {
        for event in events {
            if let Err(e) = self.on_inference_event(event).await {
                tracing::warn!(callback = self.name(), error = %e, "callback failed for event");
            }
        }
        Ok(())
    }
}

pub struct CallbackRegistry {
    callbacks: Vec<Box<dyn ObservabilityCallback>>,
}

impl CallbackRegistry {
    pub fn new() -> Self {
        Self {
            callbacks: Vec::new(),
        }
    }

    pub fn register(&mut self, callback: Box<dyn ObservabilityCallback>) {
        tracing::info!(
            callback = callback.name(),
            "registered observability callback"
        );
        self.callbacks.push(callback);
    }

    pub async fn fire_event(&self, event: &InferenceEvent) {
        for callback in &self.callbacks {
            if let Err(e) = callback.on_inference_event(event).await {
                tracing::warn!(callback = callback.name(), error = %e, "callback fire_event failed");
            }
        }
    }

    pub async fn fire_batch(&self, events: &[InferenceEvent]) {
        for callback in &self.callbacks {
            if let Err(e) = callback.on_batch(events).await {
                tracing::warn!(callback = callback.name(), error = %e, "callback fire_batch failed");
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.callbacks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EventStatus, InferenceEvent};
    use chrono::Utc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    use uuid::Uuid;

    struct MockCallback {
        call_count: Arc<AtomicU32>,
    }

    #[async_trait]
    impl ObservabilityCallback for MockCallback {
        fn name(&self) -> &str {
            "mock"
        }
        async fn on_inference_event(&self, _event: &InferenceEvent) -> Result<()> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FailingCallback;

    #[async_trait]
    impl ObservabilityCallback for FailingCallback {
        fn name(&self) -> &str {
            "failing"
        }
        async fn on_inference_event(&self, _event: &InferenceEvent) -> Result<()> {
            Err(crate::error::PrismError::Internal("test error".into()))
        }
    }

    fn test_event() -> InferenceEvent {
        InferenceEvent {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            provider: "test".into(),
            model: "test-model".into(),
            status: EventStatus::Success,
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            cache_read_input_tokens: 0,
            cache_creation_input_tokens: 0,
            estimated_cost_usd: 0.001,
            latency_ms: 100,
            prompt_hash: "abc".into(),
            completion_hash: "def".into(),
            task_type: None,
            routing_decision: None,
            variant_name: None,
            virtual_key_hash: None,
            team_id: None,
            end_user_id: None,
            episode_id: None,
            metadata: String::new(),
            trace_id: None,
            span_id: None,
            parent_span_id: None,
            agent_framework: None,
            tool_calls_json: None,
            ttft_ms: None,
            session_id: None,
            thread_id: None,
            provider_attempted: None,
        }
    }

    #[tokio::test]
    async fn registry_fire_event() {
        let count = Arc::new(AtomicU32::new(0));
        let mut registry = CallbackRegistry::new();
        registry.register(Box::new(MockCallback {
            call_count: count.clone(),
        }));
        registry.fire_event(&test_event()).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn registry_fire_batch() {
        let count = Arc::new(AtomicU32::new(0));
        let mut registry = CallbackRegistry::new();
        registry.register(Box::new(MockCallback {
            call_count: count.clone(),
        }));
        let events = vec![test_event(), test_event(), test_event()];
        registry.fire_batch(&events).await;
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn failing_callback_doesnt_block_others() {
        let count = Arc::new(AtomicU32::new(0));
        let mut registry = CallbackRegistry::new();
        registry.register(Box::new(FailingCallback));
        registry.register(Box::new(MockCallback {
            call_count: count.clone(),
        }));
        registry.fire_event(&test_event()).await;
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}
