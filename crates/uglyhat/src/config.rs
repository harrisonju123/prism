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
