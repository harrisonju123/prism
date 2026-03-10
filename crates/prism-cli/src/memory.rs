use anyhow::Result;
use std::path::{Path, PathBuf};

const MAX_DEFAULT_BYTES: usize = 4096;

pub struct MemoryManager {
    path: PathBuf,
    pending: Vec<(String, String)>,
    window_size: usize,
}

impl MemoryManager {
    pub fn new(memory_dir: &Path, window_size: usize) -> Self {
        Self {
            path: memory_dir.join("MEMORY.md"),
            pending: Vec::new(),
            window_size,
        }
    }

    pub fn load(&self) -> String {
        match std::fs::read_to_string(&self.path) {
            Ok(content) => {
                if content.len() > self.window_size {
                    // Truncate to window_size from the end (most recent memories)
                    let start = content.len() - self.window_size;
                    format!("[... memory truncated ...]\n{}", &content[start..])
                } else {
                    content
                }
            }
            Err(_) => String::new(),
        }
    }

    pub fn append(&mut self, key: String, value: String) {
        self.pending.push((key, value));
    }

    pub fn flush(&self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        // Create parent dirs if needed
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut content = std::fs::read_to_string(&self.path).unwrap_or_default();
        let now = chrono::Utc::now().format("%Y-%m-%d").to_string();
        for (key, value) in &self.pending {
            content.push_str(&format!("\n## {key}\n{value}\n<!-- saved {now} -->\n"));
        }
        std::fs::write(&self.path, &content)?;
        Ok(())
    }
}

impl Default for MemoryManager {
    fn default() -> Self {
        let memory_dir = crate::config::prism_home().join("memory");
        Self::new(&memory_dir, MAX_DEFAULT_BYTES)
    }
}
