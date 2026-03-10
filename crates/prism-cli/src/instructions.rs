use std::path::{Path, PathBuf};

const INSTRUCTION_FILE_NAMES: &[&str] = &[
    "CLAUDE.md",
    "PRISM.md",
    "AGENT.md",
    ".rules",
    ".cursorrules",
];

/// Walk up from `start_dir` to the repo root (.git boundary), collecting
/// project instruction files. Returns a formatted string ready for injection
/// into the system prompt via `cwd_section`.
pub fn load_project_instructions(start_dir: &Path) -> String {
    let mut entries: Vec<(PathBuf, String)> = Vec::new();
    let mut dir = start_dir.to_path_buf();

    loop {
        if let Some((path, content)) = find_instruction_file(&dir) {
            entries.push((path, content));
        }

        let is_repo_root = dir.join(".git").exists();

        if is_repo_root || !dir.pop() {
            break;
        }
    }

    // Reverse so root-level files come first (broader → narrower specificity)
    entries.reverse();

    format_instructions(start_dir, &entries)
}

/// Check a single directory for the first matching instruction file.
fn find_instruction_file(dir: &Path) -> Option<(PathBuf, String)> {
    for name in INSTRUCTION_FILE_NAMES {
        let path = dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(content) if !content.trim().is_empty() => return Some((path, content)),
            Ok(_) => continue,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to read instruction file");
                continue;
            }
        }
    }
    None
}

fn format_instructions(cwd: &Path, entries: &[(PathBuf, String)]) -> String {
    let mut out = format!("\n\n## Working Directory\n{}\n", cwd.display());

    for (path, content) in entries {
        let label = path
            .strip_prefix(cwd)
            .or_else(|_| {
                // Try to make it relative to cwd's ancestors
                cwd.ancestors()
                    .find_map(|a| path.strip_prefix(a).ok())
                    .ok_or(())
            })
            .unwrap_or(path.as_path());
        out.push_str(&format!(
            "\n\n## Project Instructions ({})\n{}\n",
            label.display(),
            content.trim()
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_git_repo(dir: &Path) {
        fs::create_dir_all(dir.join(".git")).unwrap();
    }

    #[test]
    fn empty_directory_returns_cwd_only() {
        let tmp = TempDir::new().unwrap();
        setup_git_repo(tmp.path());

        let result = load_project_instructions(tmp.path());
        assert!(result.contains("## Working Directory"));
        assert!(!result.contains("## Project Instructions"));
    }

    #[test]
    fn finds_claude_md_at_cwd() {
        let tmp = TempDir::new().unwrap();
        setup_git_repo(tmp.path());
        fs::write(tmp.path().join("CLAUDE.md"), "# Instructions\nDo stuff").unwrap();

        let result = load_project_instructions(tmp.path());
        assert!(result.contains("## Project Instructions"));
        assert!(result.contains("Do stuff"));
    }

    #[test]
    fn hierarchical_root_and_subdir() {
        let tmp = TempDir::new().unwrap();
        setup_git_repo(tmp.path());
        fs::write(tmp.path().join("CLAUDE.md"), "root instructions").unwrap();

        let sub = tmp.path().join("src");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("PRISM.md"), "subdir instructions").unwrap();

        let result = load_project_instructions(&sub);
        assert!(result.contains("root instructions"));
        assert!(result.contains("subdir instructions"));

        // Root should come before subdir
        let root_pos = result.find("root instructions").unwrap();
        let sub_pos = result.find("subdir instructions").unwrap();
        assert!(root_pos < sub_pos);
    }

    #[test]
    fn priority_claude_md_wins_over_prism_md() {
        let tmp = TempDir::new().unwrap();
        setup_git_repo(tmp.path());
        fs::write(tmp.path().join("CLAUDE.md"), "claude wins").unwrap();
        fs::write(tmp.path().join("PRISM.md"), "prism loses").unwrap();

        let result = load_project_instructions(tmp.path());
        assert!(result.contains("claude wins"));
        assert!(!result.contains("prism loses"));
    }

    #[test]
    fn stops_at_git_boundary() {
        let tmp = TempDir::new().unwrap();
        // Parent has instructions but no .git — repo root is in child
        fs::write(tmp.path().join("CLAUDE.md"), "parent instructions").unwrap();

        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        setup_git_repo(&repo);
        fs::write(repo.join("PRISM.md"), "repo instructions").unwrap();

        let result = load_project_instructions(&repo);
        assert!(result.contains("repo instructions"));
        assert!(!result.contains("parent instructions"));
    }

    #[test]
    fn git_worktree_file_boundary() {
        let tmp = TempDir::new().unwrap();
        // Worktrees use a .git file instead of a .git directory
        fs::write(
            tmp.path().join(".git"),
            "gitdir: /some/other/path/.git/worktrees/foo",
        )
        .unwrap();
        fs::write(tmp.path().join("AGENT.md"), "worktree instructions").unwrap();

        let result = load_project_instructions(tmp.path());
        assert!(result.contains("worktree instructions"));
    }

    #[test]
    fn skips_empty_files() {
        let tmp = TempDir::new().unwrap();
        setup_git_repo(tmp.path());
        fs::write(tmp.path().join("CLAUDE.md"), "   \n  ").unwrap();
        fs::write(tmp.path().join("PRISM.md"), "actual content").unwrap();

        let result = load_project_instructions(tmp.path());
        assert!(result.contains("actual content"));
        assert!(!result.contains("CLAUDE.md"));
    }
}
