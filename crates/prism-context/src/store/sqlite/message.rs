use sqlx::Row as _;
use uuid::Uuid;

use super::SqliteStore;
use super::types::*;
use crate::error::Result;
use crate::model::Message;

impl SqliteStore {
    pub(crate) async fn send_message_impl(
        &self,
        workspace_id: Uuid,
        from_agent: &str,
        to_agent: &str,
        content: &str,
        conversation_id: Option<Uuid>,
    ) -> Result<Message> {
        let id = Uuid::new_v4();
        let now = now_rfc3339();
        sqlx::query(
            "INSERT INTO messages (id, workspace_id, from_agent, to_agent, content, read, created_at, conversation_id)
             VALUES ($1, $2, $3, $4, $5, 0, $6, $7)",
        )
        .bind(id.to_string())
        .bind(workspace_id.to_string())
        .bind(from_agent)
        .bind(to_agent)
        .bind(content)
        .bind(&now)
        .bind(conversation_id.map(|u| u.to_string()))
        .execute(&self.pool)
        .await?;

        let message = Message {
            id,
            workspace_id,
            from_agent: from_agent.to_string(),
            to_agent: to_agent.to_string(),
            content: content.to_string(),
            read: false,
            created_at: parse_time(&now)?,
            conversation_id,
        };

        self.log_activity_fire_and_forget(
            workspace_id,
            from_agent,
            "message_sent",
            "message",
            id,
            &format!("Message to '{to_agent}'"),
            None,
        )
        .await;

        Ok(message)
    }

    pub(crate) async fn list_messages_impl(
        &self,
        workspace_id: Uuid,
        to_agent: &str,
        unread_only: bool,
    ) -> Result<Vec<Message>> {
        let sql = if unread_only {
            "SELECT id, workspace_id, from_agent, to_agent, content, read, created_at, conversation_id
             FROM messages
             WHERE workspace_id = $1 AND to_agent = $2 AND read = 0
             ORDER BY created_at ASC"
        } else {
            "SELECT id, workspace_id, from_agent, to_agent, content, read, created_at, conversation_id
             FROM messages
             WHERE workspace_id = $1 AND to_agent = $2
             ORDER BY created_at ASC"
        };
        let rows = sqlx::query(sql)
            .bind(workspace_id.to_string())
            .bind(to_agent)
            .fetch_all(&self.pool)
            .await?;

        rows.iter().map(row_to_message).collect()
    }

    pub(crate) async fn mark_messages_read_impl(
        &self,
        workspace_id: Uuid,
        to_agent: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE messages SET read = 1 WHERE workspace_id = $1 AND to_agent = $2 AND read = 0",
        )
        .bind(workspace_id.to_string())
        .bind(to_agent)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn count_unread_messages_impl(
        &self,
        workspace_id: Uuid,
        to_agent: &str,
    ) -> Result<i64> {
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM messages WHERE workspace_id = $1 AND to_agent = $2 AND read = 0",
        )
        .bind(workspace_id.to_string())
        .bind(to_agent)
        .fetch_one(&self.pool)
        .await?;
        Ok(count)
    }

    pub(crate) async fn count_all_unread_messages_impl(
        &self,
        workspace_id: Uuid,
    ) -> Result<std::collections::HashMap<String, i64>> {
        let rows = sqlx::query(
            "SELECT to_agent, COUNT(*) as cnt
             FROM messages
             WHERE workspace_id = $1 AND read = 0
             GROUP BY to_agent",
        )
        .bind(workspace_id.to_string())
        .fetch_all(&self.pool)
        .await?;

        let mut map = std::collections::HashMap::new();
        for row in rows {
            let agent: String = row.try_get("to_agent")?;
            let count: i64 = row.try_get("cnt")?;
            map.insert(agent, count);
        }
        Ok(map)
    }

    /// Prune messages older than 7 days.
    pub(crate) async fn prune_old_messages_impl(&self, workspace_id: Uuid) -> Result<()> {
        sqlx::query(
            "DELETE FROM messages WHERE workspace_id = $1 AND created_at < datetime('now', '-7 days')",
        )
        .bind(workspace_id.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

fn row_to_message(row: &sqlx::sqlite::SqliteRow) -> Result<Message> {
    let conv_id = row
        .try_get::<Option<String>, _>("conversation_id")?
        .as_deref()
        .map(parse_uuid)
        .transpose()?;
    Ok(Message {
        id: parse_uuid(&row.try_get::<String, _>("id")?)?,
        workspace_id: parse_uuid(&row.try_get::<String, _>("workspace_id")?)?,
        from_agent: row.try_get("from_agent")?,
        to_agent: row.try_get("to_agent")?,
        content: row.try_get("content")?,
        read: row.try_get::<i64, _>("read")? != 0,
        created_at: parse_time(&row.try_get::<String, _>("created_at")?)?,
        conversation_id: conv_id,
    })
}
