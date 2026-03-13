use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// New-style config: `.prism/context.json`
pub const CONFIG_DIR: &str = ".prism";
pub const NEW_CONFIG_FILE: &str = "context.json";
pub const NEW_DB_FILE: &str = "context.db";

/// Legacy config file (backward-compat discovery fallback)
pub const LEGACY_CONFIG_FILE: &str = ".uglyhat.json";
pub const LEGACY_DB_FILE: &str = ".uglyhat.db";

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub workspace_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub db_path: String,
}

/// Walk up from `start_dir` looking for `.prism/context.json`, falling back to `.uglyhat.json`.
pub fn find_config(start_dir: &Path) -> Option<PathBuf> {
    let mut dir = start_dir.to_path_buf();
    loop {
        // New-style: .prism/context.json
        let new_path = dir.join(CONFIG_DIR).join(NEW_CONFIG_FILE);
        if new_path.exists() {
            return Some(new_path);
        }
        // Legacy fallback: .uglyhat.json
        let legacy_path = dir.join(LEGACY_CONFIG_FILE);
        if legacy_path.exists() {
            return Some(legacy_path);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Parse a config file (supports both new and legacy formats).
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
    let db_filename = if config_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == NEW_CONFIG_FILE)
        .unwrap_or(false)
    {
        NEW_DB_FILE
    } else {
        LEGACY_DB_FILE
    };
    dir.join(db_filename)
}

/// Auto-initialize a prism-context workspace in `dir`.
/// Creates `.prism/context.db` and `.prism/context.json`.
/// Safe to call if config already exists (returns the existing path without overwriting).
pub async fn auto_init(
    dir: &std::path::Path,
    workspace_name: &str,
) -> Result<(PathBuf, String), String> {
    use crate::store::Store as _;
    use crate::store::sqlite::SqliteStore;

    let new_config_path = dir.join(CONFIG_DIR).join(NEW_CONFIG_FILE);
    let legacy_config_path = dir.join(LEGACY_CONFIG_FILE);

    if new_config_path.exists() {
        let cfg = load_config(&new_config_path)?;
        return Ok((new_config_path, cfg.workspace_id));
    }
    if legacy_config_path.exists() {
        let cfg = load_config(&legacy_config_path)?;
        return Ok((legacy_config_path, cfg.workspace_id));
    }

    let prism_dir = dir.join(CONFIG_DIR);
    std::fs::create_dir_all(&prism_dir).map_err(|e| format!("create .prism dir: {e}"))?;

    let db_path = prism_dir.join(NEW_DB_FILE);
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
    std::fs::write(&new_config_path, &config_json).map_err(|e| format!("write config: {e}"))?;

    Ok((new_config_path, workspace.id.to_string()))
}
