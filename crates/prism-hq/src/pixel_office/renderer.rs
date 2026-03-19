#![allow(dead_code)]

use std::sync::Arc;

use gpui::{Bounds, Corners, Pixels, Window, fill, px, rgb, size};

use super::characters::{BubbleKind, Character};
use super::office_state::OfficeState;
use super::sprites::SpriteAtlas;

// Each tile renders at this many logical pixels (before zoom).
const TILE_PX: f32 = 32.0;

/// Render one complete frame of the Pixel Office into the canvas `bounds`.
pub fn render_frame(
    bounds: Bounds<Pixels>,
    state: &OfficeState,
    atlas: &Arc<SpriteAtlas>,
    window: &mut Window,
) {
    // Dark background.
    window.paint_quad(fill(bounds, rgb(0x1a1a2e)));

    // Paint a simple tiled floor for the walkable area.
    if !atlas.floors.is_empty() {
        let floor_tile = &atlas.floors[1]; // floor_1 as default tile
        for &(tx, ty) in &state.walkable_tiles {
            let sx = bounds.origin.x + px(tx as f32 * TILE_PX);
            let sy = bounds.origin.y + px(ty as f32 * TILE_PX);
            let _ = window.paint_image(
                Bounds::new(gpui::point(sx, sy), size(px(TILE_PX), px(TILE_PX))),
                Corners::default(),
                floor_tile.clone(),
                0,
                false,
            );
        }
    }

    // Sort characters back-to-front by tile_y for crude z-ordering.
    let mut sorted: Vec<&Character> = state.characters.iter().collect();
    sorted.sort_by(|a, b| a.tile_y.partial_cmp(&b.tile_y).unwrap_or(std::cmp::Ordering::Equal));

    for ch in sorted {
        render_character(ch, atlas, bounds.origin, window);
    }
}

fn render_character(
    ch: &Character,
    atlas: &Arc<SpriteAtlas>,
    origin: gpui::Point<Pixels>,
    window: &mut Window,
) {
    let palette = ch.palette.min(atlas.characters.len().saturating_sub(1));
    let sheet = &atlas.characters[palette];

    let col = ch.sprite_col();
    let row = ch.direction.sprite_row();

    // Validate indices before indexing.
    if col >= sheet.cols || row >= sheet.rows {
        return;
    }

    let frame = sheet.frame(col, row);

    // Scale 16×32 sprite up 2× to 32×64 for visibility.
    let scale = 2.0_f32;
    let fw = px(sheet.frame_w as f32 * scale);
    let fh = px(sheet.frame_h as f32 * scale);

    // Center the sprite on the tile position.
    let sx = origin.x + px(ch.tile_x * TILE_PX) - fw / 2.0;
    let sy = origin.y + px(ch.tile_y * TILE_PX) - fh / 2.0;

    let _ = window.paint_image(
        Bounds::new(gpui::point(sx, sy), size(fw, fh)),
        Corners::default(),
        frame.clone(),
        0,
        false,
    );

    // Render bubble if active.
    if let Some(bubble_kind) = ch.bubble {
        render_bubble(bubble_kind, sx + fw / 2.0 - px(8.0), sy - px(14.0), atlas, window);
    }
}

fn render_bubble(
    kind: BubbleKind,
    sx: Pixels,
    sy: Pixels,
    _atlas: &Arc<SpriteAtlas>,
    window: &mut Window,
) {
    // We don't have dedicated bubble sprites yet — show a small colored quad as placeholder.
    let color = match kind {
        BubbleKind::Permission => rgb(0xffd700), // gold for permission requests
        BubbleKind::Waiting => rgb(0x00bfff),    // blue for waiting
    };
    window.paint_quad(fill(
        Bounds::new(gpui::point(sx, sy), size(px(16.0), px(13.0))),
        color,
    ));
}

/// Return the index of the character under `mouse_pos` (tile-based hit test), if any.
pub fn hit_test_character(
    mouse_pos: gpui::Point<Pixels>,
    state: &OfficeState,
    canvas_origin: gpui::Point<Pixels>,
) -> Option<usize> {
    let scale = 2.0_f32;
    let fw = px(16.0 * scale);
    let fh = px(32.0 * scale);

    for ch in &state.characters {
        let sx = canvas_origin.x + px(ch.tile_x * TILE_PX) - fw / 2.0;
        let sy = canvas_origin.y + px(ch.tile_y * TILE_PX) - fh / 2.0;

        let char_bounds = gpui::Bounds::new(gpui::point(sx, sy), size(fw, fh));
        if char_bounds.contains(&mouse_pos) {
            return Some(ch.id);
        }
    }
    None
}
