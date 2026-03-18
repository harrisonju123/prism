use std::collections::HashSet;

use rand::SeedableRng;
use rand::rngs::SmallRng;

use super::agent_bridge::OfficeMutation;
use super::characters::{BubbleKind, CharState, Character, Direction};

/// The mutable game state for the Pixel Office — characters, walkable tiles, etc.
pub struct OfficeState {
    pub characters: Vec<Character>,
    pub walkable_tiles: HashSet<(i32, i32)>,
    pub rng: SmallRng,
    pub hovered_char: Option<usize>,
    next_id: usize,
    next_palette: usize,
}

impl OfficeState {
    /// Create a default demo office with a 10×8 walkable floor grid and 3 demo characters.
    pub fn demo() -> Self {
        let mut walkable = HashSet::new();
        for x in 1..=10 {
            for y in 1..=8 {
                walkable.insert((x, y));
            }
        }

        let rng = SmallRng::seed_from_u64(42);

        let mut state = Self {
            characters: Vec::new(),
            walkable_tiles: walkable,
            rng,
            hovered_char: None,
            next_id: 0,
            next_palette: 0,
        };

        // Spawn 3 demo characters in different initial states.
        state.spawn_demo_character("claude", 3.0, 3.0, CharState::Type);
        state.spawn_demo_character("gemini", 7.0, 5.0, CharState::Idle);
        state.spawn_demo_character("gpt-4o", 5.0, 2.0, CharState::Wait);

        state
    }

    fn spawn_demo_character(
        &mut self,
        name: &str,
        x: f32,
        y: f32,
        initial_state: CharState,
    ) {
        let id = self.next_id;
        self.next_id += 1;
        let palette = self.next_palette % 6;
        self.next_palette += 1;

        let mut ch = Character::new(id, palette, name, x, y);
        ch.state = initial_state;

        // Assign a simple hardcoded seat near spawn.
        let seat_x = x.round() as i32;
        let seat_y = y.round() as i32;
        ch.seat = Some((seat_x, seat_y, Direction::Down));

        // Give the waiting character a bubble.
        if initial_state == CharState::Wait {
            ch.show_bubble(BubbleKind::Waiting);
        }

        self.characters.push(ch);
    }

    /// Tick all characters forward by `dt` seconds.
    pub fn tick(&mut self, dt: f32) {
        let walkable = self.walkable_tiles.clone();
        for ch in &mut self.characters {
            ch.tick(dt, &walkable, &mut self.rng);
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
    pub fn apply_mutations(
        &mut self,
        mutations: Vec<OfficeMutation>,
        agent_to_char_id: &std::collections::HashMap<String, usize>,
    ) {
        for mutation in mutations {
            match mutation {
                OfficeMutation::SpawnCharacter { agent_name, palette } => {
                    // Pick a spawn point — a random walkable tile.
                    let tiles: Vec<_> = self.walkable_tiles.iter().copied().collect();
                    if tiles.is_empty() {
                        continue;
                    }
                    let idx = rand::Rng::random_range(&mut self.rng, 0..tiles.len());
                    let (tx, ty) = tiles[idx];

                    let id = if let Some(&cid) = agent_to_char_id.get(&agent_name) {
                        cid
                    } else {
                        continue;
                    };

                    // Remove any existing character with this id or name.
                    self.characters.retain(|c| c.id != id && c.name != agent_name);

                    let mut ch = Character::new(id, palette, agent_name, tx as f32, ty as f32);
                    ch.state = CharState::Idle;
                    self.characters.push(ch);
                }
                OfficeMutation::DespawnCharacter { agent_name } => {
                    self.characters.retain(|c| c.name != agent_name);
                }
                OfficeMutation::SetState { agent_name, char_state, status_text } => {
                    if let Some(ch) = self.character_by_name_mut(&agent_name) {
                        ch.state = char_state;
                        ch.status_text = status_text;
                        ch.frame_index = 0;
                        ch.anim_timer = 0.0;
                    }
                }
                OfficeMutation::ShowBubble { agent_name, kind } => {
                    if let Some(ch) = self.character_by_name_mut(&agent_name) {
                        ch.show_bubble(kind);
                    }
                }
                OfficeMutation::ClearBubble { agent_name } => {
                    if let Some(ch) = self.character_by_name_mut(&agent_name) {
                        ch.bubble = None;
                    }
                }
            }
        }
    }
}
