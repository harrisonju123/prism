use anyhow::Result;
use serde_json::json;
use std::path::PathBuf;

/// Writes a lock file to `~/.claude/ide/<port>.lock` in the format expected by
/// MCP-aware agents (Claude Code, etc.) for IDE discovery.
///
/// The file is removed when this struct is dropped, so dropping `NativeAgent`
/// (which owns this) automatically cleans up the lock file.
pub struct IdeLockFile {
    path: PathBuf,
}

impl IdeLockFile {
    /// Write a new lock file. Fails silently (returns Ok) if the directory
    /// cannot be created — the agent just won't be discoverable.
    pub fn create(
        port: u16,
        workspace_folders: Vec<String>,
        auth_token: &str,
    ) -> Result<Self> {
        let ide_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?
            .join(".claude")
            .join("ide");

        std::fs::create_dir_all(&ide_dir)?;

        let path = ide_dir.join(format!("{port}.lock"));

        let contents = json!({
            "workspaceFolders": workspace_folders,
            "pid": std::process::id(),
            "ideName": "PrisM",
            "transport": "ws",
            "authToken": auth_token,
        });

        std::fs::write(&path, serde_json::to_string_pretty(&contents)?)?;

        log::info!("IDE MCP lock file written: {}", path.display());

        Ok(Self { path })
    }
}

impl Drop for IdeLockFile {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => log::debug!("IDE MCP lock file removed: {}", self.path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => log::warn!("failed to remove IDE lock file {}: {e}", self.path.display()),
        }
    }
}
