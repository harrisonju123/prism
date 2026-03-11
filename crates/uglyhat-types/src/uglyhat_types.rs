//! Pure serde domain types for uglyhat context management.
//!
//! This crate is a lightweight facade — no sqlx, no tokio, just the types
//! needed to deserialize uglyhat data in Zed UI crates (prism-hq, etc.).
//!
//! The canonical implementations live in `uglyhat::model`; these types are
//! kept in sync by construction (they are a verbatim copy minus the sqlx
//! derive impls that belong to the store layer).

pub use uglyhat::model::{
    ActivityEntry, Agent, AgentSession, AgentState, AgentStatus, CheckinContext, Decision,
    DecisionScope, DecisionStatus, GuardrailCheck, Handoff, HandoffConstraints, HandoffMode,
    HandoffStatus, Memory, RecallResult, Snapshot, Thread, ThreadContext, ThreadGuardrails,
    ThreadStatus, Workspace, WorkspaceOverview,
};
