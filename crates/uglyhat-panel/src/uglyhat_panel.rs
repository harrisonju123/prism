mod agent_roster_panel;
mod approval_gate;
mod session_history_panel;
mod task_board_panel;
mod types;

pub use agent_roster_panel::AgentRosterPanel;
pub use approval_gate::{ApprovalDecision, ApprovalGate};
pub use session_history_panel::SessionHistoryPanel;
pub use task_board_panel::TaskBoardPanel;

use agent_roster_panel::{Toggle as RosterToggle, ToggleFocus as RosterToggleFocus};
use gpui::App;
use session_history_panel::{Toggle as HistoryToggle, ToggleFocus as HistoryToggleFocus};
use task_board_panel::{Toggle, ToggleFocus};
use workspace::Workspace;

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
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
    })
    .detach();
}
