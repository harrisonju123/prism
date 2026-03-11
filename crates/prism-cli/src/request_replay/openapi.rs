/// OpenAPI discovery for agent-first request replay workflows.
/// Priority: explicit env → repo files → running server → swaggo generation.
use anyhow::{Context, Result};
use reqwest::Client;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use walkdir::WalkDir;

const DEFAULT_LOCAL_URL: &str = "http://localhost:9100";
const DEFAULT_OUTPUT_NAME: &str = "openapi.json";
const SCRAPE_TIMEOUT_SECS: u64 = 2;

#[derive(Debug, Clone)]
pub struct OpenApiDiscoveryResult {
    pub source: String,
    pub spec_path: PathBuf,
}

/// Discover or generate an OpenAPI spec and normalize to JSON for replay.
/// Designed to be called by agents as part of endpoint validation flows.
pub async fn discover_or_generate(output_dir: &Path) -> Result<OpenApiDiscoveryResult> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;

    if let Ok(path) = std::env::var("PRISM_OPENAPI_PATH") {
        let path = PathBuf::from(path);
        let value = read_openapi_value(&path)?;
        let output_path = output_dir.join(DEFAULT_OUTPUT_NAME);
        write_openapi_json(&value, &output_path)?;
        return Ok(OpenApiDiscoveryResult {
            source: format!("file:{}", path.display()),
            spec_path: output_path,
        });
    }

    if let Ok(url) = std::env::var("PRISM_OPENAPI_URL") {
        let value = fetch_openapi_value(&url).await?;
        let output_path = output_dir.join(DEFAULT_OUTPUT_NAME);
        write_openapi_json(&value, &output_path)?;
        return Ok(OpenApiDiscoveryResult {
            source: format!("url:{url}"),
            spec_path: output_path,
        });
    }

    if let Some(path) = discover_openapi_file(Path::new(".")) {
        let value = read_openapi_value(&path)?;
        let output_path = output_dir.join(DEFAULT_OUTPUT_NAME);
        write_openapi_json(&value, &output_path)?;
        return Ok(OpenApiDiscoveryResult {
            source: format!("file:{}", path.display()),
            spec_path: output_path,
        });
    }

    if let Some((url, value)) = scrape_openapi_from_running().await? {
        let output_path = output_dir.join(DEFAULT_OUTPUT_NAME);
        write_openapi_json(&value, &output_path)?;
        return Ok(OpenApiDiscoveryResult {
            source: format!("live:{url}"),
            spec_path: output_path,
        });
    }

    if let Some(path) = generate_openapi_with_swag(output_dir)? {
        let value = read_openapi_value(&path)?;
        let output_path = output_dir.join(DEFAULT_OUTPUT_NAME);
        write_openapi_json(&value, &output_path)?;
        return Ok(OpenApiDiscoveryResult {
            source: "swaggo:swag init".to_string(),
            spec_path: output_path,
        });
    }

    anyhow::bail!(
        "OpenAPI spec not found. Set PRISM_OPENAPI_PATH/PRISM_OPENAPI_URL, provide a running server with /openapi.json, or install swag for Go projects."
    );
}

fn discover_openapi_file(root: &Path) -> Option<PathBuf> {
    let candidates = [
        "openapi.json",
        "openapi.yaml",
        "openapi.yml",
        "swagger.json",
        "swagger.yaml",
        "swagger.yml",
        "docs/openapi.json",
        "docs/openapi.yaml",
        "docs/openapi.yml",
        "docs/swagger.json",
        "docs/swagger.yaml",
        "docs/swagger.yml",
        "api/openapi.json",
        "api/openapi.yaml",
        "api/openapi.yml",
        "api/swagger.json",
        "api/swagger.yaml",
        "api/swagger.yml",
    ];

    for rel in candidates {
        let path = root.join(rel);
        if path.exists() {
            return Some(path);
        }
    }

    const SKIP_DIRS: &[&str] = &[
        "node_modules",
        ".git",
        "target",
        "vendor",
        ".next",
        "dist",
        "build",
    ];

    for entry in WalkDir::new(root)
        .max_depth(4)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            !e.file_type().is_dir()
                || !SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref())
        })
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name == "openapi.json"
            || name == "openapi.yaml"
            || name == "openapi.yml"
            || name == "swagger.json"
            || name == "swagger.yaml"
            || name == "swagger.yml"
        {
            return Some(entry.path().to_path_buf());
        }
    }

    None
}

async fn scrape_openapi_from_running() -> Result<Option<(String, Value)>> {
    let base = std::env::var("PRISM_LOCAL_URL")
        .or_else(|_| std::env::var("PRISM_URL"))
        .unwrap_or_else(|_| DEFAULT_LOCAL_URL.to_string());

    let endpoints = [
        "/openapi.json",
        "/swagger.json",
        "/v1/openapi.json",
        "/docs/openapi.json",
        "/docs/swagger.json",
    ];

    let client = Client::builder()
        .timeout(Duration::from_secs(SCRAPE_TIMEOUT_SECS))
        .build()
        .context("failed to build OpenAPI scrape client")?;
    for endpoint in endpoints {
        let url = format!("{base}{endpoint}");
        if let Ok(resp) = client.get(&url).send().await
            && resp.status().is_success()
            && let Ok(text) = resp.text().await
            && let Ok(value) = parse_openapi_text(&text, &url)
        {
            return Ok(Some((url, value)));
        }
    }

    Ok(None)
}

fn generate_openapi_with_swag(output_dir: &Path) -> Result<Option<PathBuf>> {
    let go_mod = Path::new("go.mod");
    if !go_mod.exists() {
        return Ok(None);
    }

    let swag_cmd = std::env::var("PRISM_SWAG_INIT_CMD").ok();
    if let Some(cmd) = swag_cmd {
        let status = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .status()
            .with_context(|| format!("failed to run PRISM_SWAG_INIT_CMD: {cmd}"))?;
        if !status.success() {
            anyhow::bail!("PRISM_SWAG_INIT_CMD exited with status {}", status);
        }
    } else if has_command("swag") {
        let status = Command::new("swag")
            .arg("init")
            .arg("--output")
            .arg(output_dir)
            .status()
            .context("failed to run swag init")?;
        if !status.success() {
            anyhow::bail!("swag init exited with status {}", status);
        }
    } else {
        return Ok(None);
    }

    let json_path = output_dir.join("swagger.json");
    if json_path.exists() {
        return Ok(Some(json_path));
    }

    let yaml_path = output_dir.join("swagger.yaml");
    if yaml_path.exists() {
        return Ok(Some(yaml_path));
    }

    Ok(None)
}

pub(super) fn read_openapi_value(path: &Path) -> Result<Value> {
    let contents =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    parse_openapi_text(&contents, &path.display().to_string())
}

async fn fetch_openapi_value(url: &str) -> Result<Value> {
    let resp = Client::new()
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to fetch OpenAPI from {url}"))?;
    let text = resp
        .text()
        .await
        .with_context(|| format!("failed to read OpenAPI response from {url}"))?;
    parse_openapi_text(&text, url)
}

fn parse_openapi_text(text: &str, source: &str) -> Result<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return Ok(value);
    }
    serde_yaml::from_str::<Value>(text)
        .with_context(|| format!("invalid OpenAPI JSON/YAML from {source}"))
}

fn write_openapi_json(value: &Value, output_path: &Path) -> Result<()> {
    let payload = serde_json::to_string_pretty(value)?;
    fs::write(output_path, payload)
        .with_context(|| format!("failed to write {}", output_path.display()))?;
    Ok(())
}

fn has_command(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_openapi_text_json() {
        let json = r#"{"openapi": "3.0.0", "info": {"title": "Test"}}"#;
        let value = parse_openapi_text(json, "test").unwrap();
        assert_eq!(value["openapi"], "3.0.0");
        assert_eq!(value["info"]["title"], "Test");
    }

    #[test]
    fn test_parse_openapi_text_yaml() {
        let yaml = "openapi: '3.0.0'\ninfo:\n  title: Test\n";
        let value = parse_openapi_text(yaml, "test").unwrap();
        assert_eq!(value["openapi"], "3.0.0");
        assert_eq!(value["info"]["title"], "Test");
    }

    #[test]
    fn test_parse_openapi_text_invalid() {
        // YAML is very permissive — plain strings parse as valid scalars.
        // Use unbalanced brackets to trigger a real parse error.
        let result = parse_openapi_text("{ unclosed: [", "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_has_command_exists() {
        assert!(has_command("cargo"));
    }

    #[test]
    fn test_has_command_missing() {
        assert!(!has_command("definitely_not_a_real_command_xyz"));
    }
}
