mod agent_spawner;
mod agent_view;
pub mod activity_bus;
pub mod approval_gate;
pub mod context_panel;
pub mod context_service;
pub mod decision_executor;
mod dispatch;
mod hq_state;
mod inline_forms;
pub mod mission_panel;
mod notification;
pub mod review_panel;
mod plan_dispatch;
pub mod review_packet;
mod running_agents;
mod status_indicator;
mod thread_view;
mod types;

pub use activity_bus::{AgentActivityBus, AgentActivityBusInner, global_inner as activity_bus_inner};
pub use approval_gate::{ApprovalDecision, ApprovalGate};
pub use context_panel::{ContextPanel, ToggleContextPanel};
pub use mission_panel::{MissionPanel, ToggleMissionPanel};
pub use review_packet::ReviewPacket;
pub use review_panel::{ReviewPanel, ToggleReviewPanel};
pub use agent_view::{AgentViewItem, OpenAgentView, open_agent_view};
pub use context_service::{ContextHandle, ContextService, get_context_handle};
pub use dispatch::{DispatchTask, TaskDispatchModal};
pub use hq_state::HqState;
pub use plan_dispatch::{DispatchPlan, PlanDispatchModal};
pub use running_agents::RunningAgents;
pub use status_indicator::PrismStatusIndicator;
pub use thread_view::{OpenThreadView, ThreadViewItem, open_thread_view};

use gpui::{App, Window};
use notifications::status_toast::{StatusToast, ToastIcon};
use running_agents::RunningAgentsEvent;
use ui::{Color, IconName};
use workspace::Workspace;
use project::Event as ProjectEvent;

pub fn init(cx: &mut App) {
    // Initialize HqState polling — starts the 10-second context polling loop.
    HqState::init_global(cx);
    RunningAgents::init_global(cx);
    // Initialize the activity bus so agent_ui can write live tool activity.
    activity_bus::init(cx);

    // Initialize ContextService for each new workspace.
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
            } else {
                // Worktrees not yet loaded — wait for one to appear.
                let project = workspace.project().clone();
                cx.subscribe(&project, |workspace, _project, event, cx| {
                    if let ProjectEvent::WorktreeAdded(_) = event {
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
                    }
                })
                .detach();
            }
        }
    })
    .detach();

    cx.observe_new(
        |workspace: &mut Workspace, _window: Option<&mut Window>, cx| {
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
            workspace.register_action(|_workspace, _: &OpenThreadView, _window, _cx| {
                // OpenThreadView requires a thread name — triggered programmatically via open_thread_view()
            });
            workspace.register_action(|workspace, _: &ToggleContextPanel, window, cx| {
                workspace.toggle_panel_focus::<ContextPanel>(window, cx);
            });
            workspace.register_action(|workspace, _: &ToggleMissionPanel, window, cx| {
                workspace.toggle_panel_focus::<MissionPanel>(window, cx);
            });
            workspace.register_action(|workspace, _: &ToggleReviewPanel, window, cx| {
                workspace.toggle_panel_focus::<ReviewPanel>(window, cx);
            });

            // Show a toast when an agent exits.
            if let Some(ra) = RunningAgents::global(cx) {
                cx.subscribe(&ra, |workspace, _ra, event, cx| {
                    let RunningAgentsEvent::AgentExited { agent_name } = event;
                    let name = agent_name.clone();
                    let toast = StatusToast::new(
                        format!("{name} finished — ready for review"),
                        cx,
                        |this: StatusToast, _cx| {
                            this.icon(ToastIcon::new(IconName::Check).color(Color::Success))
                                .dismiss_button(true)
                        },
                    );
                    workspace.toggle_status_toast(toast, cx);
                })
                .detach();
            }
        },
    )
    .detach();
}
