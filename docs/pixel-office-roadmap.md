# Pixel Office — Roadmap

Bring the [pixel-agents](https://github.com/pablodelucca/pixel-agents) virtual office experience into
the PrisM IDE as a native GPUI panel in `crates/prism-hq`.

> **Reference**: pixel-agents is MIT-licensed. We can reuse its sprite assets directly and port its
> algorithms to Rust. No API integration needed — we replace its JSONL file-watcher with our existing
> `ActivityBus` + `HqState` globals.

---

## Architecture Overview

```
pixel-agents (VS Code)                     PrisM Pixel Office (GPUI)
──────────────────────────────────         ──────────────────────────────────
JSONL file watcher                    →    ActivityBus + HqState subscriptions
Extension ↔ Webview postMessage       →    In-process GPUI entity update
React + Canvas 2D (webview)           →    GPUI canvas() element + paint_image()
requestAnimationFrame loop            →    cx.request_animation_frame() loop
VS Code workspace storage             →    .prism/pixel_office_layout.json
```

### New module: `crates/prism-hq/src/pixel_office/`

```
pixel_office/
├── mod.rs              # Module root, pub re-exports
├── panel.rs            # PixelOfficePanel — GPUI dock panel
├── game_loop.rs        # Delta-time animation ticker
├── office_state.rs     # Authoritative game state (characters, layout, seats)
├── characters.rs       # Character FSM: IDLE / WALK / TYPE / WAIT
├── pathfinding.rs      # BFS on walkable tile grid
├── renderer.rs         # GPUI canvas() rendering — z-sort, sprites, bubbles
├── sprites.rs          # Sprite sheet loading, frame cache (ImageId per frame)
├── layout.rs           # Room grid, furniture catalog, wall auto-tiling
├── layout_persistence.rs # Save/load .prism/pixel_office_layout.json
└── agent_bridge.rs     # Map HqState + ActivityBus → OfficeState mutations
```

### Sprite assets: `assets/pixel_office/`

```
assets/pixel_office/
├── characters/         # character_1.png … character_6.png (MIT from pixel-agents)
├── floors/             # floor_1.png … floor_7.png
├── walls/              # wall_1.png … wall_N.png
└── furniture/          # desk.png, chair.png, plant.png, … + manifest.json
```

---

## Phase 1 — Sprite Foundation

**Goal**: Load PNG sprite sheets, extract individual frames into GPUI `ImageData`, render a
single static character sprite in a standalone panel. No animation, no game loop yet.

### 1.1 Asset pipeline

**Files**: `pixel_office/sprites.rs`, `assets/pixel_office/`

- Copy sprite PNGs from pixel-agents repo into `assets/pixel_office/` (MIT license ✓)
- Add `image` crate to `prism-hq/Cargo.toml` for PNG decoding (`image = { version = "0.25", features = ["png"] }`)
- `SpriteSheet` struct: loads a PNG at startup, slices it into a `Vec<Arc<gpui::ImageData>>` by frame grid

```rust
pub struct SpriteSheet {
    frames: Vec<Arc<gpui::ImageData>>,  // pre-sliced, registered with GPUI
    frame_w: u32,
    frame_h: u32,
    cols: u32,
    rows: u32,
}

impl SpriteSheet {
    /// Load from embedded bytes (include_bytes! at compile time)
    pub fn from_bytes(bytes: &[u8], frame_w: u32, frame_h: u32) -> anyhow::Result<Self>

    /// Returns the ImageData for a given (col, row) frame
    pub fn frame(&self, col: usize, row: usize) -> Arc<gpui::ImageData>
}
```

- `CharacterSprites` struct: wraps 6 `SpriteSheet`s (one per palette), provides
  `get(palette, direction, state, frame_index) -> Arc<gpui::ImageData>`
- `FloorSprites`, `WallSprites`, `FurnitureSprites` — same pattern
- All loaded once at panel init, stored on `PixelOfficePanel`

**Sprite sheet constants** (from pixel-agents source):
```
Character: 32×32 px per frame, 7 cols × 3 rows
Floor tile: 16×16 px, single tile
Wall tile:  16×16 px, 16 variants in a strip (bitmask 0–15)
Tile size:  16 px
```

### 1.2 Minimal panel shell

**Files**: `pixel_office/panel.rs`, `pixel_office/mod.rs`, `prism_hq.rs`

```rust
pub struct PixelOfficePanel {
    focus_handle: FocusHandle,
    sprites: Option<Arc<SpriteAtlas>>,   // None until loaded
    _load_task: Task<()>,
}

impl Render for PixelOfficePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(gpui::rgb(0x1a1a2e))   // dark navy background
            .child(
                canvas(move |bounds, window, cx| {
                    // Phase 1: just paint one character sprite at center
                    if let Some(frame) = self.sprites.as_ref().map(|s| s.character.frame(0,0,0)) {
                        window.paint_image(
                            Bounds::new(bounds.center() - point(px(16.), px(16.)), size(px(32.), px(32.))),
                            Corners::default(),
                            frame.clone(),
                            false,
                        ).ok();
                    }
                })
                .size_full()
            )
    }
}
```

- Register in `prism_hq.rs`:
  ```rust
  workspace.register_panel::<PixelOfficePanel>(window, cx);
  ```
- Add menu action `OpenPixelOffice` to toggle the panel

**Definition of done**: Panel opens from View menu, shows dark background with one static character
sprite. `cargo check -p prism-hq` clean.

---

## Phase 2 — Game Loop + Character Animation

**Goal**: Characters animate (IDLE bob, WALK cycle, TYPE flicker) driven by a real delta-time loop.
No agent data yet — all characters are in hardcoded demo positions.

### 2.1 Delta-time ticker

**File**: `pixel_office/game_loop.rs`

GPUI provides `cx.request_animation_frame(callback)`. Build a recurring ticker:

```rust
pub struct GameTicker;

impl GameTicker {
    /// Call this once; it perpetually re-schedules itself.
    /// Calls `update(dt_secs)` on the panel each frame.
    pub fn start(entity: WeakEntity<PixelOfficePanel>, cx: &mut App) {
        cx.spawn(async move |cx| {
            let mut last = std::time::Instant::now();
            loop {
                // Yield until next animation frame
                cx.background_executor().timer(Duration::from_millis(16)).await;
                let now = std::time::Instant::now();
                let dt = (now - last).as_secs_f32().min(0.1);  // cap at 100ms
                last = now;
                entity.update(cx, |panel, cx| {
                    panel.tick(dt, cx);
                }).ok();
            }
        }).detach();
    }
}
```

> 16ms polling ≈ 60fps. A true `request_animation_frame` hook can replace this later if GPUI exposes
> one on the panel level; the interface stays the same.

### 2.2 Character FSM

**File**: `pixel_office/characters.rs`

Port directly from `characters.ts`:

```rust
#[derive(Clone, Copy, PartialEq)]
pub enum CharState { Idle, Walk, Type, Wait }

#[derive(Clone, Copy, PartialEq)]
pub enum Direction { Down = 0, Left = 1, Right = 2, Up = 3 }

pub struct Character {
    pub id: usize,
    pub palette: usize,         // 0–5 → which sprite sheet
    pub tile_x: f32,            // sub-pixel position (interpolated)
    pub tile_y: f32,
    pub direction: Direction,
    pub state: CharState,
    pub anim_timer: f32,
    pub frame_index: usize,
    pub path: Vec<(i32, i32)>,  // BFS waypoints remaining
    pub seat: Option<(i32, i32, Direction)>,
    pub wander_timer: f32,
    pub bubble_timer: f32,      // > 0 → show permission/wait bubble
    pub name: String,
    pub status_text: Option<String>,
}

// Animation constants
const WALK_SPEED: f32 = 3.0;         // tiles/sec (= 48px/sec at 16px/tile)
const WALK_FRAME_DUR: f32 = 0.15;    // sec per walk frame
const TYPE_FRAME_DUR: f32 = 0.30;    // sec per type frame
const WANDER_MIN: f32 = 2.0;
const WANDER_MAX: f32 = 20.0;

impl Character {
    pub fn tick(&mut self, dt: f32, layout: &OfficeLayout, rng: &mut impl Rng) { ... }
}
```

**Frame assignments** (from pixel-agents spriteData.ts):
```
IDLE:  row = direction, col = 0 (static)
WALK:  row = direction, cols = 1,2,3,4 (cycle)
TYPE:  row = direction, cols = 5,6     (cycle)
WAIT:  row = direction, col = 0 + bubble overlay
```

### 2.3 Renderer

**File**: `pixel_office/renderer.rs`

```rust
pub fn render_frame(
    bounds: Bounds<Pixels>,
    state: &OfficeState,
    atlas: &SpriteAtlas,
    camera: Camera,
    window: &mut Window,
) {
    // 1. Fill background
    window.paint_quad(PaintQuad { bounds, background: rgb(0x1a1a2e), .. });

    // 2. Render floor tiles
    for (tile_x, tile_y, tile_type) in state.layout.tiles() {
        let screen = tile_to_screen(tile_x, tile_y, camera);
        let frame = atlas.floor.frame(tile_type.floor_variant());
        window.paint_image(tile_bounds(screen), frame).ok();
    }

    // 3. Collect drawable items: furniture + characters
    let mut drawables: Vec<Drawable> = vec![];
    for character in &state.characters {
        drawables.push(Drawable::Character(character));
    }
    for furniture in &state.layout.furniture {
        drawables.push(Drawable::Furniture(furniture));
    }

    // 4. Z-sort by tile_y (back-to-front painter's algorithm)
    drawables.sort_by(|a, b| a.z_key().partial_cmp(&b.z_key()).unwrap());

    // 5. Render each
    for drawable in drawables {
        match drawable {
            Drawable::Character(ch) => render_character(ch, atlas, camera, window),
            Drawable::Furniture(f)  => render_furniture(f, atlas, camera, window),
        }
    }

    // 6. Render speech bubbles on top
    for ch in state.characters.iter().filter(|c| c.bubble_timer > 0.0) {
        render_bubble(ch, atlas, camera, window);
    }

    // 7. Render name labels
    for ch in &state.characters {
        render_name_label(ch, camera, window);
    }
}
```

**Name labels**: Use GPUI `TextRun` / `ShapedLine` painting for crisp text at small sizes.

### 2.4 Pathfinding

**File**: `pixel_office/pathfinding.rs`

Port BFS from `tileMap.ts`:

```rust
pub fn find_path(
    from: (i32, i32),
    to: (i32, i32),
    walkable: &HashSet<(i32, i32)>,
) -> Vec<(i32, i32)> {
    // Standard BFS, returns Vec of tiles from `from` (exclusive) to `to` (inclusive)
}
```

**Definition of done**: 3 demo characters on screen, walking in idle wander loops, TYPE animation
plays when manually set, WALK animation moves between tiles smoothly. `cargo check` clean.

---

## Phase 3 — Agent Bridge

**Goal**: Characters represent real agents. State (IDLE/TYPE/WAIT) driven by live
`ActivityBus` + `HqState` data. Characters appear/disappear as agents start/stop.

### 3.1 Agent bridge

**File**: `pixel_office/agent_bridge.rs`

```rust
pub struct AgentBridge {
    /// Map from agent name → character id
    agent_to_char: HashMap<String, usize>,
    next_char_id: usize,
    palette_pool: Vec<usize>,  // [0,1,2,3,4,5] round-robin
}

impl AgentBridge {
    /// Called each HqState update. Returns list of mutations to apply to OfficeState.
    pub fn sync(
        &mut self,
        agents: &[AgentStatus],
        activity: &AgentActivityBusInner,
    ) -> Vec<OfficeMutation> { ... }
}

pub enum OfficeMutation {
    SpawnCharacter { id: usize, palette: usize, name: String, seat: Option<(i32,i32,Direction)> },
    DespawnCharacter { id: usize },
    SetState { id: usize, state: CharState, status_text: Option<String> },
    ShowBubble { id: usize, bubble: BubbleKind },
    ClearBubble { id: usize },
}
```

**Mapping rules** (adapted from pixel-agents `transcriptParser.ts`):

| Agent state | Character state | Bubble |
|---|---|---|
| `AgentState::Working` + tool active | `Type` | none |
| `AgentState::Working` + no tool | `Idle` | none |
| `AgentState::AwaitingReview` | `Idle` | `Wait` bubble |
| `waiting_for_approval` in ActivityBus | `Type` | `Permission` bubble |
| `AgentState::Idle` | `Idle` (wanders) | none |
| Agent removed from HqState | Despawn (matrix effect) | — |

**Status text** (shown in tooltip / label):
- Map `current_tool` from ActivityBus to readable strings:
  ```
  "read_file"  → "Reading {filename}"
  "edit_file"  → "Editing {filename}"
  "bash"       → "Running: {truncated_cmd}"
  "grep"       → "Searching…"
  "spawn_agent"→ "Spawning subagent…"
  ```

### 3.2 Seat assignment

When a character spawns, assign them the nearest unoccupied seat in the room layout. Seats are
stored as `(tile_x, tile_y, Direction)` in the layout JSON. Characters walk to their seat when
transitioning to Type/Wait state.

### 3.3 Subscribe panel to live data

In `PixelOfficePanel::new()`:

```rust
// Subscribe to HqState (agent roster, states)
let hq_sub = HqState::global(cx).map(|hq| {
    cx.observe(&hq, |this, hq, cx| {
        let agents = hq.read(cx).agents.clone();
        this.state.apply_mutations(
            this.bridge.sync(&agents, &this.last_activity)
        );
        cx.notify();
    })
});

// Subscribe to ActivityBus (live tool/file events)
let activity_sub = cx.try_global::<ActivityBusGlobal>().map(|bus| {
    cx.observe(&bus.0, |this, bus, cx| {
        let activity = bus.read(cx).inner.clone();
        this.last_activity = activity.clone();
        let mutations = this.bridge.sync(&this.last_agents, &activity);
        this.state.apply_mutations(mutations);
        cx.notify();
    })
});
```

**Definition of done**: When a new Claude agent connects to PrisM, a pixel character spawns in the
office. When the agent is editing a file, the character animates TYPE. When it awaits review, the
Wait bubble appears. When the session ends, the character despawns with the matrix effect.

---

## Phase 4 — Room Layout System

**Goal**: Persistent room with floor tiles, wall auto-tiling, furniture, and an in-panel room editor.
Layout saved to `.prism/pixel_office_layout.json`.

### 4.1 Layout data model

**File**: `pixel_office/layout.rs`

```rust
pub struct OfficeLayout {
    pub cols: usize,           // default 20
    pub rows: usize,           // default 11
    pub tiles: Vec<TileKind>,  // flat: index = y * cols + x
    pub furniture: Vec<PlacedFurniture>,
    pub seats: Vec<Seat>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum TileKind { Void = 0, Wall = 1, Floor(u8) }  // Floor(1..=7)

pub struct PlacedFurniture {
    pub catalog_id: String,   // "desk", "chair", "plant", …
    pub x: i32,
    pub y: i32,
    pub rotation: u8,         // 0, 1, 2, 3 (×90°)
    pub state: u8,            // 0/1 for electronics
}

pub struct Seat {
    pub x: i32,
    pub y: i32,
    pub direction: Direction,
    pub occupied_by: Option<usize>,  // character id
}
```

**Default layout**: a pre-built 20×11 room with a handful of desks, chairs, and floor tiles.
Hardcoded as a `const` JSON string, used if no saved layout exists.

### 4.2 Wall auto-tiling

```rust
pub fn wall_sprite_index(x: i32, y: i32, layout: &OfficeLayout) -> usize {
    let n = is_wall(x, y - 1, layout) as usize;
    let e = is_wall(x + 1, y, layout) as usize;
    let s = is_wall(x, y + 1, layout) as usize;
    let w = is_wall(x - 1, y, layout) as usize;
    n | (e << 1) | (s << 2) | (w << 3)  // 0–15
}
```

### 4.3 Layout persistence

**File**: `pixel_office/layout_persistence.rs`

- Save: serialize to `.prism/pixel_office_layout.json` (debounced 500ms after any edit)
- Load: read on panel init; fall back to default layout if missing
- Format matches pixel-agents `OfficeLayout` v1 JSON exactly (future cross-tool compatibility)

### 4.4 Room editor mode

Toggle with a toolbar button (pencil icon). In edit mode:

- **Hover**: highlight hovered tile with ghost overlay
- **Left-click drag**: paint tiles with selected tile type
- **Right-click**: erase tile (set to Void)
- **Furniture placement**: select from catalog sidebar, click to place, R to rotate
- **Seat tool**: click floor tile to add/remove a seat
- Undo/redo stack (50 steps)

Implement as `EditorState` on `PixelOfficePanel`:

```rust
pub struct EditorState {
    pub active: bool,
    pub selected_tool: EditorTool,
    pub selected_tile: TileKind,
    pub selected_furniture_id: Option<String>,
    pub ghost_pos: Option<(i32, i32)>,
    pub undo_stack: VecDeque<OfficeLayout>,  // max 50
    pub redo_stack: Vec<OfficeLayout>,
}
```

Mouse events via GPUI `on_mouse_down`, `on_mouse_move`, `on_mouse_up` handlers on the canvas element.

**Definition of done**: Default room renders correctly with wall auto-tiling. User can paint floors,
place desks/chairs, toggle in/out of edit mode, and the layout persists across IDE restarts.

---

## Phase 5 — Sub-agents + Matrix Effect

**Goal**: When an agent calls `spawn_agent` / `escalate_decision`, a child character spawns linked
to the parent. Matrix (digital rain) effect plays on spawn and despawn.

### 5.1 Sub-agent tracking

`AgentBridge` already handles the agent roster from HqState. Extend it to track subagents:

- When `HqState.agents` contains a new agent with `parent_id` set → spawn a subagent character
- Sub-agent characters appear near their parent's seat
- Link rendered as a thin colored line from parent to sub-agent (painted before characters in z-order)
- When subagent session ends → matrix despawn on that character only

Subagent hierarchy depth mirrors the `MAX_SUBAGENT_DEPTH` limit from `thread.rs`. At depth > 2,
characters appear smaller (12×12 instead of 16×16 render scale) to indicate nesting depth.

### 5.2 Matrix spawn/despawn effect

**File**: `pixel_office/renderer.rs` (add `render_matrix_effect()`)

Port `matrixEffect.ts` to Rust. The effect works column-by-column on the character's sprite bounds:

```rust
pub fn render_matrix_effect(
    ch: &Character,
    atlas: &SpriteAtlas,
    progress: f32,    // 0.0 → 1.0
    mode: MatrixMode, // Spawn | Despawn
    camera: Camera,
    window: &mut Window,
) {
    let sprite = atlas.character.frame(ch.palette, ch.direction as usize, 0);
    // For each pixel column:
    //   compute head_y based on progress + column stagger
    //   pixels above head: transparent (despawn) or original (spawn)
    //   at head: bright green-white flash
    //   in trail: green tint fade
    //   below trail: original (despawn) or transparent (spawn)
    // Write modified ImageData row
}
```

`MatrixMode::Spawn`: columns reveal top-to-bottom. `MatrixMode::Despawn`: columns erase top-to-bottom.
Effect duration: 0.6 seconds. Store `spawn_timer: Option<f32>` and `despawn_timer: Option<f32>` on
`Character`.

### 5.3 Parent–child link rendering

In the renderer, before drawing characters, draw lines between parent and sub-agent characters:

```rust
for (parent_id, child_id) in state.subagent_links() {
    let p = screen_pos(parent);
    let c = screen_pos(child);
    // paint_quad a 1px wide line of Color::Accent at 40% opacity
}
```

**Definition of done**: Spawning a subagent from the `spawn_agent` tool shows a new character
materialize with the matrix effect, linked to its parent. On completion, it disappears with the
despawn effect.

---

## Phase 6 — Camera, Zoom & Polish

**Goal**: Camera follows the most active agent, user can zoom/pan, tooltips show agent details.

### 6.1 Camera system

```rust
pub struct Camera {
    pub x: f32,          // world pixels (center of view)
    pub y: f32,
    pub zoom: f32,       // 1.0–4.0
    pub target_x: f32,   // lerp target
    pub target_y: f32,
}

impl Camera {
    pub fn tick(&mut self, dt: f32) {
        // Smooth lerp: speed = 5.0 (matches pixel-agents CAMERA_LERP)
        self.x += (self.target_x - self.x) * (5.0 * dt).min(1.0);
        self.y += (self.target_y - self.y) * (5.0 * dt).min(1.0);
    }

    pub fn follow(&mut self, char: &Character) {
        self.target_x = char.tile_x * 16.0;
        self.target_y = char.tile_y * 16.0;
    }
}
```

- **Auto-follow**: camera tracks the character whose agent has `ActivityBus.is_generating == true`
- **Manual override**: click any character to pin camera to them; click empty space to unpin
- **Zoom controls**: +/- buttons in bottom toolbar, or scroll wheel; range 1–4× (pixel-perfect)
- **Pan**: middle-mouse drag or click-drag on empty tiles in non-edit mode

### 6.2 Hover tooltips

When cursor hovers a character:

```rust
// In render, after painting characters:
if let Some(hovered_id) = state.hovered_char {
    let ch = &state.characters[hovered_id];
    // Paint a small popup: name, state, current tool, session cost
    render_tooltip(ch, camera, window, cx);
}
```

Tooltip content:
```
┌─────────────────────────────┐
│ ● claude-zed-surface        │
│   Editing src/thread.rs     │
│   Session: $0.042           │
└─────────────────────────────┘
```

### 6.3 Notification chime

Port `notificationSound.ts` to Rust using `rodio` (already in the Zed/IDE dep tree via CPAL):

```rust
pub fn play_done_chime() {
    // E5 → B5 ascending two-note chime
    // ~0.5 sec, sine wave, exponential fade
}
```

Play when: agent transitions from `Working` → `AwaitingReview` (session complete, needs human input).

### 6.4 Toolbar

Bottom toolbar (always visible in panel):

```
[🏠 Reset Camera]  [+] [-] [zoom%]  [✏ Edit Room]  [🔊/🔇]  [⟳ Refresh]
```

Built with GPUI `Button` components from the `ui` crate.

**Definition of done**: Smooth camera follows active agent, zoom works, tooltips show agent detail,
chime plays on review-ready, toolbar provides all controls.

---

## Phase 7 — Furniture Catalog & Room Templates

**Goal**: Full furniture catalog UI, multiple pre-built room templates, import/export layouts.

### 7.1 Furniture catalog sidebar

In room edit mode, a collapsible sidebar shows furniture categories:
- **Seating**: chair (4 rotations), couch
- **Desks**: desk (4 rotations), standing desk
- **Decor**: plant, bookshelf, whiteboard, TV
- **Electronics**: computer (on/off), server rack, coffee machine (animated)

Each item shows a 2× scaled sprite preview. Click to select for placement.

### 7.2 Room templates

Pre-built layouts selectable from a dropdown in edit mode:
- **Default Office** (20×11): open plan, 6 desks
- **War Room** (24×14): large central table, whiteboards
- **Home Office** (12×8): compact, cozy
- **Server Room** (16×10): dark, racks and monitors

Templates are stored as embedded JSON in the binary.

### 7.3 Layout import/export

- **Export**: writes current layout JSON to a user-chosen file path
- **Import**: loads layout JSON, validates schema, applies to room

---

## Dependency Changes

**`crates/prism-hq/Cargo.toml`**:

```toml
# Image loading for sprite sheets
image = { version = "0.25", default-features = false, features = ["png"] }

# Random number generation for wander behavior
rand = "0.8"
```

No other new deps — GPUI's `paint_image` handles rendering; `serde_json` (already present) handles
layout persistence.

---

## File Creation Summary

| File | Phase | Lines (est.) |
|------|-------|-------------|
| `pixel_office/mod.rs` | 1 | 30 |
| `pixel_office/panel.rs` | 1–6 | 350 |
| `pixel_office/sprites.rs` | 1 | 150 |
| `pixel_office/game_loop.rs` | 2 | 60 |
| `pixel_office/characters.rs` | 2 | 200 |
| `pixel_office/pathfinding.rs` | 2 | 80 |
| `pixel_office/renderer.rs` | 2–5 | 400 |
| `pixel_office/office_state.rs` | 2 | 120 |
| `pixel_office/agent_bridge.rs` | 3 | 180 |
| `pixel_office/layout.rs` | 4 | 200 |
| `pixel_office/layout_persistence.rs` | 4 | 80 |
| `assets/pixel_office/**` | 1 | PNG files |

**Total estimated Rust**: ~1,850 lines across 11 files.

---

## Milestones

| Phase | Milestone | Deliverable |
|-------|-----------|-------------|
| 1 | **Sprite Foundation** | Panel opens, static sprite renders |
| 2 | **Living Characters** | Animated demo characters walk/type |
| 3 | **Agent Bridge** | Real agents drive character states |
| 4 | **Office Room** | Persistent room with editor |
| 5 | **Sub-agents** | Hierarchy + matrix effects |
| 6 | **Camera & Polish** | Camera, zoom, tooltips, chime |
| 7 | **Catalog & Templates** | Full furniture library |

Phases 1–3 form the core MVP — characters in the office that reflect real agent activity.
Phases 4–7 layer in depth and delight.
