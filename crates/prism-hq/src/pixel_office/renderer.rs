use std::sync::Arc;

use gpui::{Bounds, Corners, Pixels, Window, fill, px, rgb, size};

use super::characters::{BubbleKind, Direction};
use super::office_state::{LayoutRenderData, OfficeState};
use super::sprites::SpriteAtlas;

// Each tile renders at this many logical pixels (before zoom).
const TILE_PX: f32 = 32.0;

/// Snapshot of one character's render state.
/// Tuple: (id, palette, tile_x, tile_y, direction, sprite_col, bubble).
pub type CharSnapshot = (usize, usize, f32, f32, Direction, usize, Option<BubbleKind>);

/// Render one complete frame into the canvas `bounds`.
///
/// Rendering passes (back to front):
/// 1. Dark background
/// 2. Floor tiles (non-0, non-255 tile values → floor sprite by index)
/// 3. Wall tiles (tile value 0, adjacent to floor)
/// 4. Wall-mounted furniture (row < 11, placed above the visible floor)
/// 5. Z-sorted floor furniture + characters (by bottom pixel, back to front)
/// 6. Speech bubbles
pub fn render_frame(
    bounds: Bounds<Pixels>,
    characters: &[CharSnapshot],
    layout: &LayoutRenderData,
    atlas: &Arc<SpriteAtlas>,
    window: &mut Window,
) {
    // ── 1. Background ─────────────────────────────────────────────────────────
    window.paint_quad(fill(bounds, rgb(0x1a1a2e)));

    // ── 2 & 3. Floor and wall tiles ───────────────────────────────────────────
    for row in 0..layout.rows {
        for col in 0..layout.cols {
            let tile = layout.tiles[row * layout.cols + col];
            if tile == 255 {
                continue; // void — skip
            }

            let sx = bounds.origin.x + px(col as f32 * TILE_PX);
            let sy = bounds.origin.y + px(row as f32 * TILE_PX);
            let tile_bounds =
                Bounds::new(gpui::point(sx, sy), size(px(TILE_PX), px(TILE_PX)));

            if tile == 0 {
                // Wall tile — use wall sheet frame (0,0) for now.
                if atlas.walls.cols > 0 && atlas.walls.rows > 0 {
                    let _ = window.paint_image(
                        tile_bounds,
                        Corners::default(),
                        atlas.walls.frame(0, 0).clone(),
                        0,
                        false,
                    );
                }
            } else {
                // Floor tile — tile value indexes into atlas.floors.
                let floor_idx = (tile as usize).min(atlas.floors.len().saturating_sub(1));
                let _ = window.paint_image(
                    tile_bounds,
                    Corners::default(),
                    atlas.floors[floor_idx].clone(),
                    0,
                    false,
                );
            }
        }
    }

    // ── 4. Wall-mounted decorations (row < 11 = above office floor) ──────────
    for furn in &layout.furniture {
        if furn.row >= 11 {
            continue; // floor furniture handled in pass 5
        }
        paint_furniture(furn.col, furn.row, &furn.asset_id, atlas, &bounds, window);
    }

    // ── 5. Z-sorted: floor furniture + characters ─────────────────────────────
    // Sort key = bottom pixel row (row + sprite_height_in_tiles).
    enum Renderable<'a> {
        Furniture { col: i32, row: i32, asset_id: &'a str },
        Character(&'a CharSnapshot),
    }

    let mut renderables: Vec<(f32, Renderable)> = Vec::new();

    // Collect floor furniture (row >= 11).
    for furn in &layout.furniture {
        if furn.row < 11 {
            continue;
        }
        // Sort key: bottom of furniture sprite (assume 1 tile tall for simplicity).
        let sort_y = furn.row as f32 + 1.0;
        renderables.push((
            sort_y,
            Renderable::Furniture { col: furn.col, row: furn.row, asset_id: &furn.asset_id },
        ));
    }

    // Collect characters.
    for snap in characters {
        let sort_y = snap.3 + 1.0; // tile_y + 1
        renderables.push((sort_y, Renderable::Character(snap)));
    }

    // Sort back-to-front by sort_y.
    renderables.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    for (_, renderable) in &renderables {
        match renderable {
            Renderable::Furniture { col, row, asset_id } => {
                paint_furniture(*col, *row, asset_id, atlas, &bounds, window);
            }
            Renderable::Character(snap) => {
                paint_character(snap, atlas, &bounds, window);
            }
        }
    }

    // ── 6. Bubbles (on top of everything) ────────────────────────────────────
    for snap in characters {
        let (_, _, tile_x, tile_y, _, _, bubble) = snap;
        if let Some(kind) = bubble {
            let scale = 2.0_f32;
            let fw = px(16.0 * scale);
            let fh = px(32.0 * scale);
            let sx = bounds.origin.x + px(tile_x * TILE_PX) - fw / 2.0;
            let sy = bounds.origin.y + px(tile_y * TILE_PX) - fh / 2.0;

            let color = match kind {
                BubbleKind::Permission => rgb(0xffd700),
                BubbleKind::Waiting => rgb(0x00bfff),
            };
            window.paint_quad(fill(
                Bounds::new(
                    gpui::point(sx + fw / 2.0 - px(8.0), sy - px(14.0)),
                    size(px(16.0), px(13.0)),
                ),
                color,
            ));
        }
    }
}

/// Paint a single furniture sprite at tile position (col, row).
fn paint_furniture(
    col: i32,
    row: i32,
    asset_id: &str,
    atlas: &SpriteAtlas,
    bounds: &Bounds<Pixels>,
    window: &mut Window,
) {
    let Some((image, pw, ph)) = atlas.furniture.get(asset_id) else {
        return;
    };

    let scale = 2.0_f32;
    let render_w = px(*pw as f32 * scale);
    let render_h = px(*ph as f32 * scale);

    // Anchor: tile top-left at (col * TILE_PX, row * TILE_PX).
    // For sprites taller than one tile, anchor to the bottom of the first tile row.
    let sx = bounds.origin.x + px(col as f32 * TILE_PX);
    let sy = bounds.origin.y + px(row as f32 * TILE_PX) + px(TILE_PX) - render_h;

    let _ = window.paint_image(
        Bounds::new(gpui::point(sx, sy), size(render_w, render_h)),
        Corners::default(),
        image.clone(),
        0,
        false,
    );
}

/// Paint a character sprite from a snapshot.
fn paint_character(
    snap: &CharSnapshot,
    atlas: &SpriteAtlas,
    bounds: &Bounds<Pixels>,
    window: &mut Window,
) {
    let (_, palette, tile_x, tile_y, direction, col, _) = snap;
    let pal = (*palette).min(atlas.characters.len().saturating_sub(1));
    let sheet = &atlas.characters[pal];
    let row = direction.sprite_row();

    if *col >= sheet.cols || row >= sheet.rows {
        return;
    }

    let frame = sheet.frame(*col, row);
    let scale = 2.0_f32;
    let fw = px(sheet.frame_w as f32 * scale);
    let fh = px(sheet.frame_h as f32 * scale);
    let sx = bounds.origin.x + px(tile_x * TILE_PX) - fw / 2.0;
    let sy = bounds.origin.y + px(tile_y * TILE_PX) - fh / 2.0;

    let _ = window.paint_image(
        Bounds::new(gpui::point(sx, sy), size(fw, fh)),
        Corners::default(),
        frame.clone(),
        0,
        false,
    );
}

/// Return the id of the character under `mouse_pos` (tile-based hit test), if any.
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
