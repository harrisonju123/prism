mod agent_spawner;
mod agent_view;
mod approval_gate;
mod command_center;
mod dispatch;
mod hq_state;
mod inbox_item;
mod inline_forms;
mod navigator_panel;
mod plan_dispatch;
mod plan_view;
mod running_agents;
mod task_board;
mod thread_view;
mod types;

pub use agent_view::{AgentViewItem, OpenAgentView, open_agent_view};
pub use command_center::{CommandCenterItem, OpenCommandCenter, open_command_center};
pub use dispatch::{DispatchTask, TaskDispatchModal};
pub use hq_state::HqState;
pub use inbox_item::{InboxItem, OpenInbox, open_inbox};
pub use navigator_panel::{FocusNavigator, NavigatorPanel, ToggleNavigator};
pub use plan_dispatch::{DispatchPlan, PlanDispatchModal};
pub use plan_view::{OpenPlanView, PlanViewItem, open_plan_view};
pub use running_agents::RunningAgents;
pub use task_board::{OpenTaskBoard, TaskBoardItem, open_task_board};
pub use thread_view::{OpenThreadView, ThreadViewItem, open_thread_view};

use gpui::{App, Window};
use workspace::Workspace;

pub fn init(cx: &mut App) {
    // Initialize HqState polling — starts the 3-second uglyhat polling loop.
    HqState::init_global(cx);
    RunningAgents::init_global(cx);

    cx.observe_new(
        |workspace: &mut Workspace, window: Option<&mut Window>, cx| {
            workspace.register_action(|workspace, _: &ToggleNavigator, window, cx| {
                workspace.toggle_panel_focus::<NavigatorPanel>(window, cx);
            });
            workspace.register_action(|workspace, _: &FocusNavigator, window, cx| {
                if !workspace.toggle_panel_focus::<NavigatorPanel>(window, cx) {
                    workspace.close_panel::<NavigatorPanel>(window, cx);
                }
            });
            workspace.register_action(|workspace, _: &OpenCommandCenter, window, cx| {
                if let Some(hq_state) = HqState::global(cx) {
                    open_command_center(workspace, hq_state, window, cx);
                }
            });
            workspace.register_action(|workspace, _: &OpenInbox, window, cx| {
                if let Some(hq_state) = HqState::global(cx) {
                    open_inbox(workspace, hq_state, window, cx);
                }
            });
            workspace.register_action(|workspace, _: &OpenTaskBoard, window, cx| {
                if let Some(hq_state) = HqState::global(cx) {
                    open_task_board(workspace, hq_state, window, cx);
                }
            });
            workspace.register_action(|_workspace, _: &OpenAgentView, _window, _cx| {
                // OpenAgentView requires an agent name — triggered programmatically via open_agent_view()
            });
            workspace.register_action(|workspace, _: &DispatchTask, window, cx| {
                let workspace_weak = cx.weak_entity();
                workspace.toggle_modal(window, cx, move |window, cx| {
                    TaskDispatchModal::new(workspace_weak, window, cx)
                });
            });
            workspace.register_action(|workspace, _: &DispatchPlan, window, cx| {
                PlanDispatchModal::open(workspace, window, cx);
            });
            workspace.register_action(|_workspace, _: &OpenPlanView, _window, _cx| {
                // OpenPlanView is triggered programmatically via open_plan_view()
            });
            workspace.register_action(|_workspace, _: &OpenThreadView, _window, _cx| {
                // OpenThreadView requires a thread name — triggered programmatically via open_thread_view()
            });

            // Open Inbox as the default center tab on startup.
            if let (Some(hq_state), Some(window)) = (HqState::global(cx), window) {
                open_inbox(workspace, hq_state, window, cx);
            }
        },
    )
    .detach();
}
