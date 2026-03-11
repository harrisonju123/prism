/// Agent persona system — load named TOML persona files from ~/.prism/personas/.
///
/// A persona overrides/merges into the base Config: system_prompt, model,
/// max_turns, max_cost_usd, and tool permission settings.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::config::{Config, SandboxMode};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Persona {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// System prompt override. Replaces the default SYSTEM_PROMPT if set.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Model override.
    #[serde(default)]
    pub model: Option<String>,
    /// Max turns override.
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Cost cap in USD.
    #[serde(default)]
    pub max_cost_usd: Option<f64>,
    /// Allowed tools (None = defer to config).
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,
    /// Additional denied tools (merged with config).
    #[serde(default)]
    pub denied_tools: Option<Vec<String>>,
    /// Sandbox mode override.
    #[serde(default)]
    pub sandbox_mode: Option<String>,
}

impl Persona {
    /// Apply this persona's settings onto a Config, mutating it in place.
    pub fn apply(&self, config: &mut Config) {
        if let Some(ref sp) = self.system_prompt {
            config.model.system_prompt = Some(sp.clone());
        }
        if let Some(ref m) = self.model {
            config.model.model = m.clone();
        }
        if let Some(t) = self.max_turns {
            config.model.max_turns = t;
        }
        if let Some(c) = self.max_cost_usd {
            config.model.max_cost_usd = Some(c);
        }
        if let Some(ref allowed) = self.allowed_tools {
            config.session.allowed_tools = Some(allowed.clone());
        }
        if let Some(ref denied) = self.denied_tools {
            // Merge persona deny list with config deny list
            for tool in denied {
                if !config.session.denied_tools.contains(tool) {
                    config.session.denied_tools.push(tool.clone());
                }
            }
        }
        if let Some(ref mode) = self.sandbox_mode {
            config.session.sandbox_mode = SandboxMode::from_str(mode);
        }
    }
}

/// Search paths for a persona TOML file.
/// Priority: .prism/personas/<name>.toml (project-local) → ~/.prism/personas/<name>.toml (global)
pub fn persona_search_paths(name: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Project-local
    paths.push(PathBuf::from(".prism/personas").join(format!("{name}.toml")));

    // Global (~/.prism/personas/)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persona_apply_overrides_model() {
        let mut config = Config::from_env().unwrap();
        config.model.model = "claude-haiku-4-5".to_string();

        let persona = Persona {
            name: "test".to_string(),
            description: None,
            system_prompt: None,
            model: Some("claude-opus-4-6".to_string()),
            max_turns: Some(5),
            max_cost_usd: None,
            allowed_tools: None,
            denied_tools: None,
            sandbox_mode: None,
        };

        persona.apply(&mut config);
        assert_eq!(config.model.model, "claude-opus-4-6");
        assert_eq!(config.model.max_turns, 5);
    }

    #[test]
    fn persona_merges_denied_tools() {
        let mut config = Config::from_env().unwrap();
        config.session.denied_tools = vec!["bash".to_string()];

        let persona = Persona {
            name: "test".to_string(),
            description: None,
            system_prompt: None,
            model: None,
            max_turns: None,
            max_cost_usd: None,
            allowed_tools: None,
            denied_tools: Some(vec!["write_file".to_string()]),
            sandbox_mode: None,
        };

        persona.apply(&mut config);
        assert!(config.session.denied_tools.contains(&"bash".to_string()));
        assert!(config.session.denied_tools.contains(&"write_file".to_string()));
    }
}
