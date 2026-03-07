use dashmap::DashMap;
use serde::Serialize;
use uuid::Uuid;

use super::types::MeteringRecord;

pub struct MeteringStore {
    records: DashMap<Uuid, MeteringRecord>,
}

impl MeteringStore {
    pub fn new() -> Self {
        Self {
            records: DashMap::new(),
        }
    }

    pub fn record(&self, record: MeteringRecord) {
        self.records.insert(record.invocation_id, record);
    }

    pub fn get(&self, invocation_id: &Uuid) -> Option<MeteringRecord> {
        self.records.get(invocation_id).map(|r| r.clone())
    }

    pub fn get_by_caller(&self, caller_id: &str) -> Vec<MeteringRecord> {
        self.records
            .iter()
            .filter(|r| r.caller_id == caller_id)
            .map(|r| r.clone())
            .collect()
    }

    pub fn summary(&self) -> MeteringSummary {
        let mut total_cost = 0.0;
        let mut total_tokens = 0u64;
        let mut total_latency_ms = 0u64;
        let total_invocations = self.records.len();

        for entry in self.records.iter() {
            total_cost += entry.cost;
            total_tokens += entry.tokens_used;
            total_latency_ms += entry.latency_ms;
        }

        MeteringSummary {
            total_invocations,
            total_cost,
            total_tokens,
            total_latency_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MeteringSummary {
    pub total_invocations: usize,
    pub total_cost: f64,
    pub total_tokens: u64,
    pub total_latency_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_record(caller: &str, target: &str, cost: f64, tokens: u64) -> MeteringRecord {
        MeteringRecord {
            invocation_id: Uuid::new_v4(),
            caller_id: caller.into(),
            target_id: target.into(),
            tokens_used: tokens,
            cost,
            latency_ms: 100,
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn record_and_get() {
        let store = MeteringStore::new();
        let rec = make_record("a", "b", 0.01, 100);
        let id = rec.invocation_id;
        store.record(rec);
        assert!(store.get(&id).is_some());
    }

    #[test]
    fn get_by_caller() {
        let store = MeteringStore::new();
        store.record(make_record("alice", "bob", 0.01, 100));
        store.record(make_record("alice", "charlie", 0.02, 200));
        store.record(make_record("dave", "bob", 0.03, 300));

        let alice_records = store.get_by_caller("alice");
        assert_eq!(alice_records.len(), 2);
    }

    #[test]
    fn summary() {
        let store = MeteringStore::new();
        store.record(make_record("a", "b", 0.01, 100));
        store.record(make_record("c", "d", 0.02, 200));

        let summary = store.summary();
        assert_eq!(summary.total_invocations, 2);
        assert!((summary.total_cost - 0.03).abs() < f64::EPSILON);
        assert_eq!(summary.total_tokens, 300);
    }
}
