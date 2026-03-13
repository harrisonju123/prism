mod agent_roster_panel;
mod notification;
mod agent_spawner;
mod agent_view;
pub mod approval_gate;
mod command_center;
pub mod context_service;
mod dashboard_panel;
mod dashboard_types;
pub mod decision_executor;
mod dispatch;
mod hq_state;
mod inbox_item;
mod inline_forms;
mod navigator_panel;
mod panel_types;
mod plan_dispatch;
mod plan_view;
pub mod review_packet;
mod running_agents;
mod session_history_panel;
mod task_board;
mod task_board_panel;
mod thread_view;
mod types;

pub use approval_gate::{ApprovalDecision, ApprovalGate};
pub use review_packet::ReviewPacket;
pub use agent_roster_panel::AgentRosterPanel;
pub use agent_view::{AgentViewItem, OpenAgentView, open_agent_view};
pub use command_center::{CommandCenterItem, OpenCommandCenter, open_command_center};
pub use context_service::{ContextHandle, ContextService, get_context_handle};
pub use dashboard_panel::PrismDashboardPanel;
pub use dashboard_panel::{Toggle as DashboardToggle, ToggleFocus as DashboardToggleFocus};
pub use dispatch::{DispatchTask, TaskDispatchModal};
pub use hq_state::HqState;
pub use inbox_item::{InboxItem, OpenInbox, open_inbox};
pub use navigator_panel::{FocusNavigator, NavigatorPanel, ToggleNavigator};
pub use plan_dispatch::{DispatchPlan, PlanDispatchModal};
pub use plan_view::{OpenPlanView, PlanViewItem, open_plan_view};
pub use running_agents::RunningAgents;
pub use session_history_panel::SessionHistoryPanel;
pub use task_board::{OpenTaskBoard, TaskBoardItem, open_task_board};
pub use task_board_panel::TaskBoardPanel;
pub use thread_view::{OpenThreadView, ThreadViewItem, open_thread_view};

use gpui::{App, Window};
use notifications::status_toast::{StatusToast, ToastIcon};
use running_agents::RunningAgentsEvent;
use ui::{Color, IconName};
use workspace::Workspace;

pub fn init(cx: &mut App) {
    // Initialize HqState polling — starts the 3-second context polling loop.
    HqState::init_global(cx);
    RunningAgents::init_global(cx);

    // Initialize ContextService (formerly uglyhat-panel) for each new workspace.
    cx.observe_new(|workspace: &mut Workspace, _, cx| {
        if cx.try_global::<ContextService>().is_none() {
            let root = workspace
                .project()
                .read(cx)
                .visible_worktrees(cx)
                .next()
                .map(|wt| wt.read(cx).abs_path().to_path_buf());
            if let Some(root) = root {
                if let Err(e) = ContextService::init(&root, cx) {
                    log::warn!("prism-context service init failed: {e}");
                }
            }
        }
    })
    .detach();

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
            workspace.register_action(|workspace, _: &DashboardToggleFocus, window, cx| {
                workspace.toggle_panel_focus::<PrismDashboardPanel>(window, cx);
            });
            workspace.register_action(|workspace, _: &DashboardToggle, window, cx| {
                if !workspace.toggle_panel_focus::<PrismDashboardPanel>(window, cx) {
                    workspace.close_panel::<PrismDashboardPanel>(window, cx);
                }
            });

            // Show a toast when an agent exits with a "Review" action to open the Inbox.
            if let Some(ra) = RunningAgents::global(cx) {
                cx.subscribe(&ra, |workspace, _ra, event, cx| {
                    let RunningAgentsEvent::AgentExited { agent_name } = event;
                    let name = agent_name.clone();
                    let toast = StatusToast::new(
                        format!("{name} finished — ready for review"),
                        cx,
                        |this: StatusToast, _cx| {
                            this.icon(ToastIcon::new(IconName::Check).color(Color::Success))
                                .action("Review", move |window: &mut Window, cx| {
                                    window.dispatch_action(Box::new(OpenInbox), cx);
                                })
                                .dismiss_button(true)
                        },
                    );
                    workspace.toggle_status_toast(toast, cx);
                })
                .detach();
            }

            // Open Inbox as the default center tab on startup.
            if let (Some(hq_state), Some(window)) = (HqState::global(cx), window) {
                open_inbox(workspace, hq_state, window, cx);
            }
        },
    )
    .detach();
}
