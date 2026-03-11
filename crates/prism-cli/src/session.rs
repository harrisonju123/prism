use chrono::Utc;
use prism_types::Message;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::conversation::ConversationTree;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub episode_id: Uuid,
    pub created_at: String,
    pub updated_at: String,
    pub task: String,
    pub model: String,
    pub turns: u32,
    pub total_prompt_tokens: u32,
    pub total_completion_tokens: u32,
    pub total_cost_usd: f64,
    /// Active path messages — kept for backwards compat with v1 readers.
    /// On save, always updated to match the tree's active path.
    pub messages: Vec<Message>,
    pub stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree: Option<ConversationTree>,
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub episode_id: Uuid,
    pub updated_at: String,
    pub task: String,
    pub model: String,
    pub turns: u32,
    pub total_cost_usd: f64,
    pub stop_reason: Option<String>,
}

impl Session {
    pub fn new(episode_id: Uuid, task: &str, model: &str) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            version: 2,
            episode_id,
            created_at: now.clone(),
            updated_at: now,
            task: task.to_string(),
            model: model.to_string(),
            turns: 0,
            total_prompt_tokens: 0,
            total_completion_tokens: 0,
            total_cost_usd: 0.0,
            messages: Vec::new(),
            stop_reason: None,
            tree: Some(ConversationTree::from_messages(vec![])),
        }
    }

    /// Ensure the tree exists, migrating from v1 if needed.
    fn ensure_tree(&mut self) {
        if self.tree.is_none() {
            self.tree = Some(ConversationTree::from_messages(self.messages.clone()));
        }
    }

    fn tree(&self) -> &ConversationTree {
        // Safe: new() and load() both guarantee tree is Some
        self.tree
            .as_ref()
            .expect("tree must be initialized via new() or load()")
    }

    fn tree_mut(&mut self) -> &mut ConversationTree {
        self.ensure_tree();
        self.tree.as_mut().unwrap()
    }

    /// Messages on the active conversation path.
    pub fn active_messages(&self) -> Vec<Message> {
        self.tree().active_path()
    }

    /// Push a message onto the active conversation path.
    pub fn push_message(&mut self, msg: Message) {
        self.tree_mut().push(msg);
        self.messages = self.tree().active_path();
    }

    /// Number of messages on the active path.
    pub fn active_message_count(&self) -> usize {
        self.tree().active_message_count()
    }

    /// Remove the last assistant turn (assistant + subsequent tool messages).
    /// Returns the number of messages removed from the active path.
    pub fn undo(&mut self) -> usize {
        let removed = self.tree_mut().undo();
        if removed > 0 {
            self.messages = self.tree().active_path();
        }
        removed
    }

    /// Replace the active path with compressed messages.
    pub fn set_active_messages(&mut self, msgs: Vec<Message>) {
        self.tree_mut().replace_active_path(msgs);
        self.messages = self.tree().active_path();
    }

    /// Switch to a specific branch by node ID.
    pub fn switch_branch(&mut self, node_id: u32) {
        self.tree_mut().switch_branch(node_id);
        self.messages = self.tree().active_path();
    }

    /// Atomic write: write to <path>.tmp then rename.
    pub fn save(&mut self, sessions_dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(sessions_dir)?;
        self.ensure_tree();
        self.messages = self.tree().active_path();

        let path = Self::session_path(sessions_dir, self.episode_id);
        let tmp = path.with_extension("tmp");
        let json = serde_json::to_string_pretty(&self)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let mut session: Session = serde_json::from_str(&json)?;
        // Auto-migrate v1 sessions
        if session.tree.is_none() {
            session.tree = Some(ConversationTree::from_messages(session.messages.clone()));
            session.version = 2;
        }
        Ok(session)
    }

    /// Load by UUID prefix (case-insensitive).
    /// "last" loads the most recently updated session.
    pub fn load_by_id_prefix(sessions_dir: &Path, prefix: &str) -> anyhow::Result<Self> {
        if prefix == "last" {
            let summaries = Self::list_all(sessions_dir)?;
            let first = summaries
                .first()
                .ok_or_else(|| anyhow::anyhow!("no sessions found"))?;
            return Self::load(&Self::session_path(sessions_dir, first.episode_id));
        }

        let prefix_lower = prefix.to_lowercase();
        let mut matches = Vec::new();

        if sessions_dir.exists() {
            for entry in std::fs::read_dir(sessions_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "json") {
                    if let Some(filename) = path.file_stem().and_then(|s| s.to_str()) {
                        if filename.to_lowercase().starts_with(&prefix_lower) {
                            matches.push(path);
                        }
                    }
                }
            }
        }

        match matches.len() {
            0 => anyhow::bail!("no session matching prefix '{prefix}'"),
            1 => Self::load(&matches[0]),
            _ => anyhow::bail!(
                "prefix '{prefix}' matches {} sessions; be more specific",
                matches.len()
            ),
        }
    }

    /// List all sessions sorted by updated_at descending.
    pub fn list_all(sessions_dir: &Path) -> anyhow::Result<Vec<SessionSummary>> {
        let mut summaries = Vec::new();

        if !sessions_dir.exists() {
            return Ok(summaries);
        }

        for entry in std::fs::read_dir(sessions_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().map_or(false, |ext| ext == "json") {
                match Self::load(&path) {
                    Ok(session) => {
                        let task = if session.task.chars().count() > 20 {
                            let s: String = session.task.chars().take(20).collect();
                            format!("{s}…")
                        } else {
                            session.task.clone()
                        };
                        summaries.push(SessionSummary {
                            episode_id: session.episode_id,
                            updated_at: session.updated_at,
                            task,
                            model: session.model,
                            turns: session.turns,
                            total_cost_usd: session.total_cost_usd,
                            stop_reason: session.stop_reason,
                        });
                    }
                    Err(e) => {
                        tracing::warn!("failed to load session {}: {e}", path.display());
                    }
                }
            }
        }

        // Sort by updated_at descending (most recent first)
        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        Ok(summaries)
    }

    pub fn delete(sessions_dir: &Path, episode_id: Uuid) -> anyhow::Result<()> {
        let path = Self::session_path(sessions_dir, episode_id);
        std::fs::remove_file(path)?;
        Ok(())
    }

    pub fn session_path(sessions_dir: &Path, id: Uuid) -> PathBuf {
        sessions_dir.join(format!("{id}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prism_types::MessageRole;
    use serde_json::json;

    fn msg(role: MessageRole) -> Message {
        Message {
            role,
            content: Some(json!("test")),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Default::default(),
        }
    }

    fn tool_msg(id: &str) -> Message {
        Message {
            role: MessageRole::Tool,
            content: Some(json!("result")),
            name: None,
            tool_calls: None,
            tool_call_id: Some(id.to_string()),
            extra: Default::default(),
        }
    }

    #[test]
    fn test_session_new() {
        let id = Uuid::new_v4();
        let session = Session::new(id, "test task", "claude-opus-4-6");
        assert_eq!(session.episode_id, id);
        assert_eq!(session.task, "test task");
        assert_eq!(session.model, "claude-opus-4-6");
        assert_eq!(session.turns, 0);
        assert_eq!(session.total_prompt_tokens, 0);
        assert_eq!(session.total_cost_usd, 0.0);
        assert_eq!(session.version, 2);
        assert!(session.tree.is_some());
    }

    #[test]
    fn test_session_save_load() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let sessions_dir = temp_dir.path();

        let id = Uuid::new_v4();
        let mut session = Session::new(id, "test", "claude-haiku-4-5");
        session.turns = 5;
        session.total_prompt_tokens = 100;
        session.total_completion_tokens = 50;
        session.total_cost_usd = 0.001;

        session.save(sessions_dir).unwrap();

        let loaded = Session::load(&Session::session_path(sessions_dir, id)).unwrap();
        assert_eq!(loaded.episode_id, id);
        assert_eq!(loaded.turns, 5);
        assert_eq!(loaded.total_prompt_tokens, 100);
        assert_eq!(loaded.total_completion_tokens, 50);
        assert!(loaded.tree.is_some());
    }

    #[test]
    fn test_load_by_prefix() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let sessions_dir = temp_dir.path();

        let id = Uuid::parse_str("a3f9c2d1-0000-0000-0000-000000000001").unwrap();
        let mut session = Session::new(id, "test", "claude-haiku-4-5");
        session.save(sessions_dir).unwrap();

        // Full UUID
        let loaded = Session::load_by_id_prefix(sessions_dir, "a3f9c2d1").unwrap();
        assert_eq!(loaded.episode_id, id);

        // Prefix
        let loaded = Session::load_by_id_prefix(sessions_dir, "a3f").unwrap();
        assert_eq!(loaded.episode_id, id);
    }

    #[test]
    fn test_list_all_sorted() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let sessions_dir = temp_dir.path();

        // Create sessions with different updated_at times
        let id1 = Uuid::new_v4();
        let mut session1 = Session::new(id1, "task 1", "claude-haiku-4-5");
        session1.updated_at = "2026-01-01T10:00:00Z".to_string();

        let id2 = Uuid::new_v4();
        let mut session2 = Session::new(id2, "task 2", "claude-sonnet-4-6");
        session2.updated_at = "2026-01-02T10:00:00Z".to_string();

        session1.save(sessions_dir).unwrap();
        session2.save(sessions_dir).unwrap();

        let summaries = Session::list_all(sessions_dir).unwrap();
        assert_eq!(summaries.len(), 2);
        // Most recent first
        assert_eq!(summaries[0].episode_id, id2);
        assert_eq!(summaries[1].episode_id, id1);
    }

    #[test]
    fn test_task_truncation() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let sessions_dir = temp_dir.path();

        let id = Uuid::new_v4();
        let long_task = "x".repeat(100);
        let mut session = Session::new(id, &long_task, "claude-haiku-4-5");
        session.save(sessions_dir).unwrap();

        let summaries = Session::list_all(sessions_dir).unwrap();
        assert!(summaries[0].task.ends_with('…'));
        assert!(summaries[0].task.len() <= 60);
    }

    #[test]
    fn test_undo_removes_assistant_turn() {
        let id = Uuid::new_v4();
        let mut session = Session::new(id, "test", "claude-haiku-4-5");
        session.push_message(msg(MessageRole::System));
        session.push_message(msg(MessageRole::User));
        session.push_message(msg(MessageRole::Assistant));

        let removed = session.undo();
        assert_eq!(removed, 1);
        assert_eq!(session.active_message_count(), 2);
        let msgs = session.active_messages();
        assert_eq!(msgs[0].role, MessageRole::System);
        assert_eq!(msgs[1].role, MessageRole::User);
    }

    #[test]
    fn test_undo_removes_assistant_and_tools() {
        let id = Uuid::new_v4();
        let mut session = Session::new(id, "test", "claude-haiku-4-5");
        session.push_message(msg(MessageRole::System));
        session.push_message(msg(MessageRole::User));
        session.push_message(msg(MessageRole::Assistant));
        session.push_message(tool_msg("tc1"));
        session.push_message(tool_msg("tc2"));

        let removed = session.undo();
        assert_eq!(removed, 3);
        assert_eq!(session.active_message_count(), 2);
    }

    #[test]
    fn test_undo_no_assistant_returns_zero() {
        let id = Uuid::new_v4();
        let mut session = Session::new(id, "test", "claude-haiku-4-5");
        session.push_message(msg(MessageRole::System));
        session.push_message(msg(MessageRole::User));

        let removed = session.undo();
        assert_eq!(removed, 0);
        assert_eq!(session.active_message_count(), 2);
    }

    #[test]
    fn test_undo_multiple_turns() {
        let id = Uuid::new_v4();
        let mut session = Session::new(id, "test", "claude-haiku-4-5");
        session.push_message(msg(MessageRole::System));
        session.push_message(msg(MessageRole::User));
        session.push_message(msg(MessageRole::Assistant));
        session.push_message(msg(MessageRole::User));
        session.push_message(msg(MessageRole::Assistant));

        // First undo: removes second assistant
        let removed = session.undo();
        assert_eq!(removed, 1);
        assert_eq!(session.active_message_count(), 4);

        // Second undo: removes first assistant + second user
        let removed = session.undo();
        assert_eq!(removed, 2);
        assert_eq!(session.active_message_count(), 2);
    }

    #[test]
    fn test_v1_migration_on_load() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let sessions_dir = temp_dir.path();

        // Manually write a v1-style session JSON (no tree field)
        let id = Uuid::new_v4();
        let v1_json = serde_json::json!({
            "version": 1,
            "episode_id": id.to_string(),
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "task": "test",
            "model": "claude-haiku-4-5",
            "turns": 1,
            "total_prompt_tokens": 10,
            "total_completion_tokens": 5,
            "total_cost_usd": 0.001,
            "messages": [
                {"role": "system", "content": "you are helpful"},
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": "hi there"}
            ],
            "stop_reason": "stop"
        });

        let path = Session::session_path(sessions_dir, id);
        std::fs::create_dir_all(sessions_dir).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&v1_json).unwrap()).unwrap();

        let loaded = Session::load(&path).unwrap();
        assert_eq!(loaded.version, 2);
        assert!(loaded.tree.is_some());
        assert_eq!(loaded.active_message_count(), 3);
    }
}
