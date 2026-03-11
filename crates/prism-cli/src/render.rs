use std::fmt::Write;
use std::io::IsTerminal;

use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};
use similar::{ChangeTag, TextDiff};

const DIFF_MAX_LINES: usize = 50;

pub struct Renderer {
    colored: bool,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            colored: std::io::stderr().is_terminal(),
        }
    }

    #[cfg(test)]
    pub fn with_color(colored: bool) -> Self {
        Self { colored }
    }

    pub fn colored(&self) -> bool {
        self.colored
    }

    // ── Diff rendering ──────────────────────────────────────────────

    /// Generate a unified diff between `old` and `new` for display.
    /// Truncates to DIFF_MAX_LINES of diff output.
    pub fn render_diff(&self, path: &str, old: &str, new: &str) -> String {
        let diff = TextDiff::from_lines(old, new);
        let mut out = String::new();

        if self.colored {
            let _ = write!(out, "{}", SetForegroundColor(Color::White));
        }
        let _ = writeln!(out, "--- a/{path}");
        let _ = writeln!(out, "+++ b/{path}");
        if self.colored {
            let _ = write!(out, "{}", ResetColor);
        }

        let mut lines_emitted = 0;
        for group in diff.grouped_ops(3) {
            // Hunk header
            let first = group.first().unwrap();
            let last = group.last().unwrap();
            let old_start = first.old_range().start + 1;
            let old_len = last.old_range().end - first.old_range().start;
            let new_start = first.new_range().start + 1;
            let new_len = last.new_range().end - first.new_range().start;

            if self.colored {
                let _ = write!(out, "{}", SetForegroundColor(Color::Cyan));
            }
            let _ = writeln!(out, "@@ -{old_start},{old_len} +{new_start},{new_len} @@");
            if self.colored {
                let _ = write!(out, "{}", ResetColor);
            }
            lines_emitted += 1;

            for op in &group {
                for change in diff.iter_changes(op) {
                    if lines_emitted >= DIFF_MAX_LINES {
                        if self.colored {
                            let _ = write!(out, "{}", SetForegroundColor(Color::Yellow));
                        }
                        let _ = writeln!(out, "... (diff truncated at {DIFF_MAX_LINES} lines)");
                        if self.colored {
                            let _ = write!(out, "{}", ResetColor);
                        }
                        return out;
                    }

                    let (prefix, color) = match change.tag() {
                        ChangeTag::Delete => ("-", Some(Color::Red)),
                        ChangeTag::Insert => ("+", Some(Color::Green)),
                        ChangeTag::Equal => (" ", None),
                    };

                    if self.colored {
                        if let Some(c) = color {
                            let _ = write!(out, "{}", SetForegroundColor(c));
                        }
                    }
                    let value = change.as_str().unwrap_or("");
                    let _ = write!(out, "{prefix}{value}");
                    if !change.missing_newline() {
                        let _ = writeln!(out);
                    } else {
                        let _ = writeln!(out, "\n\\ No newline at end of file");
                    }
                    if self.colored && color.is_some() {
                        let _ = write!(out, "{}", ResetColor);
                    }
                    lines_emitted += 1;
                }
            }
        }

        out
    }

    // ── Tool status ─────────────────────────────────────────────────

    pub fn tool_start(&self, name: &str, args_preview: &str) {
        if self.colored {
            eprintln!(
                "{}{SetAttribute}[tool]{Reset} {name}  args={args_preview}",
                SetForegroundColor(Color::Cyan),
                SetAttribute = SetAttribute(Attribute::Bold),
                Reset = SetAttribute(Attribute::Reset),
            );
        } else {
            eprintln!("[tool] {name}  args={args_preview}");
        }
    }

    pub fn tool_result(&self, name: &str, elapsed_ms: u128, bytes: usize, preview: &str) {
        if self.colored {
            eprintln!(
                "{}[tool]{} {name}  {elapsed_ms}ms  {bytes} bytes  {preview}",
                SetForegroundColor(Color::Green),
                ResetColor,
            );
        } else {
            eprintln!("[tool] {name}  {elapsed_ms}ms  {bytes} bytes  {preview}");
        }
    }

    pub fn tool_denied(&self, name: &str) {
        if self.colored {
            eprintln!(
                "{}[tool] {name}  permission denied{}",
                SetForegroundColor(Color::Red),
                ResetColor,
            );
        } else {
            eprintln!("[tool] {name}  permission denied");
        }
    }

    pub fn hook_denied(&self, name: &str, message: &str) {
        if self.colored {
            eprintln!(
                "{}[hook] {name}  denied: {message}{}",
                SetForegroundColor(Color::Yellow),
                ResetColor,
            );
        } else {
            eprintln!("[hook] {name}  denied: {message}");
        }
    }

    // ── Background tasks ─────────────────────────────────────────────

    pub fn background_task_spawned(&self, task_id: &str, description: &str) {
        if self.colored {
            eprintln!(
                "{}[bg]{} spawned {task_id}: {description}",
                SetForegroundColor(Color::Cyan),
                ResetColor,
            );
        } else {
            eprintln!("[bg] spawned {task_id}: {description}");
        }
    }

    pub fn background_task_complete(&self, task_id: &str, description: &str, elapsed_secs: f64) {
        if self.colored {
            eprintln!(
                "{}[bg]{} completed {task_id}: {description} ({elapsed_secs:.1}s)\x07",
                SetForegroundColor(Color::Green),
                ResetColor,
            );
        } else {
            eprintln!("[bg] completed {task_id}: {description} ({elapsed_secs:.1}s)\x07");
        }
    }

    // ── Session summary ─────────────────────────────────────────────

    pub fn session_summary(
        &self,
        model: &str,
        turns: u32,
        tokens_in: u32,
        tokens_out: u32,
        cost: f64,
        episode_id: &str,
    ) {
        let cost_str = if cost > 0.0 {
            format!("  ~${cost:.4}")
        } else {
            String::new()
        };
        if self.colored {
            eprintln!(
                "\n{}[session]{} {model}  {turns} turns  {tokens_in} in / {tokens_out} out tokens{cost_str}",
                SetForegroundColor(Color::Magenta),
                ResetColor,
            );
            eprintln!(
                "{}[session]{} episode {episode_id}",
                SetForegroundColor(Color::Magenta),
                ResetColor,
            );
        } else {
            eprintln!(
                "[session] {model}  {turns} turns  {tokens_in} in / {tokens_out} out tokens{cost_str}"
            );
            eprintln!("[session] episode {episode_id}");
        }
    }

    pub fn cost_cap_notice(&self, current: f64, cap: f64) {
        if self.colored {
            eprintln!(
                "\n{}[cost-cap] ${current:.4} >= cap ${cap:.4} — stopping{}",
                SetForegroundColor(Color::Yellow),
                ResetColor,
            );
        } else {
            eprintln!("\n[cost-cap] ${current:.4} >= cap ${cap:.4} — stopping");
        }
    }

    pub fn compile_check(&self, msg: &str) {
        let color = if msg.contains("passed") {
            Color::Green
        } else {
            Color::Yellow
        };
        if self.colored {
            eprintln!("{}[compile]{} {msg}", SetForegroundColor(color), ResetColor,);
        } else {
            eprintln!("[compile] {msg}");
        }
    }

    pub fn interrupt_notice(&self) {
        if self.colored {
            eprintln!(
                "\n{}[interrupt] Ctrl+C — stopping{}",
                SetForegroundColor(Color::Red),
                ResetColor,
            );
        } else {
            eprintln!("\n[interrupt] Ctrl+C — stopping");
        }
    }

    pub fn exploration_nudge(&self) {
        if self.colored {
            eprintln!(
                "{}[exploration nudge]{}",
                SetForegroundColor(Color::Yellow),
                ResetColor,
            );
        } else {
            eprintln!("[exploration nudge]");
        }
    }

    pub fn write_coalesce_nudge(&self) {
        if self.colored {
            eprintln!(
                "{}[write coalesce nudge]{}",
                SetForegroundColor(Color::Yellow),
                ResetColor,
            );
        } else {
            eprintln!("[write coalesce nudge]");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_colored_output() {
        let r = Renderer::with_color(true);
        let old = "fn main() {\n    println!(\"hello\");\n}\n";
        let new = "fn main() {\n    println!(\"world\");\n}\n";
        let diff = r.render_diff("src/main.rs", old, new);
        assert!(diff.contains("--- a/src/main.rs"));
        assert!(diff.contains("+++ b/src/main.rs"));
        assert!(diff.contains("-"));
        assert!(diff.contains("+"));
        // ANSI escape sequences present
        assert!(diff.contains("\x1b["));
    }

    #[test]
    fn diff_plain_output() {
        let r = Renderer::with_color(false);
        let old = "fn main() {\n    println!(\"hello\");\n}\n";
        let new = "fn main() {\n    println!(\"world\");\n}\n";
        let diff = r.render_diff("src/main.rs", old, new);
        assert!(diff.contains("--- a/src/main.rs"));
        assert!(diff.contains("+++ b/src/main.rs"));
        // No ANSI escape sequences
        assert!(!diff.contains("\x1b["));
    }

    #[test]
    fn diff_truncation() {
        let r = Renderer::with_color(false);
        // Generate a large diff — each line is a change
        let old: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let new: String = (0..100).map(|i| format!("changed {i}\n")).collect();
        let diff = r.render_diff("big.txt", &old, &new);
        assert!(diff.contains("diff truncated"));
        // Without truncation this would be ~200+ diff lines (delete+insert per line).
        // Truncation fires at DIFF_MAX_LINES individual change lines, but the total
        // output includes header lines too. Just verify it's well under the untruncated count.
        let line_count = diff.lines().count();
        assert!(
            line_count < 120,
            "got {line_count} lines, expected fewer than 120 (untruncated would be ~200+)"
        );
    }

    #[test]
    fn diff_no_changes() {
        let r = Renderer::with_color(false);
        let same = "fn main() {}\n";
        let diff = r.render_diff("same.rs", same, same);
        // Header lines but no hunks
        assert!(diff.contains("--- a/same.rs"));
        assert!(!diff.contains("@@"));
    }

    #[test]
    fn diff_new_file() {
        let r = Renderer::with_color(false);
        let diff = r.render_diff("new.rs", "", "fn main() {}\n");
        assert!(diff.contains("+fn main() {}"));
    }
}
