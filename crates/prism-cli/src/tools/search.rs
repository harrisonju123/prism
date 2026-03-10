use globset::{Glob, GlobSetBuilder};
use regex::Regex;
use std::path::Path;
use std::time::SystemTime;
use walkdir::WalkDir;

fn is_hidden_or_noisy(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.') || name == "target" || name == "node_modules"
}

/// Walk `dir`, skipping hidden/noisy directories entirely, yielding only files
/// that match an optional glob filter. Returns `(absolute_path, relative_path_string)`.
fn walk_matching_files<'a>(
    dir: &str,
    base: &'a Path,
    file_filter: Option<&'a globset::GlobSet>,
) -> impl Iterator<Item = (std::path::PathBuf, String)> + 'a {
    let dir = dir.to_string();
    WalkDir::new(&dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden_or_noisy(e))
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter_map(move |entry| {
            let rel = match entry.path().strip_prefix(base) {
                Ok(r) => r,
                Err(_) => entry.path(),
            };
            if let Some(gs) = file_filter
                && !gs.is_match(rel)
            {
                return None;
            }
            let rel_owned = rel.to_string_lossy().into_owned();
            Some((entry.into_path(), rel_owned))
        })
}

/// Read a file as UTF-8 text, returning `None` for binary files or read errors.
fn read_text_file(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    // Reject non-UTF-8 (binary) files early
    if std::str::from_utf8(&bytes).is_err() {
        return None;
    }
    Some(bytes)
}

fn build_globset(pattern: &str) -> Result<globset::GlobSet, String> {
    let glob = Glob::new(pattern).map_err(|e| format!("error: invalid glob pattern: {e}"))?;
    let mut builder = GlobSetBuilder::new();
    builder.add(glob);
    builder.build().map_err(|e| format!("error: {e}"))
}

pub fn glob_files(pattern: &str, dir: &str, max_results: usize, sort_by: Option<&str>) -> String {
    let globset = match build_globset(pattern) {
        Ok(gs) => gs,
        Err(e) => return e,
    };

    let base = Path::new(dir);
    let sort_mtime = matches!(sort_by, Some("modified" | "mtime"));

    if sort_mtime {
        let mut entries: Vec<(String, SystemTime)> = Vec::new();
        for (_, rel_str) in walk_matching_files(dir, base, Some(&globset)) {
            let mtime = std::fs::metadata(base.join(&rel_str))
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            entries.push((rel_str, mtime));
        }
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        entries.truncate(max_results);
        let results: Vec<String> = entries.into_iter().map(|(p, _)| p).collect();
        serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string())
    } else {
        let results: Vec<String> = walk_matching_files(dir, base, Some(&globset))
            .take(max_results)
            .map(|(_, rel)| rel)
            .collect();
        serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string())
    }
}

pub fn grep_files(
    pattern: &str,
    dir: &str,
    file_glob: Option<&str>,
    max_results: usize,
    output_mode: Option<&str>,
    context_lines: Option<usize>,
) -> String {
    let re = match Regex::new(pattern) {
        Ok(r) => r,
        Err(e) => return format!("error: invalid regex: {e}"),
    };

    let file_filter: Option<globset::GlobSet> = match file_glob {
        Some(fg) => match build_globset(fg) {
            Ok(gs) => Some(gs),
            Err(e) => return e,
        },
        None => None,
    };

    let base = Path::new(dir);
    let mode = output_mode.unwrap_or("content");

    match mode {
        "content" => grep_content_mode(
            &re,
            dir,
            base,
            file_filter.as_ref(),
            max_results,
            context_lines.unwrap_or(0),
        ),
        "files" => grep_files_mode(&re, dir, base, file_filter.as_ref(), max_results),
        "count" => grep_count_mode(&re, dir, base, file_filter.as_ref(), max_results),
        other => {
            format!("error: unknown output_mode '{other}', expected 'content', 'files', or 'count'")
        }
    }
}

fn grep_content_mode(
    re: &Regex,
    dir: &str,
    base: &Path,
    file_filter: Option<&globset::GlobSet>,
    max_results: usize,
    ctx: usize,
) -> String {
    let mut matches: Vec<serde_json::Value> = Vec::new();

    'outer: for (path, rel_str) in walk_matching_files(dir, base, file_filter) {
        let contents = match read_text_file(&path) {
            Some(b) => b,
            None => continue,
        };
        let text = unsafe { std::str::from_utf8_unchecked(&contents) };

        if ctx == 0 {
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
        } else {
            let lines: Vec<&str> = text.lines().collect();
            let match_indices: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| re.is_match(l))
                .map(|(i, _)| i)
                .collect();

            if match_indices.is_empty() {
                continue;
            }

            let mut ranges: Vec<(usize, usize)> = Vec::new();
            for &idx in &match_indices {
                let start = idx.saturating_sub(ctx);
                let end = (idx + ctx + 1).min(lines.len());
                if let Some(last) = ranges.last_mut()
                    && start <= last.1
                {
                    last.1 = end;
                    continue;
                }
                ranges.push((start, end));
            }

            for (start, end) in ranges {
                for (i, line) in lines.iter().enumerate().take(end).skip(start) {
                    matches.push(serde_json::json!({
                        "path": rel_str,
                        "line": i + 1,
                        "text": *line
                    }));
                    if matches.len() >= max_results {
                        break 'outer;
                    }
                }
            }
        }
    }

    serde_json::to_string(&matches).unwrap_or_else(|_| "[]".to_string())
}

fn grep_files_mode(
    re: &Regex,
    dir: &str,
    base: &Path,
    file_filter: Option<&globset::GlobSet>,
    max_results: usize,
) -> String {
    let mut results: Vec<String> = Vec::new();

    for (path, rel_str) in walk_matching_files(dir, base, file_filter) {
        let contents = match read_text_file(&path) {
            Some(b) => b,
            None => continue,
        };
        let text = unsafe { std::str::from_utf8_unchecked(&contents) };
        if text.lines().any(|line| re.is_match(line)) {
            results.push(rel_str);
            if results.len() >= max_results {
                break;
            }
        }
    }

    serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string())
}

fn grep_count_mode(
    re: &Regex,
    dir: &str,
    base: &Path,
    file_filter: Option<&globset::GlobSet>,
    max_results: usize,
) -> String {
    let mut results: Vec<serde_json::Value> = Vec::new();

    for (path, rel_str) in walk_matching_files(dir, base, file_filter) {
        let contents = match read_text_file(&path) {
            Some(b) => b,
            None => continue,
        };
        let text = unsafe { std::str::from_utf8_unchecked(&contents) };
        let count = text.lines().filter(|line| re.is_match(line)).count();
        if count > 0 {
            results.push(serde_json::json!({
                "path": rel_str,
                "count": count
            }));
            if results.len() >= max_results {
                break;
            }
        }
    }

    serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string())
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
        let result = glob_files("**/*.rs", dir.path().to_str().unwrap(), 100, None);
        let paths: Vec<String> = serde_json::from_str(&result).unwrap();
        assert_eq!(paths.len(), 3, "expected 3 .rs files, got: {paths:?}");
        assert!(paths.iter().any(|p| p.ends_with("main.rs")));
        assert!(paths.iter().any(|p| p.ends_with("lib.rs")));
        assert!(paths.iter().any(|p| p.ends_with("test.rs")));
    }

    #[test]
    fn test_glob_max_results() {
        let dir = make_temp_tree();
        let result = glob_files("**/*.rs", dir.path().to_str().unwrap(), 2, None);
        let paths: Vec<String> = serde_json::from_str(&result).unwrap();
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn test_glob_bad_pattern() {
        let dir = make_temp_tree();
        let result = glob_files("[invalid", dir.path().to_str().unwrap(), 10, None);
        assert!(result.starts_with("error:"));
    }

    #[test]
    fn test_glob_sort_by_modified() {
        let dir = make_temp_tree();
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(
            dir.path().join("src/lib.rs"),
            "pub struct Bar;\n// updated\n",
        )
        .unwrap();

        let result = glob_files(
            "**/*.rs",
            dir.path().to_str().unwrap(),
            100,
            Some("modified"),
        );
        let paths: Vec<String> = serde_json::from_str(&result).unwrap();
        assert_eq!(paths.len(), 3);
        assert!(
            paths[0].ends_with("lib.rs"),
            "expected lib.rs first, got: {paths:?}"
        );
    }

    #[test]
    fn test_glob_sort_by_none_unchanged() {
        let dir = make_temp_tree();
        let result_none = glob_files("**/*.rs", dir.path().to_str().unwrap(), 100, None);
        let result_junk = glob_files("**/*.rs", dir.path().to_str().unwrap(), 100, Some("bogus"));
        let paths_none: Vec<String> = serde_json::from_str(&result_none).unwrap();
        let paths_junk: Vec<String> = serde_json::from_str(&result_junk).unwrap();
        assert_eq!(paths_none, paths_junk);
    }

    #[test]
    fn test_grep_pub_struct() {
        let dir = make_temp_tree();
        let result = grep_files(
            "pub struct",
            dir.path().to_str().unwrap(),
            None,
            50,
            None,
            None,
        );
        let matches: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(matches.len(), 2);
        let texts: Vec<&str> = matches
            .iter()
            .map(|m| m["text"].as_str().unwrap())
            .collect();
        assert!(texts.iter().all(|t| t.contains("pub struct")));
    }

    #[test]
    fn test_grep_with_file_glob() {
        let dir = make_temp_tree();
        let result = grep_files(
            "pub struct",
            dir.path().to_str().unwrap(),
            Some("src/*.rs"),
            50,
            None,
            None,
        );
        let matches: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_grep_max_results() {
        let dir = make_temp_tree();
        let result = grep_files(
            "pub struct",
            dir.path().to_str().unwrap(),
            None,
            1,
            None,
            None,
        );
        let matches: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn test_grep_bad_regex() {
        let dir = make_temp_tree();
        let result = grep_files(
            "(unclosed",
            dir.path().to_str().unwrap(),
            None,
            10,
            None,
            None,
        );
        assert!(result.starts_with("error:"));
    }

    #[test]
    fn test_grep_output_mode_files() {
        let dir = make_temp_tree();
        let result = grep_files(
            "pub struct",
            dir.path().to_str().unwrap(),
            None,
            50,
            Some("files"),
            None,
        );
        let paths: Vec<String> = serde_json::from_str(&result).unwrap();
        assert_eq!(paths.len(), 2);
        let mut deduped = paths.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(deduped.len(), paths.len());
    }

    #[test]
    fn test_grep_output_mode_count() {
        let dir = make_temp_tree();
        let result = grep_files(
            "pub struct",
            dir.path().to_str().unwrap(),
            None,
            50,
            Some("count"),
            None,
        );
        let counts: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(counts.len(), 2);
        for entry in &counts {
            assert!(entry["path"].is_string());
            assert!(entry["count"].as_u64().unwrap() >= 1);
        }
    }

    #[test]
    fn test_grep_context_lines() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("file.txt"),
            "line1\nline2\nMATCH\nline4\nline5\n",
        )
        .unwrap();

        let result = grep_files(
            "MATCH",
            dir.path().to_str().unwrap(),
            None,
            50,
            Some("content"),
            Some(1),
        );
        let matches: Vec<serde_json::Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(
            matches.len(),
            3,
            "expected 3 context lines, got: {matches:?}"
        );
        let line_nums: Vec<u64> = matches
            .iter()
            .map(|m| m["line"].as_u64().unwrap())
            .collect();
        assert_eq!(line_nums, vec![2, 3, 4]);
    }

    #[test]
    fn test_grep_files_mode_max_results() {
        let dir = make_temp_tree();
        let result = grep_files(
            "pub struct",
            dir.path().to_str().unwrap(),
            None,
            1,
            Some("files"),
            None,
        );
        let paths: Vec<String> = serde_json::from_str(&result).unwrap();
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn test_grep_invalid_output_mode() {
        let dir = make_temp_tree();
        let result = grep_files(
            "pub struct",
            dir.path().to_str().unwrap(),
            None,
            10,
            Some("bogus"),
            None,
        );
        assert!(result.starts_with("error:"));
    }
}
