/// Agent persona system — load named TOML persona files from .prism/personas/.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Persona {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default)]
    pub denied_tools: Option<Vec<String>>,
    #[serde(default)]
    pub sandbox_mode: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub await_review: Option<bool>,
}

impl Persona {
    /// Returns true if the tool is permitted by this persona's allow/deny lists.
    /// Allowlist takes precedence over denylist; if neither is set, all tools are allowed.
    pub fn allows_tool(&self, tool_name: &str) -> bool {
        if let Some(ref allowed) = self.allowed_tools {
            allowed.iter().any(|t| t == tool_name)
        } else if let Some(ref denied) = self.denied_tools {
            !denied.iter().any(|t| t == tool_name)
        } else {
            true
        }
    }
}

/// Returns the `.prism/personas` directory for the given project root (or CWD if None).
fn personas_dir(project_root: Option<&Path>) -> PathBuf {
    match project_root {
        Some(root) => root.join(".prism/personas"),
        None => PathBuf::from(".prism/personas"),
    }
}

/// Returns the search-path directories in priority order: project-local, then global home.
fn search_dirs(project_root: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = vec![personas_dir(project_root)];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".prism/personas"));
    }
    dirs
}

/// Search paths for a persona TOML file.
/// Priority: <root>/.prism/personas/<name>.toml (project-local) → ~/.prism/personas/<name>.toml (global)
/// When `project_root` is None, falls back to CWD.
pub fn persona_search_paths(name: &str, project_root: Option<&Path>) -> Vec<PathBuf> {
    search_dirs(project_root)
        .into_iter()
        .map(|dir| dir.join(format!("{name}.toml")))
        .collect()
}

/// Load a persona by name. Searches project-local then global dirs.
/// Pass `project_root` to search in a specific directory; pass `None` to use CWD.
pub fn load_persona(name: &str, project_root: Option<&Path>) -> Result<Persona> {
    for path in persona_search_paths(name, project_root) {
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                let persona: Persona = toml::from_str(&content)
                    .with_context(|| format!("parsing persona file {}", path.display()))?;
                return Ok(persona);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("reading persona file {}", path.display()));
            }
        }
    }
    anyhow::bail!(
        "persona '{}' not found. Searched:\n{}",
        name,
        persona_search_paths(name, project_root)
            .iter()
            .map(|p| format!("  - {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// List all persona files discoverable from the project root and home.
/// Returns (name, path) pairs with project-local taking priority over global.
/// When `project_root` is None, falls back to CWD.
pub fn list_personas(project_root: Option<&Path>) -> Vec<(String, PathBuf)> {
    collect_persona_entries(project_root)
        .into_iter()
        .map(|(name, _, path)| (name, path))
        .collect()
}

/// List personas with their descriptions. Reads each TOML file once.
/// Returns (name, description) pairs; use when the description is needed (e.g. UI pickers).
pub fn list_personas_with_desc(project_root: Option<&Path>) -> Vec<(String, String)> {
    collect_persona_entries(project_root)
        .into_iter()
        .map(|(name, description, _)| (name, description))
        .collect()
}

/// Internal: scan search dirs and return (name, description, path) with deduplication.
fn collect_persona_entries(project_root: Option<&Path>) -> Vec<(String, String, PathBuf)> {
    let mut results: Vec<(String, String, PathBuf)> = Vec::new();

    for dir in search_dirs(project_root) {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        let description = std::fs::read_to_string(&path)
                            .ok()
                            .and_then(|c| toml::from_str::<Persona>(&c).ok())
                            .and_then(|p| p.description)
                            .unwrap_or_default();
                        results.push((stem.to_string(), description, path));
                    }
                }
            }
        }
    }

    // Deduplicate by name (project-local takes priority, already first)
    let mut seen = std::collections::HashSet::new();
    results.retain(|(name, _, _)| seen.insert(name.clone()));

    results
}
