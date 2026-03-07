use std::collections::HashMap;
use std::sync::RwLock;

use super::types::AgentCapability;

pub struct DiscoveryBridge {
    capabilities: RwLock<HashMap<String, AgentCapability>>,
}

impl DiscoveryBridge {
    pub fn new() -> Self {
        Self {
            capabilities: RwLock::new(HashMap::new()),
        }
    }

    pub fn register(&self, capability: AgentCapability) {
        let mut caps = self.capabilities.write().unwrap();
        tracing::info!(listing_id = %capability.listing_id, "registered agent capability");
        caps.insert(capability.listing_id.clone(), capability);
    }

    pub fn discover(&self, method: Option<&str>, framework: Option<&str>) -> Vec<AgentCapability> {
        let caps = self.capabilities.read().unwrap();
        caps.values()
            .filter(|cap| {
                let method_match = method
                    .map(|m| cap.methods.iter().any(|cm| cm == m))
                    .unwrap_or(true);
                let framework_match = framework
                    .map(|f| cap.supported_frameworks.iter().any(|sf| sf == f))
                    .unwrap_or(true);
                method_match && framework_match
            })
            .cloned()
            .collect()
    }

    pub fn get(&self, listing_id: &str) -> Option<AgentCapability> {
        self.capabilities.read().unwrap().get(listing_id).cloned()
    }

    pub fn unregister(&self, listing_id: &str) -> bool {
        self.capabilities
            .write()
            .unwrap()
            .remove(listing_id)
            .is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cap(id: &str, methods: &[&str], frameworks: &[&str]) -> AgentCapability {
        AgentCapability {
            listing_id: id.into(),
            methods: methods.iter().map(|s| s.to_string()).collect(),
            input_schema: None,
            output_schema: None,
            supported_frameworks: frameworks.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn register_and_get() {
        let bridge = DiscoveryBridge::new();
        bridge.register(test_cap("agent-1", &["process"], &["langchain"]));
        assert!(bridge.get("agent-1").is_some());
        assert!(bridge.get("nonexistent").is_none());
    }

    #[test]
    fn discover_by_method() {
        let bridge = DiscoveryBridge::new();
        bridge.register(test_cap("a1", &["process", "analyze"], &["langchain"]));
        bridge.register(test_cap("a2", &["summarize"], &["crewai"]));

        let results = bridge.discover(Some("process"), None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].listing_id, "a1");
    }

    #[test]
    fn discover_by_framework() {
        let bridge = DiscoveryBridge::new();
        bridge.register(test_cap("a1", &["process"], &["langchain"]));
        bridge.register(test_cap("a2", &["summarize"], &["langchain", "crewai"]));
        bridge.register(test_cap("a3", &["analyze"], &["autogen"]));

        let results = bridge.discover(None, Some("langchain"));
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn discover_all() {
        let bridge = DiscoveryBridge::new();
        bridge.register(test_cap("a1", &["process"], &["langchain"]));
        bridge.register(test_cap("a2", &["summarize"], &["crewai"]));

        let results = bridge.discover(None, None);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn unregister() {
        let bridge = DiscoveryBridge::new();
        bridge.register(test_cap("a1", &["process"], &["langchain"]));
        assert!(bridge.unregister("a1"));
        assert!(!bridge.unregister("a1"));
        assert!(bridge.get("a1").is_none());
    }
}
