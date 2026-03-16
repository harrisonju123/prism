use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Minimal message representation for session deserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchSummary {
    pub node_id: u32,
    pub role: String,
    pub depth: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConversationNode {
    pub id: u32,
    pub parent_id: Option<u32>,
    pub message: Message,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTree {
    nodes: Vec<ConversationNode>,
    next_id: u32,
    active_leaf: u32,
}

impl ConversationTree {
    /// All branch point node IDs (nodes with more than one child).
    pub fn branch_points(&self) -> Vec<(u32, Vec<BranchSummary>)> {
        let mut children_of: HashMap<Option<u32>, Vec<u32>> = HashMap::new();
        for node in &self.nodes {
            children_of.entry(node.parent_id).or_default().push(node.id);
        }

        let mut points: Vec<(u32, Vec<BranchSummary>)> = children_of
            .into_iter()
            .filter(|(_, kids)| kids.len() > 1)
            .filter_map(|(parent, kids)| {
                let parent_id = parent?;
                let summaries: Vec<BranchSummary> = kids
                    .iter()
                    .map(|&kid| BranchSummary {
                        node_id: kid,
                        role: self.nodes[kid as usize].message.role.clone(),
                        depth: self.subtree_depth(kid),
                    })
                    .collect();
                Some((parent_id, summaries))
            })
            .collect();

        points.sort_by_key(|(id, _)| *id);
        points
    }

    fn subtree_depth(&self, node_id: u32) -> usize {
        let children: Vec<u32> = self
            .nodes
            .iter()
            .filter(|n| n.parent_id == Some(node_id))
            .map(|n| n.id)
            .collect();

        if children.is_empty() {
            0
        } else {
            1 + children
                .iter()
                .map(|&c| self.subtree_depth(c))
                .max()
                .unwrap_or(0)
        }
    }
}

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
    #[serde(default)]
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
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let session: Session = serde_json::from_str(&json)?;
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
    fn test_session_load_list() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let sessions_dir = temp_dir.path();

        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();

        let write_session = |id: Uuid, task: &str, updated_at: &str| {
            let v = serde_json::json!({
                "version": 2,
                "episode_id": id.to_string(),
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": updated_at,
                "task": task,
                "model": "claude-haiku-4-5",
                "turns": 1,
                "total_prompt_tokens": 10,
                "total_completion_tokens": 5,
                "total_cost_usd": 0.001,
                "messages": [],
                "stop_reason": null
            });
            let path = Session::session_path(sessions_dir, id);
            std::fs::create_dir_all(sessions_dir).unwrap();
            std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
        };

        write_session(id1, "task 1", "2026-01-01T10:00:00Z");
        write_session(id2, "task 2", "2026-01-02T10:00:00Z");

        let summaries = Session::list_all(sessions_dir).unwrap();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].episode_id, id2);
        assert_eq!(summaries[1].episode_id, id1);
    }

    #[test]
    fn test_load_by_prefix() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let sessions_dir = temp_dir.path();

        let id = Uuid::parse_str("a3f9c2d1-0000-0000-0000-000000000001").unwrap();
        let v = serde_json::json!({
            "version": 2,
            "episode_id": id.to_string(),
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "task": "test",
            "model": "claude-haiku-4-5",
            "turns": 0,
            "total_prompt_tokens": 0,
            "total_completion_tokens": 0,
            "total_cost_usd": 0.0,
            "messages": [],
            "stop_reason": null
        });
        let path = Session::session_path(sessions_dir, id);
        std::fs::create_dir_all(sessions_dir).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();

        let loaded = Session::load_by_id_prefix(sessions_dir, "a3f9c2d1").unwrap();
        assert_eq!(loaded.episode_id, id);

        let loaded = Session::load_by_id_prefix(sessions_dir, "a3f").unwrap();
        assert_eq!(loaded.episode_id, id);
    }

    #[test]
    fn test_task_truncation() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let sessions_dir = temp_dir.path();

        let id = Uuid::new_v4();
        let long_task = "x".repeat(100);
        let v = serde_json::json!({
            "version": 2,
            "episode_id": id.to_string(),
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "task": long_task,
            "model": "claude-haiku-4-5",
            "turns": 0,
            "total_prompt_tokens": 0,
            "total_completion_tokens": 0,
            "total_cost_usd": 0.0,
            "messages": [],
            "stop_reason": null
        });
        let path = Session::session_path(sessions_dir, id);
        std::fs::create_dir_all(sessions_dir).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&v).unwrap()).unwrap();

        let summaries = Session::list_all(sessions_dir).unwrap();
        assert!(summaries[0].task.ends_with('…'));
    }
}
