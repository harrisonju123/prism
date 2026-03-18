use std::sync::Arc;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, IntoElement,
    ParentElement, Pixels, Render, Styled, Task, WeakEntity, Window, actions, canvas, fill, px,
    rgb,
};
use ui::IconName;
use workspace::dock::{DockPosition, Panel, PanelEvent};

use super::agent_bridge::AgentBridge;
use super::game_loop::GameLoop;
use super::office_state::OfficeState;
use super::sprites::SpriteAtlas;
use crate::activity_bus;
use crate::hq_state::HqState;

/// Read the local agent name from `PRISM_AGENT_NAME` (or `UH_AGENT_NAME` fallback).
fn local_agent_name() -> String {
    std::env::var("PRISM_AGENT_NAME")
        .or_else(|_| std::env::var("UH_AGENT_NAME"))
        .unwrap_or_else(|_| "claude".to_string())
}

actions!(prism_hq, [TogglePixelOffice]);

const PIXEL_OFFICE_PANEL_KEY: &str = "pixel_office";

pub struct PixelOfficePanel {
    focus_handle: FocusHandle,
    position: DockPosition,
    width: Option<Pixels>,

    /// Loaded sprite atlas — populated asynchronously after panel creation.
    atlas: Option<Arc<SpriteAtlas>>,

    /// Game / simulation state.
    state: OfficeState,

    /// Agent → character reconciler.
    bridge: AgentBridge,

    /// Subscriptions to HqState and ActivityBus.
    _hq_sub: Option<gpui::Subscription>,
    _activity_sub: Option<gpui::Subscription>,

    _load_task: Option<Task<()>>,
}

impl EventEmitter<PanelEvent> for PixelOfficePanel {}

impl PixelOfficePanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();

        // Spawn background task to decode sprites without blocking the UI thread.
        let load_task = cx.spawn(async move |this: WeakEntity<PixelOfficePanel>, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { SpriteAtlas::load() })
                .await;
            this.update(cx, |panel, cx| match result {
                Ok(atlas) => {
                    panel.atlas = Some(Arc::new(atlas));
                    cx.notify();
                }
                Err(e) => log::warn!("pixel_office: failed to load sprites: {e}"),
            })
            .ok();
        });

        // Start the 60 Hz game loop.
        let weak = cx.weak_entity();
        GameLoop::start(weak, cx);

        // Subscribe to HqState to sync agent roster.
        let hq_sub = HqState::global(cx).map(|hq_entity| {
            cx.observe(&hq_entity, |panel, hq, cx| {
                let agents = hq.read(cx).agents.clone();
                let activity_snap = activity_bus::global_inner(cx)
                    .map(|e| e.read(cx).clone());
                let local_name = local_agent_name();
                let mutations = panel.bridge.sync(
                    &agents,
                    activity_snap.as_ref(),
                    Some(local_name.as_str()),
                );
                let agent_map = panel.bridge.agent_to_char.clone();
                panel.state.apply_mutations(mutations, &agent_map);
                cx.notify();
            })
        });

        // Subscribe to ActivityBus for local agent state updates.
        let activity_sub = activity_bus::global_inner(cx).map(|bus_entity| {
            cx.observe(&bus_entity, |panel, bus, cx| {
                let bus_snap = bus.read(cx).clone();
                let agents = HqState::global(cx)
                    .map(|hq| hq.read(cx).agents.clone())
                    .unwrap_or_default();
                let local_name = local_agent_name();
                let mutations = panel.bridge.sync(
                    &agents,
                    Some(&bus_snap),
                    Some(local_name.as_str()),
                );
                let agent_map = panel.bridge.agent_to_char.clone();
                panel.state.apply_mutations(mutations, &agent_map);
                cx.notify();
            })
        });

        Self {
            focus_handle,
            position: DockPosition::Right,
            width: None,
            atlas: None,
            state: OfficeState::demo(),
            bridge: AgentBridge::new(),
            _hq_sub: hq_sub,
            _activity_sub: activity_sub,
            _load_task: Some(load_task),
        }
    }

    /// Called every ~16 ms by the game loop.
    pub fn tick(&mut self, dt: f32, cx: &mut Context<Self>) {
        self.state.tick(dt);
        cx.notify();
    }
}

impl Focusable for PixelOfficePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PixelOfficePanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let atlas = self.atlas.clone();
        // Snapshot the data needed for the canvas closure (can't borrow self inside).
        let characters: Vec<_> = self
            .state
            .characters
            .iter()
            .map(|ch| {
                (
                    ch.palette,
                    ch.tile_x,
                    ch.tile_y,
                    ch.direction,
                    ch.state,
                    ch.frame_index,
                    ch.bubble,
                )
            })
            .collect();
        let walkable = self.state.walkable_tiles.clone();

        gpui::div().size_full().child(
            canvas(
                |_bounds, _window, _cx| (),
                {
                    let atlas = atlas.clone();
                    let characters = characters.clone();
                    let walkable = walkable.clone();
                    move |bounds, (), window, _cx| {
                        // Dark navy background.
                        window.paint_quad(fill(bounds, rgb(0x1a1a2e)));

                        let Some(ref atlas) = atlas else { return };

                        // Paint floor tiles.
                        if !atlas.floors.is_empty() {
                            let floor_tile = &atlas.floors[1];
                            for &(tx, ty) in &walkable {
                                let sx = bounds.origin.x + px(tx as f32 * 32.0);
                                let sy = bounds.origin.y + px(ty as f32 * 32.0);
                                let _ = window.paint_image(
                                    gpui::Bounds::new(
                                        gpui::point(sx, sy),
                                        gpui::size(px(32.0), px(32.0)),
                                    ),
                                    gpui::Corners::default(),
                                    floor_tile.clone(),
                                    0,
                                    false,
                                );
                            }
                        }

                        // Sort characters back-to-front by tile_y (index 2 in tuple).
                        let mut sorted = characters.clone();
                        sorted.sort_by(|a, b| {
                            a.2.partial_cmp(&b.2)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });

                        for &(palette, tile_x, tile_y, direction, state, frame_index, bubble) in
                            &sorted
                        {
                            let pal = palette.min(atlas.characters.len().saturating_sub(1));
                            let sheet = &atlas.characters[pal];

                            let col = match state {
                                super::characters::CharState::Idle
                                | super::characters::CharState::Wait => {
                                    super::sprites::char_frames::IDLE
                                }
                                super::characters::CharState::Walk => {
                                    super::sprites::char_frames::WALK
                                        [frame_index % super::sprites::char_frames::WALK.len()]
                                }
                                super::characters::CharState::Type => {
                                    super::sprites::char_frames::TYPE_ANIM[frame_index
                                        % super::sprites::char_frames::TYPE_ANIM.len()]
                                }
                            };
                            let row = direction.sprite_row();

                            if col >= sheet.cols || row >= sheet.rows {
                                continue;
                            }

                            let frame = sheet.frame(col, row);
                            let scale = 2.0_f32;
                            let fw = px(sheet.frame_w as f32 * scale);
                            let fh = px(sheet.frame_h as f32 * scale);
                            let sx = bounds.origin.x + px(tile_x * 32.0) - fw / 2.0;
                            let sy = bounds.origin.y + px(tile_y * 32.0) - fh / 2.0;

                            let _ = window.paint_image(
                                gpui::Bounds::new(gpui::point(sx, sy), gpui::size(fw, fh)),
                                gpui::Corners::default(),
                                frame.clone(),
                                0,
                                false,
                            );

                            // Bubble indicator.
                            if let Some(kind) = bubble {
                                let bx = sx + fw / 2.0 - px(8.0);
                                let by = sy - px(14.0);
                                let color = match kind {
                                    super::characters::BubbleKind::Permission => {
                                        gpui::rgb(0xffd700)
                                    }
                                    super::characters::BubbleKind::Waiting => gpui::rgb(0x00bfff),
                                };
                                window.paint_quad(fill(
                                    gpui::Bounds::new(
                                        gpui::point(bx, by),
                                        gpui::size(px(16.0), px(13.0)),
                                    ),
                                    color,
                                ));
                            }
                        }
                    }
                },
            )
            .size_full(),
        )
    }
}

impl Panel for PixelOfficePanel {
    fn persistent_name() -> &'static str {
        "PixelOfficePanel"
    }

    fn panel_key() -> &'static str {
        PIXEL_OFFICE_PANEL_KEY
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(
            position,
            DockPosition::Left | DockPosition::Right | DockPosition::Bottom
        )
    }

    fn set_position(
        &mut self,
        position: DockPosition,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.position = position;
        cx.notify();
    }

    fn size(&self, _window: &Window, _cx: &App) -> Pixels {
        self.width.unwrap_or(px(400.0))
    }

    fn set_size(
        &mut self,
        size: Option<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.width = size;
        cx.notify();
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<IconName> {
        Some(IconName::Person)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Pixel Office")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(TogglePixelOffice)
    }

    fn activation_priority(&self) -> u32 {
        10
    }
}
