use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    App, AppContext, Bounds, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render,
    SharedString, Styled, Task, WeakEntity, Window, actions, canvas,
    fill, px, rgba,
};
use ui::{Icon, IconName};
use workspace::Workspace;
use workspace::item::{Item, ItemEvent};

use super::agent_bridge::AgentBridge;
use super::office_state::OfficeState;
use super::renderer::{self, CharSnapshot};
use super::sprites::SpriteAtlas;
use crate::activity_bus;
use crate::hq_state::HqState;

actions!(prism_hq, [OpenAgentOffice]);

pub struct AgentOfficeItem {
    focus_handle: FocusHandle,
    atlas: Option<Arc<SpriteAtlas>>,
    state: OfficeState,
    bridge: AgentBridge,
    local_agent_name: String,
    /// id of the selected character (consistent type with hovered_char_id).
    selected_char_id: Option<usize>,
    hovered_char_id: Option<usize>,
    canvas_bounds: Rc<Cell<Bounds<Pixels>>>,
    _hq_sub: Option<gpui::Subscription>,
    _activity_sub: Option<gpui::Subscription>,
    _load_task: Option<Task<()>>,
    _game_loop: Task<()>,
}

impl AgentOfficeItem {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        let local_agent_name = std::env::var("PRISM_AGENT_NAME")
            .or_else(|_| std::env::var("UH_AGENT_NAME"))
            .unwrap_or_else(|_| "claude".to_string());

        // Load sprites in background.
        let load_task = cx.spawn(async move |this: WeakEntity<AgentOfficeItem>, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { SpriteAtlas::load() })
                .await;
            this.update(cx, |item, cx| match result {
                Ok(atlas) => {
                    item.atlas = Some(Arc::new(atlas));
                    cx.notify();
                }
                Err(e) => log::warn!("agent_office: failed to load sprites: {e}"),
            })
            .ok();
        });

        // Inline game loop (typed to AgentOfficeItem, stored to cancel on drop).
        let game_loop = {
            let weak = cx.weak_entity();
            cx.spawn(async move |_this, cx| {
                let mut last = Instant::now();
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(16))
                        .await;
                    let now = Instant::now();
                    let dt = (now - last).as_secs_f32().min(0.1);
                    last = now;
                    let keep_going = weak
                        .update(cx, |item, cx| {
                            item.state.tick(dt);
                            cx.notify();
                        })
                        .is_ok();
                    if !keep_going {
                        break;
                    }
                }
            })
        };

        // Subscribe to HqState.
        let hq_sub = HqState::global(cx).map(|hq_entity| {
            cx.observe(&hq_entity, |item, hq, cx| {
                let agents = hq.read(cx).agents.clone();
                let activity_snap = activity_bus::global_inner(cx).map(|e| e.read(cx).clone());
                let mutations = item.bridge.sync(
                    &agents,
                    activity_snap.as_ref(),
                    Some(item.local_agent_name.as_str()),
                );
                item.state.apply_mutations(mutations);
                cx.notify();
            })
        });

        // Subscribe to ActivityBus.
        let activity_sub = activity_bus::global_inner(cx).map(|bus_entity| {
            cx.observe(&bus_entity, |item, bus, cx| {
                let bus_snap = bus.read(cx).clone();
                let agents = HqState::global(cx)
                    .map(|hq| hq.read(cx).agents.clone())
                    .unwrap_or_default();
                let mutations = item.bridge.sync(
                    &agents,
                    Some(&bus_snap),
                    Some(item.local_agent_name.as_str()),
                );
                item.state.apply_mutations(mutations);
                cx.notify();
            })
        });

        Self {
            focus_handle,
            atlas: None,
            state: OfficeState::from_layout(),
            bridge: AgentBridge::new(),
            local_agent_name,
            selected_char_id: None,
            hovered_char_id: None,
            canvas_bounds: Rc::new(Cell::new(Bounds::default())),
            _hq_sub: hq_sub,
            _activity_sub: activity_sub,
            _load_task: Some(load_task),
            _game_loop: game_loop,
        }
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.canvas_bounds.get();
        let id = renderer::hit_test_character(event.position, &self.state, bounds.origin);
        if self.hovered_char_id != id {
            self.hovered_char_id = id;
            cx.notify();
        }
    }

    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.canvas_bounds.get();
        let Some(char_id) =
            renderer::hit_test_character(event.position, &self.state, bounds.origin)
        else {
            self.selected_char_id = None;
            cx.notify();
            return;
        };

        self.selected_char_id = Some(char_id);
        cx.notify();

        let Some(ch) = self.state.characters.iter().find(|c| c.id == char_id) else {
            return;
        };
        let agent_name = ch.name.clone();

        // Look up agent's current thread from HqState.
        let thread_name = HqState::global(cx).and_then(|hq| {
            hq.read(cx)
                .agents
                .iter()
                .find(|a| a.name == agent_name)
                .and_then(|a| a.current_thread.clone())
        });

        if let Some(ref thread) = thread_name {
            log::info!("agent_office: selected agent '{agent_name}' on thread '{thread}'");
        } else {
            log::info!("agent_office: selected agent '{agent_name}' (no active thread)");
        }

        // TODO: dispatch OpenAgentChatSession once ContextService thread-name→UUID resolution
        // is wired up. Currently we log the selection and focus is marked via selected_char_id.
        let _ = thread_name;
    }
}

impl EventEmitter<ItemEvent> for AgentOfficeItem {}

impl Focusable for AgentOfficeItem {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AgentOfficeItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let atlas = self.atlas.clone();
        let selected_id = self.selected_char_id;
        let hovered_id = self.hovered_char_id;

        // Build per-character snapshots. The id is the first field of CharSnapshot,
        // so it doubles as the highlight key — no separate wrapper Vec needed.
        let mut char_snapshots: Vec<CharSnapshot> = self
            .state
            .characters
            .iter()
            .map(|ch| {
                (ch.id, ch.palette, ch.tile_x, ch.tile_y, ch.direction, ch.sprite_col(), ch.bubble)
            })
            .collect();
        // Sort back-to-front by tile_y for z-ordering.
        char_snapshots
            .sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));

        // Snapshot layout data for the render closure.
        let layout_data = self.state.layout_render_data();
        let canvas_bounds_cell = self.canvas_bounds.clone();

        gpui::div()
            .size_full()
            .track_focus(&self.focus_handle(cx))
            .key_context("AgentOffice")
            .child(
                gpui::div()
                    .id("agent-office-canvas")
                    .size_full()
                    .on_mouse_move(cx.listener(Self::handle_mouse_move))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::handle_mouse_down))
                    .child(
                        canvas(
                            move |bounds, _window, _cx| {
                                canvas_bounds_cell.set(bounds);
                            },
                            move |bounds, (), window, _cx| {
                                let Some(ref atlas) = atlas else { return };

                                renderer::render_frame(
                                    bounds, &char_snapshots, &layout_data, atlas, window,
                                );

                                // Hover and selection highlights drawn on top.
                                let scale = 2.0_f32;
                                let fw = px(16.0 * scale);
                                let fh = px(32.0 * scale);
                                for &(id, _, tile_x, tile_y, _, _, _) in &char_snapshots {
                                    let sx = bounds.origin.x + px(tile_x * 32.0) - fw / 2.0;
                                    let sy = bounds.origin.y + px(tile_y * 32.0) - fh / 2.0;
                                    let char_bounds = gpui::Bounds::new(
                                        gpui::point(sx, sy),
                                        gpui::size(fw, fh),
                                    );
                                    if selected_id == Some(id) {
                                        window.paint_quad(fill(char_bounds, rgba(0xffffff33)));
                                    } else if hovered_id == Some(id) {
                                        window.paint_quad(fill(char_bounds, rgba(0xffffff1a)));
                                    }
                                }
                            },
                        )
                        .size_full(),
                    ),
            )
    }
}

impl Item for AgentOfficeItem {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Agent Office".into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<Icon> {
        Some(Icon::new(IconName::Person))
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(ItemEvent)) {
        f(*event)
    }
}

/// Open the Agent Office as a singleton tab in the center editor area.
pub fn open_agent_office(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) {
    let existing = workspace
        .active_pane()
        .read(cx)
        .items()
        .find_map(|item| item.downcast::<AgentOfficeItem>());
    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let item = cx.new(|cx| AgentOfficeItem::new(window, cx));
        workspace.add_item_to_center(Box::new(item), window, cx);
    }
}
