use editor::Editor;
use gpui::{
    App, AppContext as _, Context, DismissEvent, Entity, EventEmitter, Focusable, IntoElement,
    ParentElement, Render, Styled, Task, WeakEntity, Window, actions, px,
};
use ui::{Button, ButtonStyle, Color, Label, LabelSize, h_flex, prelude::*, v_flex};
use workspace::{ModalView, Workspace};

use crate::agent_spawner::spawn_agent_in_worktree;
use crate::agent_view::open_agent_view;
use crate::dispatch::slugify;
use uglyhat_panel::UglyhatService;

actions!(prism_hq, [DispatchPlan]);

/// A draft work package before it's written to the store.
#[derive(Clone, Debug)]
pub struct WorkPackageDraft {
    pub intent: String,
    pub ordinal: i32,
    /// Index of WP this one depends on (linear deps: each depends on previous).
    pub depends_on_ordinal: Option<i32>,
}

/// Decompose a multi-line intent into draft work packages.
/// Each non-empty line → one package, sequential deps.
pub fn decompose_intent(text: &str) -> Vec<WorkPackageDraft> {
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .enumerate()
        .map(|(i, line)| WorkPackageDraft {
            intent: line.to_string(),
            ordinal: i as i32,
            depends_on_ordinal: if i > 0 { Some((i - 1) as i32) } else { None },
        })
        .collect()
}

enum ModalStep {
    InputIntent,
    ReviewPlan { drafts: Vec<WorkPackageDraft> },
    Executing,
}

pub struct PlanDispatchModal {
    step: ModalStep,
    intent_editor: Entity<Editor>,
    error: Option<String>,
    workspace: WeakEntity<Workspace>,
    exec_task: Option<Task<()>>,
    /// Agent names spawned during execution (for navigation on success).
    spawned_agents: Vec<String>,
}

impl EventEmitter<DismissEvent> for PlanDispatchModal {}
impl ModalView for PlanDispatchModal {
    fn fade_out_background(&self) -> bool {
        true
    }
}

impl Focusable for PlanDispatchModal {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.intent_editor.focus_handle(cx)
    }
}

impl PlanDispatchModal {
    pub fn new(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let intent_editor = cx.new(|cx| {
            let mut editor = Editor::multi_line(window, cx);
            editor.set_placeholder_text(
                "Describe what needs to be done.\nOne line per work package.",
                window,
                cx,
            );
            editor
        });
        Self {
            step: ModalStep::InputIntent,
            intent_editor,
            error: None,
            workspace,
            exec_task: None,
            spawned_agents: Vec::new(),
        }
    }

    pub fn open(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
        let workspace_weak = cx.weak_entity();
        workspace.toggle_modal(window, cx, move |window, cx| {
            Self::new(workspace_weak, window, cx)
        });
    }

    fn advance_to_review(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let text = self.intent_editor.read(cx).text(cx);
        let drafts = decompose_intent(&text);
        if drafts.is_empty() {
            self.error =
                Some("Enter at least one work package (one line per package).".to_string());
            cx.notify();
            return;
        }
        self.error = None;
        self.step = ModalStep::ReviewPlan { drafts };
        cx.notify();
    }

    fn back_to_input(&mut self, cx: &mut Context<Self>) {
        self.step = ModalStep::InputIntent;
        cx.notify();
    }

    fn approve_and_execute(&mut self, cx: &mut Context<Self>) {
        let drafts = match &self.step {
            ModalStep::ReviewPlan { drafts } => drafts.clone(),
            _ => return,
        };

        self.step = ModalStep::Executing;
        self.error = None;
        cx.notify();

        self.exec_task = Some(cx.spawn(async move |this, cx| {
            let (handle, repo_root) = this
                .update(cx, |this, cx| {
                    let handle = cx
                        .try_global::<UglyhatService>()
                        .and_then(|svc| svc.handle());
                    let repo_root = this.workspace.upgrade().and_then(|ws| {
                        ws.read(cx)
                            .project()
                            .read(cx)
                            .visible_worktrees(cx)
                            .next()
                            .map(|wt| wt.read(cx).abs_path().to_path_buf())
                    });
                    (handle, repo_root)
                })
                .unwrap_or((None, None));

            // Verify the entity is still alive before doing background work.
            if this.update(cx, |_, _| ()).is_err() {
                return;
            }
            let drafts_bg = drafts.clone();

            let result: anyhow::Result<Vec<(String, String)>> = cx
                .background_spawn(async move {
                    let handle = handle.ok_or_else(|| anyhow::anyhow!("uglyhat not available"))?;

                    // 1. Create the plan
                    // Combine all intents into one overview sentence
                    let overview = drafts_bg
                        .iter()
                        .map(|d| d.intent.as_str())
                        .collect::<Vec<_>>()
                        .join("; ");
                    let plan = handle.create_plan(&overview)?;

                    // 2. Create WPs and remember their UUIDs in ordinal order
                    let mut wp_ids: Vec<uuid::Uuid> = Vec::new();
                    for draft in &drafts_bg {
                        let depends_on = draft
                            .depends_on_ordinal
                            .and_then(|ord| wp_ids.get(ord as usize).copied())
                            .into_iter()
                            .collect::<Vec<_>>();
                        let wp = handle.create_work_package(
                            Some(plan.id),
                            &draft.intent,
                            vec![],
                            draft.ordinal,
                            depends_on,
                            vec![],
                        )?;
                        wp_ids.push(wp.id);
                    }

                    // 3. Approve + activate the plan
                    handle.update_plan_status(plan.id, uglyhat::model::PlanStatus::Approved)?;
                    handle.update_plan_status(plan.id, uglyhat::model::PlanStatus::Active)?;

                    // 4. Refresh readiness — dep-free WPs flip to Ready
                    let ready_wps = handle.refresh_work_package_readiness(plan.id)?;

                    // Return list of (intent, thread_name) for agent spawning
                    let spawn_targets: Vec<(String, String)> = ready_wps
                        .iter()
                        .map(|wp| {
                            let slug = slugify(&wp.intent);
                            (wp.intent.clone(), format!("wp-{}-{}", wp.ordinal, slug))
                        })
                        .collect();

                    // 5. Create threads for ready WPs
                    for (_, thread_name) in &spawn_targets {
                        let _ = handle.create_thread(thread_name, "", vec![]);
                    }

                    anyhow::Ok(spawn_targets)
                })
                .await;

            match result {
                Ok(spawn_targets) => {
                    let mut agents = Vec::new();
                    if let Some(root) = repo_root {
                        for (intent, thread_name) in spawn_targets {
                            let millis = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis();
                            let agent_name = format!("agent-{millis}");
                            if spawn_agent_in_worktree(
                                intent,
                                thread_name,
                                agent_name.clone(),
                                root.clone(),
                                cx,
                            )
                            .await
                            .is_ok()
                            {
                                agents.push(agent_name);
                            }
                        }
                    }
                    this.update(cx, |this, cx| {
                        this.spawned_agents = agents;
                        cx.notify();
                    })
                    .ok();
                }
                Err(e) => {
                    this.update(cx, |this, cx| {
                        this.error = Some(e.to_string());
                        cx.notify();
                    })
                    .ok();
                }
            }
        }));
    }
}

impl Render for PlanDispatchModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Navigate to first spawned agent and dismiss
        if let Some(agent_name) = self.spawned_agents.first().cloned() {
            self.spawned_agents.clear();
            let workspace = self.workspace.clone();
            cx.spawn_in(window, async move |this, cx| {
                if let Some(ws) = workspace.upgrade() {
                    ws.update_in(cx, |workspace, window, cx| {
                        open_agent_view(workspace, agent_name, window, cx);
                    })
                    .ok();
                }
                this.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
            })
            .detach();
        }

        match &self.step {
            ModalStep::InputIntent => self.render_input(window, cx).into_any_element(),
            ModalStep::ReviewPlan { drafts } => {
                let drafts = drafts.clone();
                self.render_review(drafts, cx).into_any_element()
            }
            ModalStep::Executing => self.render_executing(cx).into_any_element(),
        }
    }
}

impl PlanDispatchModal {
    fn render_input(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.error.clone();
        v_flex()
            .elevation_3(cx)
            .key_context("PlanDispatchModal")
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .on_action(
                cx.listener(|this, _: &menu::Confirm, window, cx| {
                    this.advance_to_review(window, cx)
                }),
            )
            .w(px(560.))
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .child(Label::new("New Plan").size(LabelSize::Small))
                    .child(
                        Label::new("Enter → Review")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(
                v_flex()
                    .gap_0p5()
                    .child(
                        Label::new("What needs to be done? (one work package per line)")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_2()
                            .min_h(px(120.))
                            .border_1()
                            .border_color(cx.theme().colors().border_focused)
                            .rounded_md()
                            .bg(cx.theme().colors().editor_background)
                            .child(self.intent_editor.clone()),
                    ),
            )
            .when_some(error, |this, err| {
                this.child(Label::new(err).size(LabelSize::XSmall).color(Color::Error))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("review", "Review Plan →")
                            .style(ButtonStyle::Filled)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.advance_to_review(window, cx)
                            })),
                    )
                    .child(
                        Button::new("cancel", "Cancel")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    ),
            )
    }

    fn render_review(
        &mut self,
        drafts: Vec<WorkPackageDraft>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let error = self.error.clone();
        v_flex()
            .elevation_3(cx)
            .key_context("PlanDispatchModal")
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .on_action(cx.listener(|this, _: &menu::Confirm, _, cx| this.approve_and_execute(cx)))
            .w(px(560.))
            .p_4()
            .gap_3()
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        Label::new(format!("Review Plan ({} packages)", drafts.len()))
                            .size(LabelSize::Small),
                    )
                    .child(
                        Label::new("approve to execute")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
            .child(
                v_flex()
                    .gap_1()
                    .max_h(px(320.))
                    .children(drafts.iter().enumerate().map(|(ix, draft)| {
                        let dep_label = draft
                            .depends_on_ordinal
                            .map(|d| format!("after #{}", d + 1));
                        h_flex()
                            .id(("wp-draft", ix))
                            .w_full()
                            .px_2()
                            .py_1()
                            .gap_2()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .rounded_md()
                            .child(
                                Label::new(format!("#{}", ix + 1))
                                    .size(LabelSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .child(Label::new(draft.intent.clone()).size(LabelSize::Small)),
                            )
                            .when_some(dep_label, |this, dep| {
                                this.child(
                                    Label::new(dep).size(LabelSize::XSmall).color(Color::Muted),
                                )
                            })
                    })),
            )
            .when_some(error, |this, err| {
                this.child(Label::new(err).size(LabelSize::XSmall).color(Color::Error))
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("approve", "Approve & Execute")
                            .style(ButtonStyle::Filled)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.approve_and_execute(cx))),
                    )
                    .child(
                        Button::new("back", "← Back")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| this.back_to_input(cx))),
                    )
                    .child(
                        Button::new("cancel", "Cancel")
                            .style(ButtonStyle::Subtle)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    ),
            )
    }

    fn render_executing(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.error.clone();
        v_flex()
            .elevation_3(cx)
            .key_context("PlanDispatchModal")
            .on_action(cx.listener(|_, _: &menu::Cancel, _, cx| cx.emit(DismissEvent)))
            .w(px(560.))
            .p_4()
            .gap_3()
            .child(Label::new("Executing Plan…").size(LabelSize::Small))
            .child(
                Label::new("Creating work packages, spawning agents…")
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .when_some(error, |this, err| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(Label::new(err).size(LabelSize::XSmall).color(Color::Error))
                        .child(
                            Button::new("close", "Close")
                                .style(ButtonStyle::Subtle)
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                        ),
                )
            })
    }
}
