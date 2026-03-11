use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CONFIG_FILE: &str = ".uglyhat.json";
pub const DB_FILE: &str = ".uglyhat.db";

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub db_path: String,
}

/// Walk up from `start_dir` looking for `.uglyhat.json`.
pub fn find_config(start_dir: &Path) -> Option<PathBuf> {
    let mut dir = start_dir.to_path_buf();
    loop {
        let path = dir.join(CONFIG_FILE);
        if path.exists() {
            return Some(path);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Parse a `.uglyhat.json` file.
pub fn load_config(path: &Path) -> Result<Config, String> {
    let data = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&data).map_err(|e| e.to_string())
}

/// Resolve the database path from a config and its parent directory.
pub fn resolve_db_path(config_path: &Path, config: &Config) -> PathBuf {
    if !config.db_path.is_empty() {
        return PathBuf::from(&config.db_path);
    }
    let dir = config_path.parent().unwrap_or(config_path);
    dir.join(DB_FILE)
}

/// Auto-initialize a uglyhat workspace in `dir`.
/// Creates `.uglyhat.db` and `.uglyhat.json`. Safe to call if `.uglyhat.json` already exists
/// (returns the existing path without overwriting).
pub async fn auto_init(
    dir: &std::path::Path,
    workspace_name: &str,
) -> Result<(PathBuf, String), String> {
    use crate::store::Store as _;
    use crate::store::sqlite::SqliteStore;

    let config_path = dir.join(CONFIG_FILE);
    if config_path.exists() {
        let cfg = load_config(&config_path)?;
        return Ok((config_path, cfg.workspace_id));
    }

    let db_path = dir.join(DB_FILE);
    let store = SqliteStore::open(&db_path.to_string_lossy())
        .await
        .map_err(|e| format!("open db: {e}"))?;

    let workspace = store
        .init_workspace(workspace_name, "")
        .await
        .map_err(|e| e.to_string())?;

    let config = Config {
        workspace_id: workspace.id.to_string(),
        db_path: String::new(),
    };
    let config_json =
        serde_json::to_string_pretty(&config).map_err(|e| format!("serialize config: {e}"))?;
    std::fs::write(&config_path, &config_json).map_err(|e| format!("write config: {e}"))?;

    Ok((config_path, workspace.id.to_string()))
}
