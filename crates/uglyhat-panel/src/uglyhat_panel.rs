mod agent_roster_panel;
mod approval_gate;
mod service;
mod session_history_panel;
mod task_board_panel;
mod types;

pub use agent_roster_panel::AgentRosterPanel;
pub use approval_gate::{ApprovalDecision, ApprovalGate};
pub use service::UglyhatService;
pub use session_history_panel::SessionHistoryPanel;
pub use task_board_panel::TaskBoardPanel;

use agent_roster_panel::{
    PickWorktree, SetAgentName, SpawnAgentInWorktree, Toggle as RosterToggle,
    ToggleFocus as RosterToggleFocus,
};
use gpui::App;
use session_history_panel::{Toggle as HistoryToggle, ToggleFocus as HistoryToggleFocus};
use task_board_panel::{Toggle, ToggleFocus};
use workspace::Workspace;

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, cx| {
        // Initialize UglyhatService from workspace root (best-effort)
        if cx.try_global::<UglyhatService>().is_none() {
            let root = workspace
                .project()
                .read(cx)
                .visible_worktrees(cx)
                .next()
                .map(|wt| wt.read(cx).abs_path().to_path_buf());
            if let Some(root) = root {
                if let Err(e) = UglyhatService::init(&root, cx) {
                    eprintln!("uglyhat service init failed: {e}");
                }
            }
        }
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<TaskBoardPanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &Toggle, window, cx| {
            if !workspace.toggle_panel_focus::<TaskBoardPanel>(window, cx) {
                workspace.close_panel::<TaskBoardPanel>(window, cx);
            }
        });

        workspace.register_action(|workspace, _: &RosterToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<AgentRosterPanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &RosterToggle, window, cx| {
            if !workspace.toggle_panel_focus::<AgentRosterPanel>(window, cx) {
                workspace.close_panel::<AgentRosterPanel>(window, cx);
            }
        });

        workspace.register_action(|workspace, _: &HistoryToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<SessionHistoryPanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &HistoryToggle, window, cx| {
            if !workspace.toggle_panel_focus::<SessionHistoryPanel>(window, cx) {
                workspace.close_panel::<SessionHistoryPanel>(window, cx);
            }
        });

        workspace.register_action(|workspace, _: &SpawnAgentInWorktree, _window, cx| {
            let app_state = workspace.app_state().clone();
            let Some(repo_root) = workspace
                .project()
                .read(cx)
                .visible_worktrees(cx)
                .next()
                .map(|wt| wt.read(cx).abs_path().to_path_buf())
            else {
                return;
            };
            if let Some(panel) = workspace.panel::<AgentRosterPanel>(cx) {
                panel.update(cx, |panel, cx| {
                    panel.spawn_worktree_agent(app_state, repo_root, cx);
                });
            }
        });

        workspace.register_action(|workspace, _: &PickWorktree, _window, cx| {
            let app_state = workspace.app_state().clone();
            let Some(repo_root) = workspace
                .project()
                .read(cx)
                .visible_worktrees(cx)
                .next()
                .map(|wt| wt.read(cx).abs_path().to_path_buf())
            else {
                return;
            };
            if let Some(panel) = workspace.panel::<AgentRosterPanel>(cx) {
                panel.update(cx, |panel, cx| {
                    panel.pick_worktree(app_state, repo_root, cx);
                });
            }
        });

        workspace.register_action(|workspace, _: &SetAgentName, _window, cx| {
            if let Some(panel) = workspace.panel::<AgentRosterPanel>(cx) {
                panel.update(cx, |panel, cx| {
                    panel.toggle_agent_name_input(cx);
                });
            }
        });
    })
    .detach();
}
