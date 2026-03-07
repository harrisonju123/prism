use chrono::Utc;
use prism_types::Message;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

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
    pub messages: Vec<Message>,
    pub stop_reason: Option<String>,
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
            version: 1,
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
        }
    }

    /// Atomic write: write to <path>.tmp then rename.
    pub fn save(&self, sessions_dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(sessions_dir)?;
        let path = Self::session_path(sessions_dir, self.episode_id);
        let tmp = path.with_extension("tmp");
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let session = serde_json::from_str(&json)?;
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
            _ => anyhow::bail!("prefix '{prefix}' matches {} sessions; be more specific", matches.len()),
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
                        let task = if session.task.len() > 60 {
                            format!("{}…", &session.task[..57])
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
    }

    #[test]
    fn test_load_by_prefix() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let sessions_dir = temp_dir.path();

        let id = Uuid::parse_str("a3f9c2d1-0000-0000-0000-000000000001").unwrap();
        let session = Session::new(id, "test", "claude-haiku-4-5");
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
        let session = Session::new(id, &long_task, "claude-haiku-4-5");
        session.save(sessions_dir).unwrap();

        let summaries = Session::list_all(sessions_dir).unwrap();
        assert!(summaries[0].task.ends_with('…'));
        assert!(summaries[0].task.len() <= 60);
    }
}
