use crate::model::*;
use crate::store::sqlite::SqliteStore;
use crate::store::{ActivityFilters, MemoryFilters, Store};

async fn setup() -> (SqliteStore, Workspace) {
    let store = SqliteStore::open_memory().await.expect("open memory db");
    let ws = store
        .init_workspace("test-workspace", "A test workspace")
        .await
        .expect("init workspace");
    (store, ws)
}

#[tokio::test]
async fn test_init_workspace() {
    let (store, ws) = setup().await;
    assert_eq!(ws.name, "test-workspace");
    assert_eq!(ws.description, "A test workspace");

    let fetched = store.get_workspace(ws.id).await.expect("get workspace");
    assert_eq!(fetched.id, ws.id);
    assert_eq!(fetched.name, "test-workspace");
}

#[tokio::test]
async fn test_thread_lifecycle() {
    let (store, ws) = setup().await;

    let t = store
        .create_thread(
            ws.id,
            "auth-refactor",
            "Refactoring auth",
            vec!["auth".into()],
        )
        .await
        .expect("create thread");
    assert_eq!(t.name, "auth-refactor");
    assert_eq!(t.status, ThreadStatus::Active);
    assert_eq!(t.tags, vec!["auth"]);

    let fetched = store
        .get_thread(ws.id, "auth-refactor")
        .await
        .expect("get thread");
    assert_eq!(fetched.id, t.id);

    let threads = store
        .list_threads(ws.id, Some(ThreadStatus::Active))
        .await
        .expect("list threads");
    assert_eq!(threads.len(), 1);

    let archived = store
        .archive_thread(ws.id, "auth-refactor")
        .await
        .expect("archive thread");
    assert_eq!(archived.status, ThreadStatus::Archived);

    let active = store
        .list_threads(ws.id, Some(ThreadStatus::Active))
        .await
        .expect("list active");
    assert!(active.is_empty());
}

#[tokio::test]
async fn test_thread_duplicate_name() {
    let (store, ws) = setup().await;

    store
        .create_thread(ws.id, "my-thread", "", vec![])
        .await
        .expect("first create");

    let err = store
        .create_thread(ws.id, "my-thread", "", vec![])
        .await
        .expect_err("duplicate should fail");
    assert!(err.to_string().contains("already exists"));
}

#[tokio::test]
async fn test_memory_upsert() {
    let (store, ws) = setup().await;

    let m1 = store
        .save_memory(
            ws.id,
            "project_lang",
            "Rust",
            None,
            "agent",
            vec!["meta".into()],
        )
        .await
        .expect("save memory");
    assert_eq!(m1.key, "project_lang");
    assert_eq!(m1.value, "Rust");

    // Upsert with same key overwrites
    let m2 = store
        .save_memory(
            ws.id,
            "project_lang",
            "Rust + TypeScript",
            None,
            "agent",
            vec![],
        )
        .await
        .expect("upsert memory");
    assert_eq!(m2.key, "project_lang");
    assert_eq!(m2.value, "Rust + TypeScript");
    // ID should be the same (upsert)
    assert_eq!(m2.id, m1.id);

    let memories = store
        .load_memories(ws.id, MemoryFilters::default())
        .await
        .expect("load memories");
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].value, "Rust + TypeScript");
}

#[tokio::test]
async fn test_memory_with_thread() {
    let (store, ws) = setup().await;

    let t = store
        .create_thread(ws.id, "api-v2", "", vec![])
        .await
        .expect("create thread");

    store
        .save_memory(
            ws.id,
            "api_approach",
            "REST + gRPC",
            Some(t.id),
            "agent",
            vec![],
        )
        .await
        .expect("save with thread");

    store
        .save_memory(ws.id, "global_fact", "uses cargo", None, "agent", vec![])
        .await
        .expect("save global");

    // Filter by thread
    let thread_mems = store
        .load_memories(
            ws.id,
            MemoryFilters {
                thread_id: Some(t.id),
                ..Default::default()
            },
        )
        .await
        .expect("load by thread");
    assert_eq!(thread_mems.len(), 1);
    assert_eq!(thread_mems[0].key, "api_approach");

    // Filter by thread name
    let by_name = store
        .load_memories(
            ws.id,
            MemoryFilters {
                thread_name: Some("api-v2".into()),
                ..Default::default()
            },
        )
        .await
        .expect("load by thread name");
    assert_eq!(by_name.len(), 1);

    // Global only
    let global = store
        .load_memories(
            ws.id,
            MemoryFilters {
                global_only: true,
                ..Default::default()
            },
        )
        .await
        .expect("load global");
    assert_eq!(global.len(), 1);
    assert_eq!(global[0].key, "global_fact");
}

#[tokio::test]
async fn test_delete_memory() {
    let (store, ws) = setup().await;

    store
        .save_memory(ws.id, "temp_fact", "something", None, "agent", vec![])
        .await
        .expect("save");

    store
        .delete_memory(ws.id, "temp_fact")
        .await
        .expect("delete");

    let memories = store
        .load_memories(ws.id, MemoryFilters::default())
        .await
        .expect("load");
    assert!(memories.is_empty());

    // Double delete should error
    let err = store
        .delete_memory(ws.id, "temp_fact")
        .await
        .expect_err("not found");
    assert!(err.to_string().contains("not found"));
}

#[tokio::test]
async fn test_decisions() {
    let (store, ws) = setup().await;

    let t = store
        .create_thread(ws.id, "arch", "", vec![])
        .await
        .expect("create thread");

    let d = store
        .save_decision(
            ws.id,
            "Use JWT",
            "JWT with refresh tokens",
            Some(t.id),
            vec!["auth".into()],
            DecisionScope::Thread,
        )
        .await
        .expect("save decision");
    assert_eq!(d.title, "Use JWT");
    assert_eq!(d.status, DecisionStatus::Active);
    assert_eq!(d.scope, DecisionScope::Thread);

    store
        .save_decision(
            ws.id,
            "Global decision",
            "something",
            None,
            vec![],
            DecisionScope::Thread,
        )
        .await
        .expect("save global decision");

    // Filter by thread
    let thread_decisions = store
        .list_decisions(ws.id, Some(t.id), None)
        .await
        .expect("list by thread");
    assert_eq!(thread_decisions.len(), 1);
    assert_eq!(thread_decisions[0].title, "Use JWT");

    // All decisions
    let all = store
        .list_decisions(ws.id, None, None)
        .await
        .expect("list all");
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn test_decision_supersede() {
    let (store, ws) = setup().await;

    let d1 = store
        .save_decision(
            ws.id,
            "Use sessions",
            "HTTP sessions",
            None,
            vec![],
            DecisionScope::Thread,
        )
        .await
        .expect("save d1");

    let d2 = store
        .supersede_decision(ws.id, d1.id, "Use JWT", "JWT is better", None, vec![])
        .await
        .expect("supersede");
    assert_eq!(d2.status, DecisionStatus::Active);
    assert_eq!(d2.supersedes, Some(d1.id));

    // Old decision should be superseded
    let all = store.list_decisions(ws.id, None, None).await.expect("list");
    let old = all.iter().find(|d| d.id == d1.id).expect("find old");
    assert_eq!(old.status, DecisionStatus::Superseded);
    assert_eq!(old.superseded_by, Some(d2.id));
}

#[tokio::test]
async fn test_decision_revoke() {
    let (store, ws) = setup().await;

    let d = store
        .save_decision(
            ws.id,
            "Bad idea",
            "nope",
            None,
            vec![],
            DecisionScope::Thread,
        )
        .await
        .expect("save");

    let revoked = store.revoke_decision(ws.id, d.id).await.expect("revoke");
    assert_eq!(revoked.status, DecisionStatus::Revoked);
}

#[tokio::test]
async fn test_agent_checkin_checkout() {
    let (store, ws) = setup().await;

    let t = store
        .create_thread(ws.id, "work-thread", "", vec![])
        .await
        .expect("create thread");

    // Checkin
    let ctx = store
        .checkin(ws.id, "claude-1", vec!["rust".into()], Some(t.id))
        .await
        .expect("checkin");
    assert_eq!(ctx.agent.name, "claude-1");
    assert_eq!(ctx.agent.state, AgentState::Idle);
    assert_eq!(ctx.session.thread_id, Some(t.id));

    // Agent appears in list
    let agents = store.list_agents(ws.id).await.expect("list agents");
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].name, "claude-1");
    assert_eq!(agents[0].state, AgentState::Idle);
    assert!(agents[0].session_open);
    assert_eq!(agents[0].current_thread.as_deref(), Some("work-thread"));

    // Checkout
    let session = store
        .checkout(
            ws.id,
            "claude-1",
            "Did some work",
            vec!["found a bug".into()],
            vec!["src/main.rs".into()],
            vec!["fix the bug".into()],
        )
        .await
        .expect("checkout");
    assert_eq!(session.summary, "Did some work");
    assert_eq!(session.findings, vec!["found a bug"]);
    assert!(session.ended_at.is_some());
}

#[tokio::test]
async fn test_agent_heartbeat_and_state() {
    let (store, ws) = setup().await;

    // Checkin first
    store
        .checkin(ws.id, "claude-1", vec![], None)
        .await
        .expect("checkin");

    // Heartbeat
    store.heartbeat(ws.id, "claude-1").await.expect("heartbeat");

    // Set state
    store
        .set_agent_state(ws.id, "claude-1", AgentState::Working)
        .await
        .expect("set state");

    let agents = store.list_agents(ws.id).await.expect("list");
    assert_eq!(agents[0].state, AgentState::Working);
    assert!(agents[0].last_heartbeat.is_some());
}

#[tokio::test]
async fn test_handoff_lifecycle() {
    let (store, ws) = setup().await;

    // Create two agents
    store
        .checkin(ws.id, "parent", vec![], None)
        .await
        .expect("checkin parent");
    store
        .checkin(ws.id, "child", vec![], None)
        .await
        .expect("checkin child");

    // Create handoff
    let h = store
        .create_handoff(
            ws.id,
            "parent",
            "fix the bug in auth.rs",
            None,
            HandoffConstraints::default(),
            HandoffMode::DelegateAndAwait,
        )
        .await
        .expect("create handoff");
    assert_eq!(h.status, HandoffStatus::Pending);
    assert_eq!(h.task, "fix the bug in auth.rs");

    // Accept
    let h2 = store
        .accept_handoff(ws.id, h.id, "child")
        .await
        .expect("accept");
    assert_eq!(h2.status, HandoffStatus::Accepted);

    // Complete
    let h3 = store
        .complete_handoff(ws.id, h.id, serde_json::json!({"fixed": true}))
        .await
        .expect("complete");
    assert_eq!(h3.status, HandoffStatus::Completed);
    assert!(h3.result.is_some());

    // List
    let all = store.list_handoffs(ws.id, None, None).await.expect("list");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].status, HandoffStatus::Completed);
}

#[tokio::test]
async fn test_guardrails() {
    let (store, ws) = setup().await;

    let t = store
        .create_thread(ws.id, "restricted", "", vec![])
        .await
        .expect("create thread");
    store
        .checkin(ws.id, "agent-a", vec![], None)
        .await
        .expect("checkin");

    // Look up agent-a's id for owner
    let ctx = store
        .checkin(ws.id, "agent-a", vec![], None)
        .await
        .expect("checkin");
    let owner_id = ctx.agent.id;

    // Set guardrails
    let g = store
        .set_guardrails(
            ws.id,
            "restricted",
            ThreadGuardrails {
                id: uuid::Uuid::new_v4(),
                thread_id: t.id,
                workspace_id: ws.id,
                owner_agent_id: Some(owner_id),
                locked: true,
                allowed_files: vec!["src/*".to_string()],
                allowed_tools: vec!["read_file".to_string(), "edit_file".to_string()],
                cost_budget_usd: Some(1.0),
                cost_spent_usd: 0.0,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        )
        .await
        .expect("set guardrails");
    assert!(g.locked);

    // Get guardrails
    let fetched = store
        .get_guardrails(ws.id, "restricted")
        .await
        .expect("get")
        .expect("should exist");
    assert_eq!(fetched.allowed_tools, vec!["read_file", "edit_file"]);

    // Check: owner can use allowed tool on allowed file
    let check = store
        .check_guardrail(
            ws.id,
            "restricted",
            "agent-a",
            "read_file",
            Some("src/main.rs"),
        )
        .await
        .expect("check");
    assert!(check.allowed);

    // Check: disallowed tool
    let check2 = store
        .check_guardrail(ws.id, "restricted", "agent-a", "bash", None)
        .await
        .expect("check");
    assert!(!check2.allowed);
    assert!(check2.reason.unwrap().contains("not in allowed tools"));

    // Check: disallowed file
    let check3 = store
        .check_guardrail(
            ws.id,
            "restricted",
            "agent-a",
            "read_file",
            Some("tests/main.rs"),
        )
        .await
        .expect("check");
    assert!(!check3.allowed);
    assert!(check3.reason.unwrap().contains("not in allowed files"));

    // Check: non-owner on locked thread
    store
        .checkin(ws.id, "agent-b", vec![], None)
        .await
        .expect("checkin b");
    let check4 = store
        .check_guardrail(
            ws.id,
            "restricted",
            "agent-b",
            "read_file",
            Some("src/main.rs"),
        )
        .await
        .expect("check");
    assert!(!check4.allowed);
    assert!(check4.reason.unwrap().contains("locked"));
}

#[tokio::test]
async fn test_guardrail_null_file_path_passes_when_files_restricted() {
    // When file_path is None (e.g. for non-write tools), allowed_files must not block the call.
    let (store, ws) = setup().await;
    let t = store
        .create_thread(ws.id, "plan-thread", "plan mode test", vec![])
        .await
        .expect("create thread");

    store
        .set_guardrails(
            ws.id,
            "plan-thread",
            ThreadGuardrails {
                id: uuid::Uuid::new_v4(),
                thread_id: t.id,
                workspace_id: ws.id,
                owner_agent_id: None,
                locked: false,
                allowed_files: vec!["/tmp/plan.md".to_string()],
                allowed_tools: vec!["read_file".to_string(), "write_file".to_string()],
                cost_budget_usd: None,
                cost_spent_usd: 0.0,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        )
        .await
        .expect("set guardrails");

    store.checkin(ws.id, "agent-x", vec![], None).await.expect("checkin");

    // file_path = None: allowed_files check is skipped entirely (read-only tools don't carry a path)
    let check = store
        .check_guardrail(ws.id, "plan-thread", "agent-x", "read_file", None)
        .await
        .expect("check");
    assert!(check.allowed, "None file_path must pass even when allowed_files is set");

    // file_path = plan file: write should be allowed
    let check2 = store
        .check_guardrail(ws.id, "plan-thread", "agent-x", "write_file", Some("/tmp/plan.md"))
        .await
        .expect("check");
    assert!(check2.allowed, "writing the plan file must be allowed");

    // file_path = other file: write must be denied
    let check3 = store
        .check_guardrail(ws.id, "plan-thread", "agent-x", "write_file", Some("/tmp/other.rs"))
        .await
        .expect("check");
    assert!(!check3.allowed, "writing outside the plan file must be denied");
}

#[tokio::test]
async fn test_decision_notifications() {
    let (store, ws) = setup().await;

    // Create two agents with open sessions
    store
        .checkin(ws.id, "agent-a", vec![], None)
        .await
        .expect("checkin a");
    store
        .checkin(ws.id, "agent-b", vec![], None)
        .await
        .expect("checkin b");

    // Agent-a makes a workspace-scoped decision
    let d = store
        .save_decision(
            ws.id,
            "Use JWT",
            "auth",
            None,
            vec![],
            DecisionScope::Workspace,
        )
        .await
        .expect("save decision");

    // Agent-b should have a pending notification
    let pending = store
        .pending_decision_notifications(ws.id, "agent-b")
        .await
        .expect("pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].title, "Use JWT");

    // Acknowledge
    store
        .acknowledge_decisions(ws.id, "agent-b", vec![d.id])
        .await
        .expect("ack");

    // No more pending
    let pending2 = store
        .pending_decision_notifications(ws.id, "agent-b")
        .await
        .expect("pending2");
    assert!(pending2.is_empty());
}

#[tokio::test]
async fn test_recall_thread() {
    let (store, ws) = setup().await;

    let t = store
        .create_thread(
            ws.id,
            "feature-x",
            "Building feature X",
            vec!["feature".into()],
        )
        .await
        .expect("create thread");

    store
        .save_memory(ws.id, "approach", "TDD", Some(t.id), "agent", vec![])
        .await
        .expect("save memory");

    store
        .save_decision(
            ws.id,
            "Use React",
            "Modern UI",
            Some(t.id),
            vec![],
            DecisionScope::Thread,
        )
        .await
        .expect("save decision");

    let ctx = store
        .recall_thread(ws.id, "feature-x")
        .await
        .expect("recall thread");
    assert_eq!(ctx.thread.name, "feature-x");
    assert_eq!(ctx.memories.len(), 1);
    assert_eq!(ctx.decisions.len(), 1);
}

#[tokio::test]
async fn test_recall_by_tags() {
    let (store, ws) = setup().await;

    store
        .save_memory(
            ws.id,
            "auth_secret",
            "use env vars",
            None,
            "agent",
            vec!["security".into()],
        )
        .await
        .expect("save");

    store
        .save_memory(
            ws.id,
            "db_choice",
            "postgres",
            None,
            "agent",
            vec!["infra".into()],
        )
        .await
        .expect("save");

    let result = store
        .recall_by_tags(ws.id, vec!["security".into()], None)
        .await
        .expect("recall by tags");
    assert_eq!(result.memories.len(), 1);
    assert_eq!(result.memories[0].key, "auth_secret");
}

#[tokio::test]
async fn test_tag_exact_match() {
    let (store, ws) = setup().await;

    store
        .save_memory(
            ws.id,
            "auth_key",
            "use JWT",
            None,
            "agent",
            vec!["auth".into()],
        )
        .await
        .expect("save auth memory");

    store
        .save_memory(
            ws.id,
            "auth_ext_key",
            "use OAuth",
            None,
            "agent",
            vec!["authentication".into()],
        )
        .await
        .expect("save authentication memory");

    // Filtering by "auth" should only return the exact match, not "authentication"
    let results = store
        .load_memories(
            ws.id,
            MemoryFilters {
                tags: Some(vec!["auth".into()]),
                ..Default::default()
            },
        )
        .await
        .expect("load by tag");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "auth_key");

    // Same for decisions
    store
        .save_decision(
            ws.id,
            "D1",
            "",
            None,
            vec!["auth".into()],
            DecisionScope::Thread,
        )
        .await
        .expect("save decision with auth tag");

    store
        .save_decision(
            ws.id,
            "D2",
            "",
            None,
            vec!["authentication".into()],
            DecisionScope::Thread,
        )
        .await
        .expect("save decision with authentication tag");

    let decisions = store
        .list_decisions(ws.id, None, Some(vec!["auth".into()]))
        .await
        .expect("list decisions by tag");
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].title, "D1");
}

#[tokio::test]
async fn test_activity_log() {
    let (store, ws) = setup().await;

    // Creating a thread generates activity
    store
        .create_thread(ws.id, "t1", "", vec![])
        .await
        .expect("create thread");

    let activity = store
        .list_activity(
            ws.id,
            ActivityFilters {
                limit: 10,
                ..Default::default()
            },
        )
        .await
        .expect("list activity");
    assert!(!activity.is_empty());
    assert_eq!(activity[0].entity_type, "thread");
}

#[tokio::test]
async fn test_snapshot() {
    let (store, ws) = setup().await;

    store
        .create_thread(ws.id, "t1", "", vec![])
        .await
        .expect("create thread");

    store
        .save_memory(ws.id, "k1", "v1", None, "agent", vec![])
        .await
        .expect("save memory");

    let snap = store
        .create_snapshot(ws.id, "before-deploy")
        .await
        .expect("snapshot");
    assert_eq!(snap.label, "before-deploy");
    assert!(snap.summary.contains("1 threads"));
    assert!(snap.summary.contains("1 memories"));
}

#[tokio::test]
async fn test_workspace_overview() {
    let (store, ws) = setup().await;

    store
        .create_thread(ws.id, "t1", "", vec![])
        .await
        .expect("create thread");

    store
        .save_memory(ws.id, "k1", "v1", None, "agent", vec![])
        .await
        .expect("save memory");

    let overview = store.get_workspace_overview(ws.id).await.expect("overview");
    assert_eq!(overview.workspace.id, ws.id);
    assert_eq!(overview.active_threads.len(), 1);
    assert_eq!(overview.recent_memories.len(), 1);
}
