use std::collections::{HashMap, HashSet};

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
pub struct LayoutRenderData {
    pub cols: usize,
    pub rows: usize,
    pub tiles: Vec<u8>,
    pub furniture: Vec<FurnitureRenderItem>,
    pub pc_states: HashMap<String, PcRenderState>,
}

// ── office state ──────────────────────────────────────────────────────────────

/// The mutable game state for the Pixel Office — characters, walkable tiles, etc.
pub struct OfficeState {
    pub characters: Vec<Character>,
    pub walkable_tiles: HashSet<(i32, i32)>,
    pub rng: SmallRng,
    pub hovered_char: Option<usize>,
    pub layout: OfficeLayout,
    pub pc_anim_states: HashMap<String, PcAnimState>,
    next_id: usize,
    next_palette: usize,
}

impl OfficeState {
    /// Create a live office from the embedded layout JSON.
    pub fn from_layout() -> Self {
        let layout = OfficeLayout::load();
        let walkable_tiles = layout.walkable_tiles.clone();
        let rng = SmallRng::seed_from_u64(42);

        Self {
            characters: Vec::new(),
            walkable_tiles,
            rng,
            hovered_char: None,
            layout,
            pc_anim_states: HashMap::new(),
            next_id: 0,
            next_palette: 0,
        }
    }

    /// Build a `LayoutRenderData` snapshot suitable for the render closure.
    pub fn layout_render_data(&self) -> LayoutRenderData {
        let furniture = self
            .layout
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
            .collect();

        let pc_states = self
            .pc_anim_states
            .iter()
            .map(|(uid, st)| {
                (uid.clone(), PcRenderState { on: st.on, frame: st.frame })
            })
            .collect();

        LayoutRenderData {
            cols: self.layout.cols,
            rows: self.layout.rows,
            tiles: self.layout.tiles.clone(),
            furniture,
            pc_states,
        }
    }

    /// Tick all characters forward by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        // Compute the lounge zone tile set once for idle wander pooling.
        let lounge_tiles: HashSet<(i32, i32)> = self
            .layout
            .zones
            .lounge
            .tiles_in(&self.walkable_tiles)
            .into_iter()
            .collect();

        for ch in &mut self.characters {
            ch.tick(dt, &self.walkable_tiles, &lounge_tiles, &mut self.rng);
        }

        self.tick_pc_anims(dt);
    }

    /// Update PC on/off animation based on which desk seats are occupied by typing characters.
    fn tick_pc_anims(&mut self, dt: f32) {
        // Build a set of seat tiles where the character is in Type state.
        let active_seats: HashSet<(i32, i32)> = self
            .characters
            .iter()
            .filter(|ch| ch.state == CharState::Type)
            .filter_map(|ch| ch.seat.map(|(x, y, _)| (x, y)))
            .collect();

        // Collect (tile, pc_uid) pairs to avoid borrowing layout inside the mutation loop.
        let desk_info: Vec<((i32, i32), String)> = self
            .layout
            .stations
            .desk_seats
            .iter()
            .filter_map(|s| s.furniture_uid.as_ref().map(|uid| (s.tile, uid.clone())))
            .collect();

        for (tile, pc_uid) in desk_info {
            let is_active = active_seats.contains(&tile);
            let entry = self.pc_anim_states.entry(pc_uid).or_default();
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
                    let lounge_tiles = self.layout.zones.lounge.tiles_in(&self.walkable_tiles);
                    let spawn_tile = lounge_tiles
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
                        // Collect first to end the zone borrow before mutably borrowing rng.
                        let candidates = zone.tiles_in(&self.walkable_tiles);
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
                            ch.walk_to(t, char_state, &self.walkable_tiles);
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
