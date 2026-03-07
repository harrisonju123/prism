mod task_board_panel;
mod types;

pub use task_board_panel::TaskBoardPanel;

use gpui::App;
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
    })
    .detach();
}
