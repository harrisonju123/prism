/// Agent persona system — load named TOML persona files from ~/.prism/personas/.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

/// Search paths for a persona TOML file.
/// Priority: .prism/personas/<name>.toml (project-local) → ~/.prism/personas/<name>.toml (global)
pub fn persona_search_paths(name: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    paths.push(PathBuf::from(".prism/personas").join(format!("{name}.toml")));

    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".prism/personas").join(format!("{name}.toml")));
    }

    paths
}

/// Load a persona by name. Searches project-local then global dirs.
pub fn load_persona(name: &str) -> Result<Persona> {
    for path in persona_search_paths(name) {
        if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("reading persona file {}", path.display()))?;
            let persona: Persona = toml::from_str(&content)
                .with_context(|| format!("parsing persona file {}", path.display()))?;
            return Ok(persona);
        }
    }
    anyhow::bail!(
        "persona '{}' not found. Searched:\n{}",
        name,
        persona_search_paths(name)
            .iter()
            .map(|p| format!("  - {}", p.display()))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// List all persona files discoverable from the current directory and home.
pub fn list_personas() -> Vec<(String, PathBuf)> {
    let mut results = Vec::new();

    let search_dirs: Vec<PathBuf> = {
        let mut dirs = vec![PathBuf::from(".prism/personas")];
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".prism/personas"));
        }
        dirs
    };

    for dir in search_dirs {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("toml") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        results.push((stem.to_string(), path));
                    }
                }
            }
        }
    }

    // Deduplicate by name (project-local takes priority, already first)
    let mut seen = std::collections::HashSet::new();
    results.retain(|(name, _)| seen.insert(name.clone()));

    results
}
