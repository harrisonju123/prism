use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const CODE_EXTENSIONS: &[&str] = &[
    "go", "rs", "ts", "tsx", "js", "jsx", "py", "java", "c", "cpp", "h", "cs", "rb", "swift",
    "kt",
];

fn has_code_files(files: &[String]) -> bool {
    files.iter().any(|f| {
        std::path::Path::new(f)
            .extension()
            .and_then(|e| e.to_str())
            .map(|ext| CODE_EXTENSIONS.contains(&ext))
            .unwrap_or(false)
    })
}

/// Run the compile check command if any code files were touched this turn.
///
/// Returns a formatted message to inject into the conversation, or None if the check
/// was skipped (no code files touched).
pub async fn run_compile_check(
    files_this_turn: &[String],
    command: &str,
    timeout_secs: u64,
    cwd: Option<&str>,
) -> Option<String> {
    if files_this_turn.is_empty() || !has_code_files(files_this_turn) {
        return None;
    }

    let mut cmd = Command::new("sh");
    cmd.args(["-c", command])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    let child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Some(format!(
                "[Compile Check] `{command}` failed to spawn: {e}"
            ));
        }
    };

    // Cap at 5 minutes — consistent with shell.rs capping at 2 minutes for interactive tools,
    // but compile checks legitimately take longer for large workspaces.
    let timeout_secs = timeout_secs.min(300);

    match tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await {
        Ok(Ok(output)) => {
            let code = output.status.code().unwrap_or(-1);
            if code == 0 {
                Some(format!("[Compile Check] `{command}` passed (exit code 0)."))
            } else {
                // Prefer stderr; fall back to stdout if stderr is empty
                let detail = if output.stderr.iter().any(|b| !b.is_ascii_whitespace()) {
                    String::from_utf8_lossy(&output.stderr).into_owned()
                } else {
                    String::from_utf8_lossy(&output.stdout).into_owned()
                };
                Some(format!(
                    "[Compile Check] `{command}` failed (exit code {code})\n\nstderr:\n{detail}\n\nFix these compilation errors before proceeding."
                ))
            }
        }
        Ok(Err(e)) => Some(format!("[Compile Check] `{command}` io error: {e}")),
        Err(_) => Some(format!(
            "[Compile Check] `{command}` timed out after {timeout_secs}s."
        )),
    }
}
