use globset::{Glob, GlobSetBuilder};
use regex::Regex;
use std::path::Path;
use walkdir::WalkDir;

fn is_hidden_or_noisy(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.') || name == "target" || name == "node_modules"
}

pub fn glob_files(pattern: &str, dir: &str, max_results: usize) -> String {
    let glob = match Glob::new(pattern) {
        Ok(g) => g,
        Err(e) => return format!("error: invalid glob pattern: {e}"),
    };
    let mut builder = GlobSetBuilder::new();
    builder.add(glob);
    let globset = match builder.build() {
        Ok(gs) => gs,
        Err(e) => return format!("error: {e}"),
    };

    let base = Path::new(dir);
    let mut results: Vec<String> = Vec::new();

    for entry in WalkDir::new(dir).follow_links(false).into_iter().flatten() {
        if is_hidden_or_noisy(&entry) {
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = match entry.path().strip_prefix(base) {
            Ok(r) => r,
            Err(_) => entry.path(),
        };
        if globset.is_match(rel) {
            results.push(rel.to_string_lossy().into_owned());
            if results.len() >= max_results {
                break;
            }
        }
    }

    serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string())
}

pub fn grep_files(
    pattern: &str,
    dir: &str,
    file_glob: Option<&str>,
    max_results: usize,
) -> String {
    let re = match Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => return format!("error: invalid regex: {e}"),
    };

    let file_filter: Option<globset::GlobSet> = match file_glob {
        Some(fg) => {
            let g = match Glob::new(fg) {
                Ok(g) => g,
                Err(e) => return format!("error: invalid file_glob: {e}"),
            };
            let mut b = GlobSetBuilder::new();
            b.add(g);
            match b.build() {
                Ok(gs) => Some(gs),
                Err(e) => return format!("error: {e}"),
            }
        }
        None => None,
    };

    let base = Path::new(dir);
    let mut matches: Vec<serde_json::Value> = Vec::new();

    'outer: for entry in WalkDir::new(dir).follow_links(false).into_iter().flatten() {
        if is_hidden_or_noisy(&entry) {
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = match entry.path().strip_prefix(base) {
            Ok(r) => r,
            Err(_) => entry.path(),
        };
        if let Some(ref gs) = file_filter {
            if !gs.is_match(rel) {
                continue;
            }
        }
        let contents = match std::fs::read(entry.path()) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let text = match std::str::from_utf8(&contents) {
            Ok(s) => s,
            Err(_) => continue, // skip binary files
        };
        let rel_str = rel.to_string_lossy().into_owned();
        for (i, line) in text.lines().enumerate() {
            if re.is_match(line) {
                matches.push(serde_json::json!({
                    "path": rel_str,
                    "line": i + 1,
                    "text": line
                }));
                if matches.len() >= max_results {
                    break 'outer;
                }
            }
        }
    }

    serde_json::to_string(&matches).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_temp_tree() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\npub struct Foo;\n").unwrap();
        fs::write(root.join("src/lib.rs"), "pub struct Bar;\n").unwrap();
        fs::write(root.join("tests/test.rs"), "// test file\n").unwrap();
        fs::write(root.join("README.md"), "# readme\n").unwrap();
        dir
    }

    #[test]
    fn test_glob_rs_files() {
        let dir = make_temp_tree();
        let result = glob_files("**/*.rs", dir.path().to_str().unwrap(), 100);
        let paths: Vec<String> = serde_json::from_str(&result).unwrap();
        assert_eq!(paths.len(), 3, "expected 3 .rs files, got: {paths:?}");
        assert!(paths.iter().any(|p| p.ends_with("main.rs")));
        assert!(paths.iter().any(|p| p.ends_with("lib.rs")));
        assert!(paths.iter().any(|p| p.ends_with("test.rs")));
    }

    #[test]
    fn test_glob_max_results() {
        let dir = make_temp_tree();
        let result = glob_files("**/*.rs", dir.path().to_str().unwrap(), 2);
        let paths: Vec<String> = serde_json::from_str(&result).unwrap();
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn test_glob_bad_pattern() {
        let dir = make_temp_tree();
        let result = glob_files("[invalid", dir.path().to_str().unwrap(), 10);
        assert!(result.starts_with("error:"));
    }

    #[test]
    fn test_grep_pub_struct() {
        let dir = make_temp_tree();
        let result = grep_files("pub struct", dir.path().to_str().unwrap(), None, 50);
        let matches: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(matches.len(), 2);
        let texts: Vec<&str> = matches.iter().map(|m| m["text"].as_str().unwrap()).collect();
        assert!(texts.iter().all(|t| t.contains("pub struct")));
    }

    #[test]
    fn test_grep_with_file_glob() {
        let dir = make_temp_tree();
        // Only search src/*.rs, should find 2 pub struct
        let result = grep_files(
            "pub struct",
            dir.path().to_str().unwrap(),
            Some("src/*.rs"),
            50,
        );
        let matches: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_grep_max_results() {
        let dir = make_temp_tree();
        let result = grep_files("pub struct", dir.path().to_str().unwrap(), None, 1);
        let matches: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_grep_bad_regex() {
        let dir = make_temp_tree();
        let result = grep_files("(unclosed", dir.path().to_str().unwrap(), None, 10);
        assert!(result.starts_with("error:"));
    }
}
