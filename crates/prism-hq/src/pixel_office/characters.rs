use std::collections::HashSet;

use rand::Rng;
use rand::seq::IteratorRandom as _;

use super::pathfinding::find_path;
use super::sprites::{char_frames, char_rows};

// ── timing constants ──────────────────────────────────────────────────────────

pub const WALK_SPEED: f32 = 3.0; // tiles per second
pub const WALK_FRAME_DUR: f32 = 0.15; // seconds per walk frame
pub const TYPE_FRAME_DUR: f32 = 0.30; // seconds per type frame
pub const IDLE_FRAME_DUR: f32 = 0.50; // seconds per idle frame (single frame)
pub const WANDER_PAUSE_MIN: f32 = 2.0;
pub const WANDER_PAUSE_MAX: f32 = 20.0;
pub const BUBBLE_DURATION: f32 = 4.0;
pub const SNAP_THRESHOLD: f32 = 0.05; // tile units

// ── enums ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharState {
    Idle,
    Walk,
    Type,
    Wait,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Down = 0,
    Left = 1,
    Right = 2,
    Up = 3,
}

impl Direction {
    pub fn sprite_row(self) -> usize {
        match self {
            Direction::Down => char_rows::DOWN,
            Direction::Up => char_rows::UP,
            Direction::Right => char_rows::RIGHT,
            Direction::Left => char_rows::LEFT, // same sheet row as Right
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BubbleKind {
    Permission,
    Waiting,
}

// ── character ─────────────────────────────────────────────────────────────────

pub struct Character {
    pub id: usize,
    pub palette: usize,
    pub name: String,

    // World position in tile units (fractional for smooth movement).
    pub tile_x: f32,
    pub tile_y: f32,

    pub direction: Direction,
    pub state: CharState,

    // Animation
    pub anim_timer: f32,
    pub frame_index: usize,

    // Pathfinding
    pub path: Vec<(i32, i32)>,
    pub path_index: usize,
    /// State to enter when the current path finishes.
    pub path_target_state: CharState,

    // Seating
    pub seat: Option<(i32, i32, Direction)>,

    // Wander
    pub wander_timer: f32,

    // Bubble
    pub bubble: Option<BubbleKind>,
    pub bubble_timer: f32,

    pub status_text: Option<String>,
}

impl Character {
    pub fn new(
        id: usize,
        palette: usize,
        name: impl Into<String>,
        tile_x: f32,
        tile_y: f32,
    ) -> Self {
        Self {
            id,
            palette,
            name: name.into(),
            tile_x,
            tile_y,
            direction: Direction::Down,
            state: CharState::Idle,
            anim_timer: 0.0,
            frame_index: 0,
            path: Vec::new(),
            path_index: 0,
            path_target_state: CharState::Idle,
            seat: None,
            wander_timer: 0.0,
            bubble: None,
            bubble_timer: 0.0,
            status_text: None,
        }
    }

    /// Current sprite sheet column index for this character's animation.
    pub fn sprite_col(&self) -> usize {
        match self.state {
            CharState::Idle | CharState::Wait => char_frames::IDLE,
            CharState::Walk => char_frames::WALK[self.frame_index % char_frames::WALK.len()],
            CharState::Type => char_frames::TYPE_ANIM[self.frame_index % char_frames::TYPE_ANIM.len()],
        }
    }

    /// Update animation and movement by `dt` seconds.
    pub fn tick(&mut self, dt: f32, walkable: &HashSet<(i32, i32)>, rng: &mut impl Rng) {
        // ── bubble timer ──────────────────────────────────────────────────────
        if self.bubble.is_some() {
            self.bubble_timer -= dt;
            if self.bubble_timer <= 0.0 {
                self.bubble = None;
            }
        }

        // ── animation cycling ─────────────────────────────────────────────────
        let frame_dur = match self.state {
            CharState::Walk => WALK_FRAME_DUR,
            CharState::Type => TYPE_FRAME_DUR,
            _ => IDLE_FRAME_DUR,
        };
        self.anim_timer += dt;
        if self.anim_timer >= frame_dur {
            self.anim_timer -= frame_dur;
            let cycle_len = match self.state {
                CharState::Walk => char_frames::WALK.len(),
                CharState::Type => char_frames::TYPE_ANIM.len(),
                _ => 1,
            };
            self.frame_index = (self.frame_index + 1) % cycle_len;
        }

        // ── path following ────────────────────────────────────────────────────
        if self.state == CharState::Walk && !self.path.is_empty() {
            if self.path_index < self.path.len() {
                let (tx, ty) = self.path[self.path_index];
                let (tx, ty) = (tx as f32, ty as f32);

                let dx = tx - self.tile_x;
                let dy = ty - self.tile_y;
                let dist = (dx * dx + dy * dy).sqrt();

                // Update direction based on movement.
                if dx.abs() > dy.abs() {
                    self.direction = if dx > 0.0 { Direction::Right } else { Direction::Left };
                } else if dy.abs() > 0.001 {
                    self.direction = if dy > 0.0 { Direction::Down } else { Direction::Up };
                }

                if dist < SNAP_THRESHOLD {
                    // Snap to waypoint, advance to next.
                    self.tile_x = tx;
                    self.tile_y = ty;
                    self.path_index += 1;
                } else {
                    let step = WALK_SPEED * dt;
                    self.tile_x += (dx / dist) * step.min(dist);
                    self.tile_y += (dy / dist) * step.min(dist);
                }
            } else {
                // Path complete — transition to target state.
                self.path.clear();
                self.path_index = 0;
                self.state = self.path_target_state;
                self.frame_index = 0;
                self.anim_timer = 0.0;
            }
            return;
        }

        // ── wander (idle, no path, no active task) ─────────────────────────
        if self.state == CharState::Idle {
            self.wander_timer -= dt;
            if self.wander_timer <= 0.0 {
                self.wander_timer =
                    rng.random_range(WANDER_PAUSE_MIN..WANDER_PAUSE_MAX);

                // IteratorRandom::choose picks a random element without collecting to Vec.
                if let Some(&target) = walkable.iter().choose(rng) {
                    let from = (self.tile_x.round() as i32, self.tile_y.round() as i32);
                    let path = find_path(from, target, walkable);
                    if !path.is_empty() {
                        self.path = path;
                        self.path_index = 0;
                        self.path_target_state = CharState::Idle;
                        self.state = CharState::Walk;
                        self.frame_index = 0;
                        self.anim_timer = 0.0;
                    }
                }
            }
        }
    }

    /// Queue a walk to `target`, transitioning to `on_arrive` when done.
    pub fn walk_to(
        &mut self,
        target: (i32, i32),
        on_arrive: CharState,
        walkable: &HashSet<(i32, i32)>,
    ) {
        let from = (self.tile_x.round() as i32, self.tile_y.round() as i32);
        let path = find_path(from, target, walkable);
        if !path.is_empty() {
            self.path = path;
            self.path_index = 0;
            self.path_target_state = on_arrive;
            self.state = CharState::Walk;
            self.frame_index = 0;
            self.anim_timer = 0.0;
        } else {
            // Can't reach — just switch state immediately.
            self.state = on_arrive;
            self.frame_index = 0;
            self.anim_timer = 0.0;
        }
    }

    /// Show a speech bubble above this character.
    pub fn show_bubble(&mut self, kind: BubbleKind) {
        self.bubble = Some(kind);
        self.bubble_timer = BUBBLE_DURATION;
    }
}
