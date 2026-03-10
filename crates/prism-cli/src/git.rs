use std::path::Path;
use std::process::Command;

/// Gather git context for the system prompt. Returns empty string if not a git repo.
pub fn gather_git_context(cwd: &Path) -> String {
    if !is_git_repo(cwd) {
        return String::new();
    }

    let mut sections = Vec::new();

    let branch = current_branch(cwd);
    let main = detect_main_branch(cwd);

    if branch.is_some() || main.is_some() {
        if let Some(ref b) = branch {
            sections.push(format!("Current branch: {b}"));
        }
        if let Some(ref m) = main {
            sections.push(format!("Main branch: {m}"));
        }
    }

    if let Some(status) = short_status(cwd) {
        if !status.is_empty() {
            sections.push(format!("\nStatus:\n{status}"));
        }
    }

    if let Some(commits) = recent_commits(cwd, 5) {
        if !commits.is_empty() {
            sections.push(format!("\nRecent commits:\n{commits}"));
        }
    }

    if sections.is_empty() {
        return String::new();
    }

    format!("\n\n## Git Status\n{}\n", sections.join("\n"))
}

fn run_git(cwd: &Path, args: &[&str]) -> Option<String> {
    match Command::new("git").args(args).current_dir(cwd).output() {
        Ok(output) if output.status.success() => {
            let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        }
        Ok(output) => {
            tracing::debug!(
                args = ?args,
                stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                "git command failed"
            );
            None
        }
        Err(e) => {
            tracing::debug!(error = %e, "failed to run git");
            None
        }
    }
}

fn is_git_repo(cwd: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(cwd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn current_branch(cwd: &Path) -> Option<String> {
    run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
}

fn detect_main_branch(cwd: &Path) -> Option<String> {
    for name in &["main", "master", "develop"] {
        let ref_path = format!("refs/heads/{name}");
        if Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", &ref_path])
            .current_dir(cwd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return Some(name.to_string());
        }
    }
    None
}

fn short_status(cwd: &Path) -> Option<String> {
    let output = run_git(cwd, &["status", "--short"])?;
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() > 20 {
        let truncated: String = lines[..20].join("\n");
        Some(format!(
            "{truncated}\n... and {} more files",
            lines.len() - 20
        ))
    } else {
        Some(output)
    }
}

fn recent_commits(cwd: &Path, count: usize) -> Option<String> {
    run_git(cwd, &["log", "--oneline", "-n", &count.to_string()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn not_a_git_repo_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let result = gather_git_context(tmp.path());
        assert!(result.is_empty());
    }

    #[test]
    fn basic_git_repo() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .unwrap();

        fs::write(dir.join("file.txt"), "hello").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial commit"])
            .current_dir(dir)
            .output()
            .unwrap();

        let result = gather_git_context(dir);
        assert!(result.contains("## Git Status"));
        assert!(result.contains("initial commit"));
    }

    #[test]
    fn status_shows_changes() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        Command::new("git")
            .args(["init"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir)
            .output()
            .unwrap();

        fs::write(dir.join("a.txt"), "a").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir)
            .output()
            .unwrap();

        fs::write(dir.join("untracked.txt"), "new").unwrap();

        let result = gather_git_context(dir);
        assert!(result.contains("untracked.txt"));
    }
}
