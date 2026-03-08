use uuid::Uuid;

use super::SqliteStore;
use crate::error::Result;
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn get_task_context_impl(&self, task_id: Uuid) -> Result<TaskContext> {
        // First, get the task itself
        let task = self.get_task_impl(task_id).await?;

        let epic_id = task.epic_id;
        let initiative_id = task.initiative_id;

        // Run remaining queries concurrently
        let (
            initiative,
            epic,
            sibling_tasks,
            deps_pair,
            notes,
            epic_decisions,
            init_decisions,
            recent_activity,
            handoffs,
        ): (
            Initiative,
            Epic,
            Vec<TaskSummary>,
            (Vec<DependencyInfo>, Vec<DependencyInfo>),
            Vec<Note>,
            Vec<Decision>,
            Vec<Decision>,
            Vec<ActivityEntry>,
            Vec<Handoff>,
        ) = tokio::try_join!(
            self.get_initiative_impl(initiative_id),
            self.get_epic_impl(epic_id),
            self.scan_task_summaries(
                "SELECT t.id, t.name, t.status, t.priority, t.assignee,
                        ep.name AS epic_name, i.name AS initiative_name, t.domain_tags, t.created_at
                 FROM tasks t
                 JOIN epics ep ON ep.id = t.epic_id
                 JOIN initiatives i ON i.id = t.initiative_id
                 WHERE t.epic_id = $1 AND t.id != $2
                 ORDER BY t.created_at",
                vec![epic_id.to_string(), task_id.to_string()],
            ),
            self.get_dependencies_impl(task_id),
            self.list_notes_by_parent_impl("task", task_id),
            self.fetch_decisions_by_epic(epic_id),
            self.fetch_decisions_by_initiative(initiative_id),
            self.fetch_recent_activity_for_epic(epic_id),
            self.get_handoffs_by_task_impl(task_id),
        )?;
        let (blocks, blocked_by) = deps_pair;

        Ok(TaskContext {
            task,
            initiative: Some(initiative),
            epic: Some(epic),
            sibling_tasks,
            blocks,
            blocked_by,
            notes,
            epic_decisions,
            initiative_decisions: init_decisions,
            recent_activity,
            handoffs,
        })
    }

    pub(crate) async fn fetch_decisions_by_epic(&self, epic_id: Uuid) -> Result<Vec<Decision>> {
        let rows = sqlx::query(
            "SELECT d.id, d.workspace_id, COALESCE(w.name, '') AS workspace_name,
                    d.initiative_id, COALESCE(i.name, '') AS initiative_name,
                    d.epic_id, COALESCE(ep.name, '') AS epic_name,
                    d.title, d.content, d.status, d.metadata, d.created_at, d.updated_at
             FROM decisions d
             LEFT JOIN workspaces w ON w.id = d.workspace_id
             LEFT JOIN initiatives i ON i.id = d.initiative_id
             LEFT JOIN epics ep ON ep.id = d.epic_id
             WHERE d.epic_id = $1 ORDER BY d.created_at DESC",
        )
        .bind(epic_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(super::decision::row_to_decision).collect()
    }

    pub(crate) async fn fetch_decisions_by_initiative(
        &self,
        initiative_id: Uuid,
    ) -> Result<Vec<Decision>> {
        let rows = sqlx::query(
            "SELECT d.id, d.workspace_id, COALESCE(w.name, '') AS workspace_name,
                    d.initiative_id, COALESCE(i.name, '') AS initiative_name,
                    d.epic_id, COALESCE(ep.name, '') AS epic_name,
                    d.title, d.content, d.status, d.metadata, d.created_at, d.updated_at
             FROM decisions d
             LEFT JOIN workspaces w ON w.id = d.workspace_id
             LEFT JOIN initiatives i ON i.id = d.initiative_id
             LEFT JOIN epics ep ON ep.id = d.epic_id
             WHERE d.initiative_id = $1 ORDER BY d.created_at DESC",
        )
        .bind(initiative_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(super::decision::row_to_decision).collect()
    }

    pub(crate) async fn fetch_recent_activity_for_epic(
        &self,
        epic_id: Uuid,
    ) -> Result<Vec<ActivityEntry>> {
        let rows = sqlx::query(
            "SELECT a.id, a.workspace_id, a.actor, a.action, a.entity_type, a.entity_id, a.summary, a.detail, a.created_at
             FROM activity_log a
             JOIN tasks t ON t.id = a.entity_id AND a.entity_type = 'task'
             WHERE t.epic_id = $1
             ORDER BY a.created_at DESC
             LIMIT 20",
        )
        .bind(epic_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        rows.iter()
            .map(super::activity::row_to_activity_entry)
            .collect()
    }
}
