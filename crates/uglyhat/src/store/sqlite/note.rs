use sqlx::Row;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::{Error, Result};
use crate::model::*;

impl SqliteStore {
    pub(crate) async fn create_note_impl(
        &self,
        workspace_id: Option<Uuid>,
        initiative_id: Option<Uuid>,
        epic_id: Option<Uuid>,
        task_id: Option<Uuid>,
        decision_id: Option<Uuid>,
        title: &str,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Note> {
        let now = now_rfc3339();
        let id = Uuid::new_v4();
        let row = sqlx::query(
            "INSERT INTO notes (id, workspace_id, initiative_id, epic_id, task_id, decision_id, title, content, metadata, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             RETURNING id, workspace_id, initiative_id, epic_id, task_id, decision_id, title, content, metadata, created_at, updated_at",
        )
        .bind(id.to_string())
        .bind(opt_uuid_to_str(workspace_id))
        .bind(opt_uuid_to_str(initiative_id))
        .bind(opt_uuid_to_str(epic_id))
        .bind(opt_uuid_to_str(task_id))
        .bind(opt_uuid_to_str(decision_id))
        .bind(title)
        .bind(content)
        .bind(opt_value_to_str(&metadata))
        .bind(&now)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;
        row_to_note(&row)
    }

    pub(crate) async fn get_note_impl(&self, id: Uuid) -> Result<Note> {
        let row = sqlx::query(
            "SELECT id, workspace_id, initiative_id, epic_id, task_id, decision_id, title, content, metadata, created_at, updated_at
             FROM notes WHERE id = $1",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("note {id} not found")))?;
        row_to_note(&row)
    }

    pub(crate) async fn list_notes_by_parent_impl(
        &self,
        parent_type: &str,
        parent_id: Uuid,
    ) -> Result<Vec<Note>> {
        let column = match parent_type {
            "workspace" => "workspace_id",
            "initiative" => "initiative_id",
            "epic" => "epic_id",
            "task" => "task_id",
            "decision" => "decision_id",
            other => {
                return Err(Error::BadRequest(format!(
                    "invalid parent type {other:?}: must be workspace, initiative, epic, task, or decision"
                )));
            }
        };

        let query = format!(
            "SELECT id, workspace_id, initiative_id, epic_id, task_id, decision_id, title, content, metadata, created_at, updated_at
             FROM notes WHERE {column} = $1 ORDER BY created_at DESC"
        );

        let rows = sqlx::query(&query)
            .bind(parent_id.to_string())
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(row_to_note).collect()
    }

    pub(crate) async fn update_note_impl(
        &self,
        id: Uuid,
        title: &str,
        content: &str,
        metadata: Option<serde_json::Value>,
    ) -> Result<Note> {
        let now = now_rfc3339();
        let row = sqlx::query(
            "UPDATE notes
             SET title = $1, content = $2, metadata = $3, updated_at = $4
             WHERE id = $5
             RETURNING id, workspace_id, initiative_id, epic_id, task_id, decision_id, title, content, metadata, created_at, updated_at",
        )
        .bind(title)
        .bind(content)
        .bind(opt_value_to_str(&metadata))
        .bind(&now)
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| Error::NotFound(format!("note {id} not found")))?;
        row_to_note(&row)
    }

    pub(crate) async fn delete_note_impl(&self, id: Uuid) -> Result<()> {
        let result = sqlx::query("DELETE FROM notes WHERE id = $1")
            .bind(id.to_string())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(Error::NotFound(format!("note {id} not found")));
        }
        Ok(())
    }
}

fn row_to_note(row: &sqlx::sqlite::SqliteRow) -> Result<Note> {
    let id_str: String = row.try_get("id")?;
    let ws_id_str: Option<String> = row.try_get("workspace_id")?;
    let init_id_str: Option<String> = row.try_get("initiative_id")?;
    let epic_id_str: Option<String> = row.try_get("epic_id")?;
    let task_id_str: Option<String> = row.try_get("task_id")?;
    let dec_id_str: Option<String> = row.try_get("decision_id")?;
    let meta_str: Option<String> = row.try_get("metadata")?;
    let created_str: String = row.try_get("created_at")?;
    let updated_str: String = row.try_get("updated_at")?;

    Ok(Note {
        id: parse_uuid(&id_str)?,
        workspace_id: parse_opt_uuid(ws_id_str)?,
        initiative_id: parse_opt_uuid(init_id_str)?,
        epic_id: parse_opt_uuid(epic_id_str)?,
        task_id: parse_opt_uuid(task_id_str)?,
        decision_id: parse_opt_uuid(dec_id_str)?,
        title: row.try_get("title")?,
        content: row.try_get("content")?,
        metadata: str_to_opt_value(meta_str),
        created_at: parse_time(&created_str)?,
        updated_at: parse_time(&updated_str)?,
    })
}
