use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rand::SeedableRng;
use rand::rngs::SmallRng;

use super::agent_bridge::OfficeMutation;
use super::characters::{CharState, Character, Direction};
use super::layout::{OfficeLayout, StationRegistry, Zone};

// ── PC animation state ─────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct PcAnimState {
    pub on: bool,
    pub frame: usize,
    pub timer: f32,
}

// ── render snapshot types (used by renderer.rs) ────────────────────────────────

/// Lightweight per-furniture data for rendering.
#[derive(Clone)]
pub struct FurnitureRenderItem {
    pub asset_id: String,
    pub mirror: bool,
    pub col: i32,
    pub row: i32,
}

/// PC animation state needed by the renderer.
#[derive(Clone)]
pub struct PcRenderState {
    pub on: bool,
    pub frame: usize,
}

/// All layout-level data needed by `renderer::render_frame`.
///
/// `tiles` and `furniture` are `Arc`-wrapped because the layout is static for the
/// entire session — cloning them into the 60 fps canvas closure is a cheap Arc refcount
/// bump rather than a full Vec copy.
pub struct LayoutRenderData {
    pub cols: usize,
    pub rows: usize,
    pub tiles: Arc<Vec<u8>>,
    pub furniture: Arc<Vec<FurnitureRenderItem>>,
    /// Dynamic: changes when PCs turn on/off. Rebuilt cheaply (2–4 entries).
    pub pc_states: HashMap<String, PcRenderState>,
}

// ── office state ──────────────────────────────────────────────────────────────

/// The mutable game state for the Pixel Office — characters, walkable tiles, etc.
pub struct OfficeState {
    pub characters: Vec<Character>,
    pub rng: SmallRng,
    pub hovered_char: Option<usize>,
    pub layout: OfficeLayout,
    pub pc_anim_states: HashMap<String, PcAnimState>,
    /// Cached tiles Arc — avoids cloning Vec<u8> on every render frame.
    cached_tiles: Arc<Vec<u8>>,
    /// Cached furniture render items Arc — avoids rebuilding on every render frame.
    cached_furniture_render: Arc<Vec<FurnitureRenderItem>>,
    /// Tiles within the lounge zone — precomputed once, used for idle wander pooling.
    lounge_wander_tiles: HashSet<(i32, i32)>,
    /// (seat_tile, pc_uid) pairs for desk seats — precomputed once for tick_pc_anims.
    desk_pc_links: Vec<((i32, i32), String)>,
    next_id: usize,
    next_palette: usize,
}

impl OfficeState {
    /// Create a live office from the embedded layout JSON.
    pub fn from_layout() -> Self {
        let layout = OfficeLayout::load();

        let lounge_wander_tiles: HashSet<(i32, i32)> =
            layout.zones.lounge.tiles_in(&layout.walkable_tiles).into_iter().collect();

        let desk_pc_links: Vec<((i32, i32), String)> = layout
            .stations
            .desk_seats
            .iter()
            .filter_map(|s| s.furniture_uid.as_ref().map(|uid| (s.tile, uid.clone())))
            .collect();

        let cached_tiles = Arc::new(layout.tiles.clone());

        let cached_furniture_render = Arc::new(
            layout
                .furniture
                .iter()
                .map(|f| FurnitureRenderItem {
                    asset_id: if f.mirror {
                        format!("{}:left", f.asset_id)
                    } else {
                        f.asset_id.clone()
                    },
                    mirror: f.mirror,
                    col: f.col,
                    row: f.row,
                })
                .collect(),
        );

        Self {
            characters: Vec::new(),
            rng: SmallRng::seed_from_u64(42),
            hovered_char: None,
            layout,
            pc_anim_states: HashMap::new(),
            cached_tiles,
            cached_furniture_render,
            lounge_wander_tiles,
            desk_pc_links,
            next_id: 0,
            next_palette: 0,
        }
    }

    /// Build a `LayoutRenderData` snapshot suitable for the render closure.
    ///
    /// Static portions (tiles, furniture) are Arc clones — O(1). Only `pc_states`
    /// is rebuilt from scratch each call (2–4 entries in the default layout).
    pub fn layout_render_data(&self) -> LayoutRenderData {
        let pc_states = self
            .pc_anim_states
            .iter()
            .map(|(uid, st)| (uid.clone(), PcRenderState { on: st.on, frame: st.frame }))
            .collect();

        LayoutRenderData {
            cols: self.layout.cols,
            rows: self.layout.rows,
            tiles: Arc::clone(&self.cached_tiles),
            furniture: Arc::clone(&self.cached_furniture_render),
            pc_states,
        }
    }

    /// Tick all characters forward by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        for ch in &mut self.characters {
            ch.tick(
                dt,
                &self.layout.walkable_tiles,
                &self.lounge_wander_tiles,
                &mut self.rng,
            );
        }
        self.tick_pc_anims(dt);
    }

    /// Update PC on/off animation based on which desk seats are occupied by typing characters.
    fn tick_pc_anims(&mut self, dt: f32) {
        // Build a set of seat tiles where the character is currently typing.
        let active_seats: HashSet<(i32, i32)> = self
            .characters
            .iter()
            .filter(|ch| ch.state == CharState::Type)
            .filter_map(|ch| ch.seat.map(|(x, y, _)| (x, y)))
            .collect();

        for (tile, pc_uid) in &self.desk_pc_links {
            let is_active = active_seats.contains(tile);
            let entry = self.pc_anim_states.entry(pc_uid.clone()).or_default();
            entry.on = is_active;
            if is_active {
                entry.timer += dt;
                if entry.timer >= 0.3 {
                    entry.timer -= 0.3;
                    entry.frame = (entry.frame + 1) % 3;
                }
            } else {
                entry.frame = 0;
                entry.timer = 0.0;
            }
        }
    }

    /// Spawn a new character and return its assigned id.
    pub fn spawn_character(
        &mut self,
        name: impl Into<String>,
        tile_x: f32,
        tile_y: f32,
    ) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        let palette = self.next_palette % 6;
        self.next_palette += 1;

        let ch = Character::new(id, palette, name, tile_x, tile_y);
        self.characters.push(ch);
        id
    }

    /// Remove the character with the given id.
    pub fn despawn_character(&mut self, id: usize) {
        self.characters.retain(|ch| ch.id != id);
    }

    /// Find character by id (mutable).
    pub fn character_mut(&mut self, id: usize) -> Option<&mut Character> {
        self.characters.iter_mut().find(|ch| ch.id == id)
    }

    /// Find character by agent name (mutable).
    pub fn character_by_name_mut(&mut self, name: &str) -> Option<&mut Character> {
        self.characters.iter_mut().find(|ch| ch.name == name)
    }

    /// Apply a batch of mutations from the `AgentBridge`.
    pub fn apply_mutations(&mut self, mutations: Vec<OfficeMutation>) {
        for mutation in mutations {
            match mutation {
                OfficeMutation::SpawnCharacter { agent_name, palette, char_id } => {
                    // Spawn inside the lounge zone.
                    use rand::seq::IteratorRandom as _;
                    let spawn_tile = self
                        .lounge_wander_tiles
                        .iter()
                        .copied()
                        .choose(&mut self.rng)
                        .unwrap_or((14, 15));

                    // Remove any existing character with this id or name before re-spawning.
                    self.characters
                        .retain(|c| c.id != char_id && c.name != agent_name);

                    let mut ch = Character::new(
                        char_id,
                        palette,
                        agent_name,
                        spawn_tile.0 as f32,
                        spawn_tile.1 as f32,
                    );
                    ch.state = CharState::Idle;
                    self.characters.push(ch);
                }

                OfficeMutation::DespawnCharacter { agent_name } => {
                    self.layout.stations.release_all(&agent_name);
                    self.characters.retain(|c| c.name != agent_name);
                }

                OfficeMutation::SetState { agent_name, char_state, status_text } => {
                    // 1. Release all existing stations for this agent.
                    self.layout.stations.release_all(&agent_name);

                    // 2. Try to claim a station in the target zone.
                    let claimed: Option<((i32, i32), Direction)> = match char_state {
                        CharState::Read => {
                            StationRegistry::claim(
                                &mut self.layout.stations.library_spots,
                                &agent_name,
                            )
                            .map(|t| (t, Direction::Up))
                        }
                        CharState::Type => {
                            StationRegistry::claim(
                                &mut self.layout.stations.desk_seats,
                                &agent_name,
                            )
                            .map(|t| (t, Direction::Up))
                        }
                        CharState::Wait => {
                            StationRegistry::claim(
                                &mut self.layout.stations.meeting_spots,
                                &agent_name,
                            )
                            .map(|t| (t, Direction::Right))
                        }
                        CharState::Idle => {
                            StationRegistry::claim(
                                &mut self.layout.stations.lounge_spots,
                                &agent_name,
                            )
                            .map(|t| (t, Direction::Down))
                        }
                        CharState::Walk => None,
                    };

                    // 3. Determine target tile (claimed station or random zone tile).
                    let target_tile: Option<(i32, i32)> = if let Some((tile, _)) = claimed {
                        Some(tile)
                    } else {
                        let zone: &Zone = match char_state {
                            CharState::Read => &self.layout.zones.library,
                            CharState::Type => &self.layout.zones.computer,
                            CharState::Wait => &self.layout.zones.meeting,
                            CharState::Idle | CharState::Walk => &self.layout.zones.lounge,
                        };
                        let candidates = zone.tiles_in(&self.layout.walkable_tiles);
                        use rand::seq::IteratorRandom as _;
                        candidates.into_iter().choose(&mut self.rng)
                    };

                    // 4. Update the character.
                    if let Some(ch) =
                        self.characters.iter_mut().find(|c| c.name == agent_name)
                    {
                        ch.status_text = status_text;
                        ch.frame_index = 0;
                        ch.anim_timer = 0.0;

                        if let Some((tile, dir)) = claimed {
                            ch.seat = Some((tile.0, tile.1, dir));
                        }

                        if let Some(t) = target_tile {
                            ch.walk_to(t, char_state, &self.layout.walkable_tiles);
                        } else {
                            ch.state = char_state;
                        }
                    }
                }

                OfficeMutation::ShowBubble { agent_name, kind } => {
                    if let Some(ch) = self.characters.iter_mut().find(|c| c.name == agent_name) {
                        ch.show_bubble(kind);
                    }
                }

                OfficeMutation::ClearBubble { agent_name } => {
                    if let Some(ch) = self.characters.iter_mut().find(|c| c.name == agent_name) {
                        ch.bubble = None;
                    }
                }
            }
        }
    }
}
