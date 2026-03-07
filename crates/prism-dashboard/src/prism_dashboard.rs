mod panel;
mod types;

pub use panel::PrismDashboardPanel;
use panel::{Toggle, ToggleFocus};
use workspace::Workspace;

pub fn init(cx: &mut gpui::App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<PrismDashboardPanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &Toggle, window, cx| {
            if !workspace.toggle_panel_focus::<PrismDashboardPanel>(window, cx) {
                workspace.close_panel::<PrismDashboardPanel>(window, cx);
            }
        });
    })
    .detach();
}
