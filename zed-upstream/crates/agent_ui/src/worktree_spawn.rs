use std::path::PathBuf;

use collections::HashMap;
use editor::Editor;
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Render, Styled, WeakEntity, Window,
};
use menu::{Cancel, Confirm};
use task::{RevealStrategy, RevealTarget, SpawnInTerminal, TaskId};
use terminal_view::terminal_panel::TerminalPanel;
use ui::{Headline, HeadlineSize, Icon, IconName, IconSize, prelude::*, rems};
use util::ResultExt;
use workspace::{ModalView, Workspace};

/// Modal that prompts for a feature/task name, then:
/// 1. Creates `.worktrees/<name>` via `git worktree add`
/// 2. Claims or creates a uglyhat task with the same name
/// 3. Opens a terminal in the worktree directory with `UH_AGENT_NAME` set
/// 4. Launches a Claude Code session in that terminal
pub struct SpawnWorktreeModal {
    name_editor: Entity<Editor>,
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
}

impl SpawnWorktreeModal {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Enter feature / task name (e.g. my-feature)…", window, cx);
            editor
        });
        let focus_handle = name_editor.focus_handle(cx);
        window.focus(&focus_handle, cx);
        Self {
            name_editor,
            workspace,
            focus_handle,
        }
    }

    fn cancel(&mut self, _: &Cancel, _window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.name_editor.read(cx).text(cx);
        let name = raw.trim().replace(' ', "-");
        if name.is_empty() {
            cx.emit(DismissEvent);
            return;
        }

        let workspace = self.workspace.clone();
        spawn_agent_in_worktree(name, workspace, window, cx);
        cx.emit(DismissEvent);
    }
}

impl EventEmitter<DismissEvent> for SpawnWorktreeModal {}
impl ModalView for SpawnWorktreeModal {}

impl Focusable for SpawnWorktreeModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SpawnWorktreeModal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("SpawnWorktreeModal")
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            .elevation_2(cx)
            .w(rems(34.))
            .child(
                h_flex()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .w_full()
                    .gap_1p5()
                    .child(Icon::new(IconName::GitBranch).size(IconSize::XSmall))
                    .child(Headline::new("Spawn Agent in New Worktree").size(HeadlineSize::XSmall)),
            )
            .child(div().px_3().pb_3().w_full().child(self.name_editor.clone()))
    }
}

/// Resolves the project root from the workspace's first worktree.
fn project_root(workspace: &Workspace, cx: &App) -> Option<PathBuf> {
    let project = workspace.project().read(cx);
    let worktree = project.worktrees(cx).next()?;
    Some(worktree.read(cx).abs_path().to_path_buf())
}

/// Creates a git worktree, then opens a terminal in it running a Claude Code session.
fn spawn_agent_in_worktree(
    name: String,
    workspace: WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut Context<SpawnWorktreeModal>,
) {
    let Some(root) = workspace
        .update(cx, |ws, cx| project_root(ws, cx))
        .ok()
        .flatten()
    else {
        return;
    };

    let worktree_path = root.join(".worktrees").join(&name);
    let agent_name = name.clone();

    // Build the shell command that:
    //   1. Creates the git worktree (idempotent: only if it doesn't exist)
    //   2. Runs uh task claim or create (best-effort, via uh next + uh task claim)
    //   3. Launches Claude Code headlessly in the worktree
    let shell_cmd = format!(
        r#"set -e
WORKTREE_PATH="{worktree_path}"
NAME="{name}"
AGENT_NAME="{agent_name}"

if [ ! -d "$WORKTREE_PATH" ]; then
  git -C "{root}" worktree add "$WORKTREE_PATH" -b "$NAME" 2>/dev/null || \
  git -C "{root}" worktree add "$WORKTREE_PATH" "$NAME"
fi

export PATH="$HOME/.cargo/bin:$PATH"
export UH_AGENT_NAME="$AGENT_NAME"

# Attempt to claim an existing task whose name matches, otherwise create one
TASK_ID=$(~/.cargo/bin/uh next 2>/dev/null | \
  python3 -c "import sys,json; tasks=json.load(sys.stdin); \
  matches=[t for t in (tasks.get('tasks') or []) if '{name}'.lower() in t.get('name','').lower()]; \
  print(matches[0]['id'] if matches else '')" 2>/dev/null || true)

if [ -n "$TASK_ID" ]; then
  ~/.cargo/bin/uh task claim "$TASK_ID" --name "$AGENT_NAME" 2>/dev/null || true
fi

cd "$WORKTREE_PATH"
exec claude --dangerously-skip-permissions
"#,
        worktree_path = worktree_path.display(),
        name = name,
        agent_name = agent_name,
        root = root.display(),
    );

    let spawn_task = SpawnInTerminal {
        id: TaskId(format!("spawn-agent-{}", name)),
        full_label: format!("agent: spawn in worktree ({})", name),
        label: format!("Agent: {}", name),
        command: Some("bash".to_string()),
        args: vec!["-c".to_string(), shell_cmd],
        command_label: format!("claude --dangerously-skip-permissions # worktree: {}", name),
        cwd: Some(worktree_path),
        env: HashMap::from_iter([("UH_AGENT_NAME".to_string(), agent_name)]),
        use_new_terminal: true,
        allow_concurrent_runs: true,
        reveal: RevealStrategy::Always,
        reveal_target: RevealTarget::Dock,
        ..Default::default()
    };

    cx.spawn_in(window, async move |_, cx| {
        workspace
            .update_in(cx, |workspace, window, cx| {
                let Some(terminal_panel) = workspace.panel::<TerminalPanel>(cx) else {
                    return;
                };
                terminal_panel
                    .update(cx, |panel, cx| panel.spawn_task(&spawn_task, window, cx))
                    .detach_and_log_err(cx);
            })
            .log_err();
        anyhow::Ok(())
    })
    .detach();
}

/// Entry point called from the action handler.
pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let handle = workspace.weak_handle();
    workspace.toggle_modal(window, cx, |window, cx| {
        SpawnWorktreeModal::new(handle, window, cx)
    });
}
