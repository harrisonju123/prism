use std::sync::Arc;

use axum::routing::{delete, get, post};
use axum::{Router, middleware};

use crate::api::*;
use crate::middleware::auth::auth_middleware;

pub fn build_router(state: Arc<AppState>) -> Router {
    let public = Router::new()
        .route("/health", get(health::health))
        .route("/workspaces", post(workspace::create_workspace));

    let protected = Router::new()
        // Workspaces
        .route("/workspaces", get(workspace::list_workspaces))
        .route(
            "/workspaces/{workspaceId}",
            get(workspace::get_workspace)
                .put(workspace::update_workspace)
                .delete(workspace::delete_workspace),
        )
        // Workspace context
        .route(
            "/workspaces/{workspaceId}/context",
            get(context::get_workspace_context),
        )
        .route(
            "/workspaces/{workspaceId}/next",
            get(context::get_next_tasks),
        )
        // Workspace tasks
        .route(
            "/workspaces/{workspaceId}/tasks",
            get(task::list_tasks_by_workspace),
        )
        // Agent issue reporting
        .route(
            "/workspaces/{workspaceId}/issues",
            post(workspace::report_issue),
        )
        // Initiatives
        .route(
            "/workspaces/{workspaceId}/initiatives",
            get(initiative::list_initiatives).post(initiative::create_initiative),
        )
        .route(
            "/initiatives/{id}",
            get(initiative::get_initiative)
                .put(initiative::update_initiative)
                .delete(initiative::delete_initiative),
        )
        // Epics
        .route(
            "/initiatives/{initiativeId}/epics",
            get(epic::list_epics).post(epic::create_epic),
        )
        .route(
            "/epics/{id}",
            get(epic::get_epic)
                .put(epic::update_epic)
                .delete(epic::delete_epic),
        )
        // Tasks
        .route(
            "/epics/{epicId}/tasks",
            get(task::list_tasks_by_epic).post(task::create_task),
        )
        .route(
            "/tasks/{id}",
            get(task::get_task)
                .put(task::update_task)
                .delete(task::delete_task),
        )
        // Task dependencies
        .route(
            "/tasks/{id}/dependencies",
            get(dependency::get_dependencies).post(dependency::add_dependency),
        )
        .route(
            "/tasks/{id}/dependencies/{depId}",
            delete(dependency::remove_dependency),
        )
        // Task context
        .route("/tasks/{id}/context", get(task::get_task_context))
        // Task handoffs
        .route("/tasks/{id}/handoffs", get(handoff::get_handoffs_by_task))
        // Handoffs (workspace level)
        .route(
            "/workspaces/{workspaceId}/handoffs",
            get(handoff::list_handoffs).post(handoff::create_handoff),
        )
        // Decisions
        .route(
            "/workspaces/{workspaceId}/decisions",
            get(decision::list_decisions),
        )
        .route("/decisions", post(decision::create_decision))
        .route(
            "/decisions/{id}",
            get(decision::get_decision)
                .put(decision::update_decision)
                .delete(decision::delete_decision),
        )
        // Notes
        .route("/notes", post(note::create_note))
        .route(
            "/notes/{id}",
            get(note::get_note)
                .put(note::update_note)
                .delete(note::delete_note),
        )
        .route(
            "/notes/by/{parentType}/{parentId}",
            get(note::list_notes_by_parent),
        )
        // Activity log
        .route(
            "/workspaces/{workspaceId}/activity",
            get(activity::list_activity).post(activity::create_activity),
        )
        // Agents
        .route(
            "/workspaces/{workspaceId}/agents/checkin",
            post(agent::checkin),
        )
        .route(
            "/workspaces/{workspaceId}/agents/checkout",
            post(agent::checkout),
        )
        .route("/workspaces/{workspaceId}/agents", get(agent::list_agents))
        // API Keys
        .route(
            "/workspaces/{workspaceId}/api-keys",
            get(apikey::list_api_keys).post(apikey::create_api_key),
        )
        .route("/api-keys/{id}", delete(apikey::delete_api_key))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    Router::new()
        .merge(public)
        .merge(protected)
        .with_state(state)
}
