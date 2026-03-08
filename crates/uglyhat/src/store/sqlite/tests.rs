use chrono::Utc;
use uuid::Uuid;

use crate::error::Error;
use crate::model::*;
use crate::store::{ActivityFilters, BootstrapResult, HandoffFilters, TaskFilters};

use super::SqliteStore;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn setup() -> (SqliteStore, BootstrapResult) {
    let store = SqliteStore::open_memory()
        .await
        .expect("open_memory failed");
    let result = store
        .bootstrap_workspace_impl("Test Workspace", "Test description", "hash123", "key_")
        .await
        .expect("bootstrap failed");
    (store, result)
}

async fn create_test_task(store: &SqliteStore, epic_id: Uuid) -> Task {
    store
        .create_task_impl(
            epic_id,
            "Test Task",
            "Test description",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .expect("create_task failed")
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

#[tokio::test]
async fn workspace_bootstrap() {
    let (store, result) = setup().await;
    assert_eq!(result.workspace.name, "Test Workspace");
    let sys_epic = store
        .get_system_epic_id_impl(result.workspace.id)
        .await
        .unwrap();
    assert_eq!(sys_epic, result.epic_id);
}

#[tokio::test]
async fn workspace_create_get() {
    let (store, _) = setup().await;
    let ws = store
        .create_workspace_impl("My Project", "desc", None)
        .await
        .unwrap();
    assert_eq!(ws.name, "My Project");
    let got = store.get_workspace_impl(ws.id).await.unwrap();
    assert_eq!(got.id, ws.id);
    assert_eq!(got.name, "My Project");
}

#[tokio::test]
async fn workspace_list() {
    let (store, _) = setup().await;
    store.create_workspace_impl("WS-A", "", None).await.unwrap();
    store.create_workspace_impl("WS-B", "", None).await.unwrap();
    let list = store.list_workspaces_impl().await.unwrap();
    // Original bootstrap workspace + 2 new ones
    assert!(list.len() >= 3);
}

#[tokio::test]
async fn workspace_update() {
    let (store, result) = setup().await;
    let updated = store
        .update_workspace_impl(result.workspace.id, "Updated", "new desc", None)
        .await
        .unwrap();
    assert_eq!(updated.name, "Updated");
    assert_eq!(updated.description, "new desc");
}

#[tokio::test]
async fn workspace_delete() {
    let (store, _) = setup().await;
    let ws = store
        .create_workspace_impl("To Delete", "", None)
        .await
        .unwrap();
    store.delete_workspace_impl(ws.id).await.unwrap();
    let err = store.get_workspace_impl(ws.id).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn workspace_not_found() {
    let (store, _) = setup().await;
    let err = store.get_workspace_impl(Uuid::new_v4()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn workspace_get_system_epic_id() {
    let (store, result) = setup().await;
    let sys_epic_id = store
        .get_system_epic_id_impl(result.workspace.id)
        .await
        .unwrap();
    assert_eq!(sys_epic_id, result.epic_id);
}

// ---------------------------------------------------------------------------
// Initiative
// ---------------------------------------------------------------------------

#[tokio::test]
async fn initiative_create_get() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "Phase 1", "First phase", None)
        .await
        .unwrap();
    assert_eq!(init.name, "Phase 1");
    let got = store.get_initiative_impl(init.id).await.unwrap();
    assert_eq!(got.id, init.id);
    assert_eq!(got.workspace_id, result.workspace.id);
}

#[tokio::test]
async fn initiative_list() {
    let (store, result) = setup().await;
    store
        .create_initiative_impl(result.workspace.id, "Init-A", "", None)
        .await
        .unwrap();
    store
        .create_initiative_impl(result.workspace.id, "Init-B", "", None)
        .await
        .unwrap();
    let list = store
        .list_initiatives_by_workspace_impl(result.workspace.id)
        .await
        .unwrap();
    // System initiative + 2 new ones
    assert!(list.len() >= 3);
    let names: Vec<&str> = list.iter().map(|i| i.name.as_str()).collect();
    assert!(names.contains(&"Init-A"));
    assert!(names.contains(&"Init-B"));
}

#[tokio::test]
async fn initiative_update() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "Old Name", "", None)
        .await
        .unwrap();
    let updated = store
        .update_initiative_impl(init.id, "New Name", "New desc", "active", None)
        .await
        .unwrap();
    assert_eq!(updated.name, "New Name");
    assert_eq!(updated.description, "New desc");
}

#[tokio::test]
async fn initiative_delete() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "Del Init", "", None)
        .await
        .unwrap();
    store.delete_initiative_impl(init.id).await.unwrap();
    let err = store.get_initiative_impl(init.id).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn initiative_not_found() {
    let (store, _) = setup().await;
    let err = store.get_initiative_impl(Uuid::new_v4()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn initiative_cascade_on_workspace_delete() {
    let (store, _) = setup().await;
    let ws = store
        .create_workspace_impl("Temp WS", "", None)
        .await
        .unwrap();
    let init = store
        .create_initiative_impl(ws.id, "Cascade Init", "", None)
        .await
        .unwrap();
    store.delete_workspace_impl(ws.id).await.unwrap();
    let err = store.get_initiative_impl(init.id).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// ---------------------------------------------------------------------------
// Epic
// ---------------------------------------------------------------------------

#[tokio::test]
async fn epic_create_get() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "Epic Init", "", None)
        .await
        .unwrap();
    let epic = store
        .create_epic_impl(init.id, "Sprint 1", "First sprint", None)
        .await
        .unwrap();
    assert_eq!(epic.name, "Sprint 1");
    let got = store.get_epic_impl(epic.id).await.unwrap();
    assert_eq!(got.id, epic.id);
    assert_eq!(got.initiative_id, init.id);
    assert_eq!(got.workspace_id, result.workspace.id);
}

#[tokio::test]
async fn epic_list() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "List Init", "", None)
        .await
        .unwrap();
    store
        .create_epic_impl(init.id, "Epic-A", "", None)
        .await
        .unwrap();
    store
        .create_epic_impl(init.id, "Epic-B", "", None)
        .await
        .unwrap();
    let list = store.list_epics_by_initiative_impl(init.id).await.unwrap();
    assert_eq!(list.len(), 2);
}

#[tokio::test]
async fn epic_update() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "Upd Init", "", None)
        .await
        .unwrap();
    let epic = store
        .create_epic_impl(init.id, "Old Epic", "", None)
        .await
        .unwrap();
    let updated = store
        .update_epic_impl(epic.id, "New Epic", "updated", "active", None)
        .await
        .unwrap();
    assert_eq!(updated.name, "New Epic");
}

#[tokio::test]
async fn epic_delete() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "Del Init", "", None)
        .await
        .unwrap();
    let epic = store
        .create_epic_impl(init.id, "Del Epic", "", None)
        .await
        .unwrap();
    store.delete_epic_impl(epic.id).await.unwrap();
    let err = store.get_epic_impl(epic.id).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn epic_not_found() {
    let (store, _) = setup().await;
    let err = store.get_epic_impl(Uuid::new_v4()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn epic_requires_valid_initiative() {
    let (store, _) = setup().await;
    let err = store
        .create_epic_impl(Uuid::new_v4(), "Orphan", "", None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

#[tokio::test]
async fn task_create_get() {
    let (store, result) = setup().await;
    let task = store
        .create_task_impl(
            result.epic_id,
            "My Task",
            "desc",
            TaskStatus::Todo,
            TaskPriority::High,
            "alice",
            vec!["backend".to_string()],
            None,
        )
        .await
        .unwrap();
    assert_eq!(task.name, "My Task");
    assert_eq!(task.assignee, "alice");
    assert_eq!(task.domain_tags, vec!["backend"]);
    let got = store.get_task_impl(task.id).await.unwrap();
    assert_eq!(got.id, task.id);
    assert_eq!(got.epic_id, result.epic_id);
}

#[tokio::test]
async fn task_update() {
    let (store, result) = setup().await;
    let task = create_test_task(&store, result.epic_id).await;
    let updated = store
        .update_task_impl(
            task.id,
            "Updated Task",
            "new desc",
            TaskStatus::InProgress,
            TaskPriority::High,
            "bob",
            vec!["frontend".to_string()],
            None,
        )
        .await
        .unwrap();
    assert_eq!(updated.name, "Updated Task");
    assert_eq!(updated.status, TaskStatus::InProgress);
    assert_eq!(updated.assignee, "bob");
}

#[tokio::test]
async fn task_delete() {
    let (store, result) = setup().await;
    let task = create_test_task(&store, result.epic_id).await;
    store.delete_task_impl(task.id).await.unwrap();
    let err = store.get_task_impl(task.id).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn task_not_found() {
    let (store, _) = setup().await;
    let err = store.get_task_impl(Uuid::new_v4()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn task_list_by_epic() {
    let (store, result) = setup().await;
    create_test_task(&store, result.epic_id).await;
    create_test_task(&store, result.epic_id).await;
    let tasks = store.list_tasks_by_epic_impl(result.epic_id).await.unwrap();
    assert_eq!(tasks.len(), 2);
}

#[tokio::test]
async fn task_filter_by_status() {
    let (store, result) = setup().await;
    store
        .create_task_impl(
            result.epic_id,
            "T1",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    store
        .create_task_impl(
            result.epic_id,
            "T2",
            "",
            TaskStatus::Done,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    let filters = TaskFilters {
        status: Some(TaskStatus::Todo),
        ..Default::default()
    };
    let tasks = store
        .list_tasks_by_workspace_impl(result.workspace.id, filters)
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name, "T1");
}

#[tokio::test]
async fn task_filter_by_priority() {
    let (store, result) = setup().await;
    store
        .create_task_impl(
            result.epic_id,
            "High Task",
            "",
            TaskStatus::Todo,
            TaskPriority::High,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    store
        .create_task_impl(
            result.epic_id,
            "Low Task",
            "",
            TaskStatus::Todo,
            TaskPriority::Low,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    let filters = TaskFilters {
        priority: Some(TaskPriority::High),
        ..Default::default()
    };
    let tasks = store
        .list_tasks_by_workspace_impl(result.workspace.id, filters)
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name, "High Task");
}

#[tokio::test]
async fn task_filter_by_assignee() {
    let (store, result) = setup().await;
    store
        .create_task_impl(
            result.epic_id,
            "Alice Task",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "alice",
            vec![],
            None,
        )
        .await
        .unwrap();
    store
        .create_task_impl(
            result.epic_id,
            "Bob Task",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "bob",
            vec![],
            None,
        )
        .await
        .unwrap();
    let filters = TaskFilters {
        assignee: Some("alice".to_string()),
        ..Default::default()
    };
    let tasks = store
        .list_tasks_by_workspace_impl(result.workspace.id, filters)
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name, "Alice Task");
}

#[tokio::test]
async fn task_filter_unassigned() {
    let (store, result) = setup().await;
    store
        .create_task_impl(
            result.epic_id,
            "Unassigned",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    store
        .create_task_impl(
            result.epic_id,
            "Assigned",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "charlie",
            vec![],
            None,
        )
        .await
        .unwrap();
    let filters = TaskFilters {
        unassigned: Some(true),
        ..Default::default()
    };
    let tasks = store
        .list_tasks_by_workspace_impl(result.workspace.id, filters)
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name, "Unassigned");
}

#[tokio::test]
async fn task_filter_by_domain() {
    let (store, result) = setup().await;
    store
        .create_task_impl(
            result.epic_id,
            "Backend Task",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec!["backend".to_string()],
            None,
        )
        .await
        .unwrap();
    store
        .create_task_impl(
            result.epic_id,
            "Frontend Task",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec!["frontend".to_string()],
            None,
        )
        .await
        .unwrap();
    let filters = TaskFilters {
        domain: Some("backend".to_string()),
        ..Default::default()
    };
    let tasks = store
        .list_tasks_by_workspace_impl(result.workspace.id, filters)
        .await
        .unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].name, "Backend Task");
}

// ---------------------------------------------------------------------------
// Auto-close propagation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn autoclose_epic_when_all_tasks_done() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "AC Init", "", None)
        .await
        .unwrap();
    let epic = store
        .create_epic_impl(init.id, "AC Epic", "", None)
        .await
        .unwrap();
    let task = store
        .create_task_impl(
            epic.id,
            "Only Task",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    store
        .update_task_impl(
            task.id,
            "Only Task",
            "",
            TaskStatus::Done,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    let epic_after = store.get_epic_impl(epic.id).await.unwrap();
    assert_eq!(epic_after.status, "done");
}

#[tokio::test]
async fn autoclose_initiative_when_all_epics_done() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "AC2 Init", "", None)
        .await
        .unwrap();
    let epic = store
        .create_epic_impl(init.id, "AC2 Epic", "", None)
        .await
        .unwrap();
    let task = store
        .create_task_impl(
            epic.id,
            "Single Task",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    store
        .update_task_impl(
            task.id,
            "Single Task",
            "",
            TaskStatus::Done,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    let init_after = store.get_initiative_impl(init.id).await.unwrap();
    assert_eq!(init_after.status, "done");
}

#[tokio::test]
async fn autoclose_does_not_close_epic_with_remaining_tasks() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "AC3 Init", "", None)
        .await
        .unwrap();
    let epic = store
        .create_epic_impl(init.id, "AC3 Epic", "", None)
        .await
        .unwrap();
    let t1 = store
        .create_task_impl(
            epic.id,
            "Task 1",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    store
        .create_task_impl(
            epic.id,
            "Task 2",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    store
        .update_task_impl(
            t1.id,
            "Task 1",
            "",
            TaskStatus::Done,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    let epic_after = store.get_epic_impl(epic.id).await.unwrap();
    assert_ne!(epic_after.status, "done");
}

#[tokio::test]
async fn autoclose_cancelled_is_terminal() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "AC4 Init", "", None)
        .await
        .unwrap();
    let epic = store
        .create_epic_impl(init.id, "AC4 Epic", "", None)
        .await
        .unwrap();
    let task = store
        .create_task_impl(
            epic.id,
            "Cancelled Task",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    store
        .update_task_impl(
            task.id,
            "Cancelled Task",
            "",
            TaskStatus::Cancelled,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    let epic_after = store.get_epic_impl(epic.id).await.unwrap();
    assert_eq!(epic_after.status, "done");
}

// ---------------------------------------------------------------------------
// Dependencies
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dep_add_and_get() {
    let (store, result) = setup().await;
    let t1 = create_test_task(&store, result.epic_id).await;
    let t2 = create_test_task(&store, result.epic_id).await;
    let dep = store.add_dependency_impl(t1.id, t2.id).await.unwrap();
    assert_eq!(dep.blocking_task_id, t1.id);
    assert_eq!(dep.blocked_task_id, t2.id);
    let (blocks, blocked_by) = store.get_dependencies_impl(t1.id).await.unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].task_id, t2.id);
    assert_eq!(blocked_by.len(), 0);
}

#[tokio::test]
async fn dep_remove() {
    let (store, result) = setup().await;
    let t1 = create_test_task(&store, result.epic_id).await;
    let t2 = create_test_task(&store, result.epic_id).await;
    let dep = store.add_dependency_impl(t1.id, t2.id).await.unwrap();
    store.remove_dependency_impl(dep.id).await.unwrap();
    let (blocks, _) = store.get_dependencies_impl(t1.id).await.unwrap();
    assert_eq!(blocks.len(), 0);
}

#[tokio::test]
async fn dep_remove_not_found() {
    let (store, _) = setup().await;
    let err = store
        .remove_dependency_impl(Uuid::new_v4())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn dep_self_rejected() {
    let (store, result) = setup().await;
    let t1 = create_test_task(&store, result.epic_id).await;
    // SQLite CHECK constraint (blocking_task_id != blocked_task_id) rejects self-deps
    let err = store.add_dependency_impl(t1.id, t1.id).await.unwrap_err();
    assert!(matches!(err, Error::Sqlx(_) | Error::Conflict(_)));
}

#[tokio::test]
async fn dep_direct_cycle_rejected() {
    let (store, result) = setup().await;
    let t1 = create_test_task(&store, result.epic_id).await;
    let t2 = create_test_task(&store, result.epic_id).await;
    store.add_dependency_impl(t1.id, t2.id).await.unwrap();
    // t2 → t1 would close the cycle
    let err = store.add_dependency_impl(t2.id, t1.id).await.unwrap_err();
    assert!(matches!(err, Error::Conflict(_)));
}

#[tokio::test]
async fn dep_indirect_cycle_rejected() {
    let (store, result) = setup().await;
    let t1 = create_test_task(&store, result.epic_id).await;
    let t2 = create_test_task(&store, result.epic_id).await;
    let t3 = create_test_task(&store, result.epic_id).await;
    store.add_dependency_impl(t1.id, t2.id).await.unwrap();
    store.add_dependency_impl(t2.id, t3.id).await.unwrap();
    // t3 → t1 would create a cycle: t1→t2→t3→t1
    let err = store.add_dependency_impl(t3.id, t1.id).await.unwrap_err();
    assert!(matches!(err, Error::Conflict(_)));
}

#[tokio::test]
async fn dep_blocked_by() {
    let (store, result) = setup().await;
    let t1 = create_test_task(&store, result.epic_id).await;
    let t2 = create_test_task(&store, result.epic_id).await;
    store.add_dependency_impl(t1.id, t2.id).await.unwrap();
    let (_, blocked_by) = store.get_dependencies_impl(t2.id).await.unwrap();
    assert_eq!(blocked_by.len(), 1);
    assert_eq!(blocked_by[0].task_id, t1.id);
}

// ---------------------------------------------------------------------------
// Activity
// ---------------------------------------------------------------------------

#[tokio::test]
async fn activity_log_and_list() {
    let (store, result) = setup().await;
    let entity_id = Uuid::new_v4();
    store
        .log_activity_impl(
            result.workspace.id,
            "test-agent",
            "created",
            "task",
            entity_id,
            "Created something",
            None,
        )
        .await
        .unwrap();
    let entries = store
        .list_activity_impl(
            result.workspace.id,
            ActivityFilters {
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!entries.is_empty());
}

#[tokio::test]
async fn activity_create_returns_entry() {
    let (store, result) = setup().await;
    let entity_id = Uuid::new_v4();
    let entry = store
        .create_activity_impl(
            result.workspace.id,
            "agent1",
            "updated",
            "epic",
            entity_id,
            "Updated the epic",
            Some(serde_json::json!({"key": "value"})),
        )
        .await
        .unwrap();
    assert_eq!(entry.actor, "agent1");
    assert_eq!(entry.action, "updated");
    assert_eq!(entry.entity_type, "epic");
    assert!(entry.detail.is_some());
}

#[tokio::test]
async fn activity_filter_by_actor() {
    let (store, result) = setup().await;
    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();
    store
        .log_activity_impl(
            result.workspace.id,
            "alice",
            "created",
            "task",
            id1,
            "",
            None,
        )
        .await
        .unwrap();
    store
        .log_activity_impl(result.workspace.id, "bob", "created", "task", id2, "", None)
        .await
        .unwrap();
    let entries = store
        .list_activity_impl(
            result.workspace.id,
            ActivityFilters {
                actor: Some("alice".to_string()),
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!entries.is_empty());
    assert!(entries.iter().all(|e| e.actor == "alice"));
}

#[tokio::test]
async fn activity_filter_by_entity_type() {
    let (store, result) = setup().await;
    let id1 = Uuid::new_v4();
    let id2 = Uuid::new_v4();
    store
        .log_activity_impl(result.workspace.id, "", "created", "task", id1, "", None)
        .await
        .unwrap();
    store
        .log_activity_impl(result.workspace.id, "", "created", "epic", id2, "", None)
        .await
        .unwrap();
    let entries = store
        .list_activity_impl(
            result.workspace.id,
            ActivityFilters {
                entity_type: Some("task".to_string()),
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(entries.iter().all(|e| e.entity_type == "task"));
}

#[tokio::test]
async fn activity_filter_since() {
    let (store, result) = setup().await;
    let before = Utc::now();
    let entity_id = Uuid::new_v4();
    store
        .log_activity_impl(
            result.workspace.id,
            "",
            "updated",
            "task",
            entity_id,
            "",
            None,
        )
        .await
        .unwrap();
    let entries = store
        .list_activity_impl(
            result.workspace.id,
            ActivityFilters {
                since: Some(before),
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!entries.is_empty());
}

#[tokio::test]
async fn activity_list_since() {
    let (store, result) = setup().await;
    let since = Utc::now();
    let entity_id = Uuid::new_v4();
    store
        .log_activity_impl(
            result.workspace.id,
            "",
            "created",
            "task",
            entity_id,
            "",
            None,
        )
        .await
        .unwrap();
    let entries = store
        .list_activity_since_impl(result.workspace.id, since, 10)
        .await
        .unwrap();
    assert!(!entries.is_empty());
}

#[tokio::test]
async fn activity_auto_logged_on_task_create() {
    let (store, result) = setup().await;
    create_test_task(&store, result.epic_id).await;
    let entries = store
        .list_activity_impl(
            result.workspace.id,
            ActivityFilters {
                entity_type: Some("task".to_string()),
                limit: 50,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(!entries.is_empty());
    assert!(entries.iter().any(|e| e.action == "created"));
}

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

#[tokio::test]
async fn decision_create_get() {
    let (store, result) = setup().await;
    let d = store
        .create_decision_impl(
            Some(result.workspace.id),
            None,
            None,
            "Use Rust",
            "We decided to use Rust",
            None,
        )
        .await
        .unwrap();
    assert_eq!(d.title, "Use Rust");
    assert_eq!(d.status, "proposed");
    let got = store.get_decision_impl(d.id).await.unwrap();
    assert_eq!(got.id, d.id);
}

#[tokio::test]
async fn decision_list_by_workspace() {
    let (store, result) = setup().await;
    store
        .create_decision_impl(Some(result.workspace.id), None, None, "D1", "", None)
        .await
        .unwrap();
    store
        .create_decision_impl(Some(result.workspace.id), None, None, "D2", "", None)
        .await
        .unwrap();
    let list = store
        .list_decisions_by_workspace_impl(result.workspace.id)
        .await
        .unwrap();
    assert!(list.len() >= 2);
}

#[tokio::test]
async fn decision_update() {
    let (store, result) = setup().await;
    let d = store
        .create_decision_impl(Some(result.workspace.id), None, None, "Old Title", "", None)
        .await
        .unwrap();
    let updated = store
        .update_decision_impl(d.id, "New Title", "new content", "accepted", None)
        .await
        .unwrap();
    assert_eq!(updated.title, "New Title");
    assert_eq!(updated.status, "accepted");
}

#[tokio::test]
async fn decision_delete() {
    let (store, result) = setup().await;
    let d = store
        .create_decision_impl(Some(result.workspace.id), None, None, "To Delete", "", None)
        .await
        .unwrap();
    store.delete_decision_impl(d.id).await.unwrap();
    let err = store.get_decision_impl(d.id).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn decision_not_found() {
    let (store, _) = setup().await;
    let err = store.get_decision_impl(Uuid::new_v4()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn decision_scoped_to_epic() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "Scope Init", "", None)
        .await
        .unwrap();
    let epic = store
        .create_epic_impl(init.id, "Scope Epic", "", None)
        .await
        .unwrap();
    let d = store
        .create_decision_impl(None, None, Some(epic.id), "Epic Decision", "", None)
        .await
        .unwrap();
    assert_eq!(d.epic_id, Some(epic.id));
    assert_eq!(d.epic_name, "Scope Epic");
}

// ---------------------------------------------------------------------------
// Note
// ---------------------------------------------------------------------------

#[tokio::test]
async fn note_create_get() {
    let (store, result) = setup().await;
    let n = store
        .create_note_impl(
            Some(result.workspace.id),
            None,
            None,
            None,
            None,
            "My Note",
            "Content here",
            None,
        )
        .await
        .unwrap();
    assert_eq!(n.title, "My Note");
    assert_eq!(n.workspace_id, Some(result.workspace.id));
    let got = store.get_note_impl(n.id).await.unwrap();
    assert_eq!(got.id, n.id);
}

#[tokio::test]
async fn note_update() {
    let (store, result) = setup().await;
    let n = store
        .create_note_impl(
            Some(result.workspace.id),
            None,
            None,
            None,
            None,
            "Old Note",
            "Old content",
            None,
        )
        .await
        .unwrap();
    let updated = store
        .update_note_impl(n.id, "New Note", "New content", None)
        .await
        .unwrap();
    assert_eq!(updated.title, "New Note");
    assert_eq!(updated.content, "New content");
}

#[tokio::test]
async fn note_delete() {
    let (store, result) = setup().await;
    let n = store
        .create_note_impl(
            Some(result.workspace.id),
            None,
            None,
            None,
            None,
            "Del Note",
            "",
            None,
        )
        .await
        .unwrap();
    store.delete_note_impl(n.id).await.unwrap();
    let err = store.get_note_impl(n.id).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn note_list_by_parent_workspace() {
    let (store, result) = setup().await;
    store
        .create_note_impl(
            Some(result.workspace.id),
            None,
            None,
            None,
            None,
            "Note A",
            "",
            None,
        )
        .await
        .unwrap();
    store
        .create_note_impl(
            Some(result.workspace.id),
            None,
            None,
            None,
            None,
            "Note B",
            "",
            None,
        )
        .await
        .unwrap();
    let notes = store
        .list_notes_by_parent_impl("workspace", result.workspace.id)
        .await
        .unwrap();
    assert_eq!(notes.len(), 2);
}

#[tokio::test]
async fn note_list_by_parent_task() {
    let (store, result) = setup().await;
    let task = create_test_task(&store, result.epic_id).await;
    store
        .create_note_impl(None, None, None, Some(task.id), None, "Task Note", "", None)
        .await
        .unwrap();
    let notes = store
        .list_notes_by_parent_impl("task", task.id)
        .await
        .unwrap();
    assert_eq!(notes.len(), 1);
}

#[tokio::test]
async fn note_invalid_parent_type() {
    let (store, result) = setup().await;
    let err = store
        .list_notes_by_parent_impl("invalid_type", result.workspace.id)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::BadRequest(_)));
}

#[tokio::test]
async fn note_not_found() {
    let (store, _) = setup().await;
    let err = store.get_note_impl(Uuid::new_v4()).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_checkin_new() {
    let (store, result) = setup().await;
    let resp = store
        .checkin_agent_impl(
            result.workspace.id,
            "agent-1",
            vec!["rust".to_string(), "sql".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(resp.agent.name, "agent-1");
    assert_eq!(resp.agent.capabilities, vec!["rust", "sql"]);
    assert!(resp.session.ended_at.is_none());
}

#[tokio::test]
async fn agent_checkin_upsert() {
    let (store, result) = setup().await;
    store
        .checkin_agent_impl(result.workspace.id, "agent-x", vec!["python".to_string()])
        .await
        .unwrap();
    // Second checkin should update capabilities
    let resp = store
        .checkin_agent_impl(result.workspace.id, "agent-x", vec!["rust".to_string()])
        .await
        .unwrap();
    assert_eq!(resp.agent.capabilities, vec!["rust"]);
}

#[tokio::test]
async fn agent_checkin_returns_assigned_tasks() {
    let (store, result) = setup().await;
    store
        .create_task_impl(
            result.epic_id,
            "My Task",
            "",
            TaskStatus::InProgress,
            TaskPriority::Medium,
            "worker",
            vec![],
            None,
        )
        .await
        .unwrap();
    let resp = store
        .checkin_agent_impl(result.workspace.id, "worker", vec![])
        .await
        .unwrap();
    assert_eq!(resp.assigned_tasks.len(), 1);
    assert_eq!(resp.assigned_tasks[0].name, "My Task");
}

#[tokio::test]
async fn agent_checkout() {
    let (store, result) = setup().await;
    store
        .checkin_agent_impl(result.workspace.id, "agent-2", vec![])
        .await
        .unwrap();
    let resp = store
        .checkout_agent_impl(result.workspace.id, "agent-2", "Work complete", false)
        .await
        .unwrap();
    assert!(resp.session.ended_at.is_some());
    assert_eq!(resp.session.summary, "Work complete");
}

#[tokio::test]
async fn agent_checkout_no_open_session() {
    let (store, result) = setup().await;
    let err = store
        .checkout_agent_impl(result.workspace.id, "nonexistent", "", false)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn agent_list() {
    let (store, result) = setup().await;
    store
        .checkin_agent_impl(result.workspace.id, "ag-1", vec![])
        .await
        .unwrap();
    store
        .checkin_agent_impl(result.workspace.id, "ag-2", vec![])
        .await
        .unwrap();
    let agents = store.list_agents_impl(result.workspace.id).await.unwrap();
    assert_eq!(agents.len(), 2);
}

// ---------------------------------------------------------------------------
// Handoff
// ---------------------------------------------------------------------------

#[tokio::test]
async fn handoff_create_get_by_task() {
    let (store, result) = setup().await;
    let task = create_test_task(&store, result.epic_id).await;
    let h = store
        .create_handoff_impl(
            task.id,
            "agent-a",
            "Completed analysis",
            vec!["finding1".to_string()],
            vec![],
            vec!["next1".to_string()],
            None,
        )
        .await
        .unwrap();
    assert_eq!(h.task_id, task.id);
    assert_eq!(h.agent_name, "agent-a");
    assert_eq!(h.findings, vec!["finding1"]);
    let handoffs = store.get_handoffs_by_task_impl(task.id).await.unwrap();
    assert_eq!(handoffs.len(), 1);
    assert_eq!(handoffs[0].id, h.id);
}

#[tokio::test]
async fn handoff_list_by_workspace() {
    let (store, result) = setup().await;
    let task = create_test_task(&store, result.epic_id).await;
    store
        .create_handoff_impl(task.id, "ag1", "s1", vec![], vec![], vec![], None)
        .await
        .unwrap();
    store
        .create_handoff_impl(task.id, "ag2", "s2", vec![], vec![], vec![], None)
        .await
        .unwrap();
    let handoffs = store
        .list_handoffs_by_workspace_impl(result.workspace.id, HandoffFilters::default())
        .await
        .unwrap();
    assert_eq!(handoffs.len(), 2);
}

#[tokio::test]
async fn handoff_list_by_epic() {
    let (store, result) = setup().await;
    let task = create_test_task(&store, result.epic_id).await;
    store
        .create_handoff_impl(task.id, "ag1", "s1", vec![], vec![], vec![], None)
        .await
        .unwrap();
    let handoffs = store
        .list_handoffs_by_epic_impl(result.epic_id)
        .await
        .unwrap();
    assert_eq!(handoffs.len(), 1);
}

#[tokio::test]
async fn handoff_filter_by_agent() {
    let (store, result) = setup().await;
    let task = create_test_task(&store, result.epic_id).await;
    store
        .create_handoff_impl(task.id, "alice", "s", vec![], vec![], vec![], None)
        .await
        .unwrap();
    store
        .create_handoff_impl(task.id, "bob", "s", vec![], vec![], vec![], None)
        .await
        .unwrap();
    let handoffs = store
        .list_handoffs_by_workspace_impl(
            result.workspace.id,
            HandoffFilters {
                agent: Some("alice".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(handoffs.len(), 1);
    assert_eq!(handoffs[0].agent_name, "alice");
}

#[tokio::test]
async fn handoff_empty_for_nonexistent_task() {
    let (store, _) = setup().await;
    let handoffs = store
        .get_handoffs_by_task_impl(Uuid::new_v4())
        .await
        .unwrap();
    assert!(handoffs.is_empty());
}

// ---------------------------------------------------------------------------
// API Key
// ---------------------------------------------------------------------------

#[tokio::test]
async fn apikey_bootstrap_default() {
    let (_, result) = setup().await;
    assert_eq!(result.api_key.name, "default");
    assert_eq!(result.api_key.key_hash, "hash123");
    assert_eq!(result.api_key.workspace_id, result.workspace.id);
}

#[tokio::test]
async fn apikey_create_get_by_hash() {
    let (store, result) = setup().await;
    let key = store
        .create_api_key_impl(result.workspace.id, "my-key", "newhash", "nk_")
        .await
        .unwrap();
    assert_eq!(key.name, "my-key");
    let got = store.get_api_key_by_hash_impl("newhash").await.unwrap();
    assert_eq!(got.id, key.id);
    assert_eq!(got.workspace_id, result.workspace.id);
}

#[tokio::test]
async fn apikey_list() {
    let (store, result) = setup().await;
    store
        .create_api_key_impl(result.workspace.id, "key1", "h1", "k1_")
        .await
        .unwrap();
    store
        .create_api_key_impl(result.workspace.id, "key2", "h2", "k2_")
        .await
        .unwrap();
    let keys = store
        .list_api_keys_by_workspace_impl(result.workspace.id)
        .await
        .unwrap();
    // Default key from bootstrap + 2 new ones
    assert_eq!(keys.len(), 3);
}

#[tokio::test]
async fn apikey_delete() {
    let (store, result) = setup().await;
    let key = store
        .create_api_key_impl(result.workspace.id, "del-key", "delhash", "dk_")
        .await
        .unwrap();
    store.delete_api_key_impl(key.id).await.unwrap();
    let err = store.get_api_key_by_hash_impl("delhash").await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

#[tokio::test]
async fn apikey_not_found() {
    let (store, _) = setup().await;
    let err = store
        .get_api_key_by_hash_impl("nonexistent")
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// ---------------------------------------------------------------------------
// Workspace Context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_workspace_empty() {
    let (store, result) = setup().await;
    let ctx = store
        .get_workspace_context_impl(result.workspace.id)
        .await
        .unwrap();
    assert_eq!(ctx.workspace.id, result.workspace.id);
    assert!(ctx.active_tasks.is_empty());
}

#[tokio::test]
async fn context_workspace_with_data() {
    let (store, result) = setup().await;
    store
        .create_task_impl(
            result.epic_id,
            "In-Progress Task",
            "",
            TaskStatus::InProgress,
            TaskPriority::High,
            "alice",
            vec![],
            None,
        )
        .await
        .unwrap();
    let ctx = store
        .get_workspace_context_impl(result.workspace.id)
        .await
        .unwrap();
    assert!(!ctx.active_tasks.is_empty());
    assert!(!ctx.tasks_by_status.is_empty());
}

#[tokio::test]
async fn context_next_tasks_ordering() {
    let (store, result) = setup().await;
    store
        .create_task_impl(
            result.epic_id,
            "Low Priority",
            "",
            TaskStatus::Todo,
            TaskPriority::Low,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    store
        .create_task_impl(
            result.epic_id,
            "High Priority",
            "",
            TaskStatus::Todo,
            TaskPriority::High,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    let tasks = store
        .get_next_tasks_impl(result.workspace.id, 10)
        .await
        .unwrap();
    assert_eq!(tasks.len(), 2);
    assert_eq!(tasks[0].name, "High Priority");
    assert_eq!(tasks[1].name, "Low Priority");
}

#[tokio::test]
async fn context_next_tasks_excludes_blocked() {
    let (store, result) = setup().await;
    let t1 = store
        .create_task_impl(
            result.epic_id,
            "Blocker",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    let t2 = store
        .create_task_impl(
            result.epic_id,
            "Blocked",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    store.add_dependency_impl(t1.id, t2.id).await.unwrap();
    let tasks = store
        .get_next_tasks_impl(result.workspace.id, 10)
        .await
        .unwrap();
    let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
    assert!(!names.contains(&"Blocked"));
    assert!(names.contains(&"Blocker"));
}

#[tokio::test]
async fn context_next_tasks_excludes_assigned() {
    let (store, result) = setup().await;
    store
        .create_task_impl(
            result.epic_id,
            "Assigned Task",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "someone",
            vec![],
            None,
        )
        .await
        .unwrap();
    store
        .create_task_impl(
            result.epic_id,
            "Free Task",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    let tasks = store
        .get_next_tasks_impl(result.workspace.id, 10)
        .await
        .unwrap();
    let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
    assert!(!names.contains(&"Assigned Task"));
    assert!(names.contains(&"Free Task"));
}

// ---------------------------------------------------------------------------
// Task Context
// ---------------------------------------------------------------------------

#[tokio::test]
async fn task_context_full() {
    let (store, result) = setup().await;
    let task = create_test_task(&store, result.epic_id).await;
    store
        .create_note_impl(None, None, None, Some(task.id), None, "Task Note", "", None)
        .await
        .unwrap();
    let ctx = store.get_task_context_impl(task.id).await.unwrap();
    assert_eq!(ctx.task.id, task.id);
    assert!(ctx.epic.is_some());
    assert!(ctx.initiative.is_some());
    assert_eq!(ctx.notes.len(), 1);
}

#[tokio::test]
async fn task_context_minimal() {
    let (store, result) = setup().await;
    let task = create_test_task(&store, result.epic_id).await;
    let ctx = store.get_task_context_impl(task.id).await.unwrap();
    assert_eq!(ctx.task.id, task.id);
    assert!(ctx.blocks.is_empty());
    assert!(ctx.blocked_by.is_empty());
    assert!(ctx.handoffs.is_empty());
}

#[tokio::test]
async fn task_context_not_found() {
    let (store, _) = setup().await;
    let err = store
        .get_task_context_impl(Uuid::new_v4())
        .await
        .unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
}

// ---------------------------------------------------------------------------
// Cascade Deletes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cascade_workspace_deletes_all() {
    let (store, _) = setup().await;
    let ws = store
        .create_workspace_impl("Cascade WS", "", None)
        .await
        .unwrap();
    let init = store
        .create_initiative_impl(ws.id, "Init", "", None)
        .await
        .unwrap();
    let epic = store
        .create_epic_impl(init.id, "Epic", "", None)
        .await
        .unwrap();
    let task = store
        .create_task_impl(
            epic.id,
            "Task",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    store.delete_workspace_impl(ws.id).await.unwrap();
    assert!(matches!(
        store.get_initiative_impl(init.id).await.unwrap_err(),
        Error::NotFound(_)
    ));
    assert!(matches!(
        store.get_epic_impl(epic.id).await.unwrap_err(),
        Error::NotFound(_)
    ));
    assert!(matches!(
        store.get_task_impl(task.id).await.unwrap_err(),
        Error::NotFound(_)
    ));
}

#[tokio::test]
async fn cascade_initiative_deletes_epics_and_tasks() {
    let (store, result) = setup().await;
    let init = store
        .create_initiative_impl(result.workspace.id, "Del Init", "", None)
        .await
        .unwrap();
    let epic = store
        .create_epic_impl(init.id, "Del Epic", "", None)
        .await
        .unwrap();
    let task = store
        .create_task_impl(
            epic.id,
            "Del Task",
            "",
            TaskStatus::Todo,
            TaskPriority::Medium,
            "",
            vec![],
            None,
        )
        .await
        .unwrap();
    store.delete_initiative_impl(init.id).await.unwrap();
    assert!(matches!(
        store.get_epic_impl(epic.id).await.unwrap_err(),
        Error::NotFound(_)
    ));
    assert!(matches!(
        store.get_task_impl(task.id).await.unwrap_err(),
        Error::NotFound(_)
    ));
}

#[tokio::test]
async fn cascade_task_deletes_deps_and_handoffs() {
    let (store, result) = setup().await;
    let t1 = create_test_task(&store, result.epic_id).await;
    let t2 = create_test_task(&store, result.epic_id).await;
    let dep = store.add_dependency_impl(t1.id, t2.id).await.unwrap();
    store
        .create_handoff_impl(t1.id, "ag", "s", vec![], vec![], vec![], None)
        .await
        .unwrap();
    store.delete_task_impl(t1.id).await.unwrap();
    // Dependency was cascade-deleted
    let err = store.remove_dependency_impl(dep.id).await.unwrap_err();
    assert!(matches!(err, Error::NotFound(_)));
    // Handoffs for deleted task return empty
    let handoffs = store.get_handoffs_by_task_impl(t1.id).await.unwrap();
    assert!(handoffs.is_empty());
}
