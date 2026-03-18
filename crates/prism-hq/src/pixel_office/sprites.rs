use std::sync::Arc;

use anyhow::Result;
use gpui::RenderImage;
use image::{GenericImageView as _, ImageBuffer, Rgba};

/// A sliced sprite sheet where each frame has been pre-converted to BGRA and wrapped
/// in an `Arc<RenderImage>` for use with `Window::paint_image()`.
///
/// GPUI requires BGRA format, so RGBA channels are swapped (0↔2) at load time.
pub struct SpriteSheet {
    frames: Vec<Arc<RenderImage>>,
    pub cols: usize,
    pub rows: usize,
    pub frame_w: u32,
    pub frame_h: u32,
}

impl SpriteSheet {
    /// Decode a PNG sprite sheet, convert RGBA→BGRA, and slice into individual frames.
    pub fn from_bytes(bytes: &[u8], frame_w: u32, frame_h: u32) -> Result<Self> {
        let img = image::load_from_memory(bytes)?.into_rgba8();
        let (sheet_w, sheet_h) = img.dimensions();
        let cols = (sheet_w / frame_w) as usize;
        let rows = (sheet_h / frame_h) as usize;
        let mut frames = Vec::with_capacity(cols * rows);

        for row in 0..rows {
            for col in 0..cols {
                let x = col as u32 * frame_w;
                let y = row as u32 * frame_h;
                let sub: ImageBuffer<Rgba<u8>, Vec<u8>> = img.view(x, y, frame_w, frame_h).to_image();

                // Convert RGBA → BGRA by swapping the red (0) and blue (2) channels.
                let mut bgra = sub;
                for pixel in bgra.pixels_mut() {
                    pixel.0.swap(0, 2);
                }

                let render = Arc::new(RenderImage::new(vec![image::Frame::new(bgra)]));
                frames.push(render);
            }
        }

        Ok(Self { frames, cols, rows, frame_w, frame_h })
    }

    /// Get a frame by column and row index.
    ///
    /// Panics if the indices are out of bounds.
    pub fn frame(&self, col: usize, row: usize) -> &Arc<RenderImage> {
        &self.frames[row * self.cols + col]
    }
}

/// Load a single-tile PNG as a one-frame `Arc<RenderImage>` (BGRA-converted).
fn single_frame(bytes: &[u8]) -> Result<Arc<RenderImage>> {
    let img = image::load_from_memory(bytes)?.into_rgba8();
    let mut bgra = img;
    for pixel in bgra.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![image::Frame::new(bgra)])))
}

/// All loaded sprite assets for the Pixel Office panel.
pub struct SpriteAtlas {
    /// 6 character palettes, each a 7-column × 3-row sheet (16×32 px per frame).
    pub characters: Vec<SpriteSheet>,
    /// 9 floor tiles (16×16 each), indexed 0–8.
    pub floors: Vec<Arc<RenderImage>>,
    /// Wall sprite sheet sliced into 16×16 variants.
    pub walls: SpriteSheet,
}

impl SpriteAtlas {
    /// Load all sprites from embedded bytes.  Called once in a background task.
    pub fn load() -> Result<Self> {
        let characters = vec![
            SpriteSheet::from_bytes(
                include_bytes!("../../../../assets/pixel_office/characters/char_0.png"),
                16,
                32,
            )?,
            SpriteSheet::from_bytes(
                include_bytes!("../../../../assets/pixel_office/characters/char_1.png"),
                16,
                32,
            )?,
            SpriteSheet::from_bytes(
                include_bytes!("../../../../assets/pixel_office/characters/char_2.png"),
                16,
                32,
            )?,
            SpriteSheet::from_bytes(
                include_bytes!("../../../../assets/pixel_office/characters/char_3.png"),
                16,
                32,
            )?,
            SpriteSheet::from_bytes(
                include_bytes!("../../../../assets/pixel_office/characters/char_4.png"),
                16,
                32,
            )?,
            SpriteSheet::from_bytes(
                include_bytes!("../../../../assets/pixel_office/characters/char_5.png"),
                16,
                32,
            )?,
        ];

        let floors = vec![
            single_frame(include_bytes!("../../../../assets/pixel_office/floors/floor_0.png"))?,
            single_frame(include_bytes!("../../../../assets/pixel_office/floors/floor_1.png"))?,
            single_frame(include_bytes!("../../../../assets/pixel_office/floors/floor_2.png"))?,
            single_frame(include_bytes!("../../../../assets/pixel_office/floors/floor_3.png"))?,
            single_frame(include_bytes!("../../../../assets/pixel_office/floors/floor_4.png"))?,
            single_frame(include_bytes!("../../../../assets/pixel_office/floors/floor_5.png"))?,
            single_frame(include_bytes!("../../../../assets/pixel_office/floors/floor_6.png"))?,
            single_frame(include_bytes!("../../../../assets/pixel_office/floors/floor_7.png"))?,
            single_frame(include_bytes!("../../../../assets/pixel_office/floors/floor_8.png"))?,
        ];

        // Wall sheet is 64×128 — slice as 16×16 px tiles (4 cols × 8 rows).
        let walls = SpriteSheet::from_bytes(
            include_bytes!("../../../../assets/pixel_office/walls/wall_0.png"),
            16,
            16,
        )?;

        Ok(Self { characters, floors, walls })
    }
}

/// Frame column constants for character animations.
///
/// The sprite sheet has 7 frames per direction row (columns 0–6):
/// - 0: walk frame 0
/// - 1: walk frame 1 / idle (feet together)
/// - 2: walk frame 2
/// - 3: typing frame 0
/// - 4: typing frame 1
/// - 5: reading frame 0
/// - 6: reading frame 1
pub mod char_frames {
    pub const IDLE: usize = 1;
    pub const WALK: [usize; 4] = [0, 1, 2, 1];
    pub const TYPE_ANIM: [usize; 2] = [3, 4];
    pub const READ: [usize; 2] = [5, 6];
}

/// Direction row constants for character sprite sheets.
///
/// The PNG has 3 rows: DOWN (0), UP (1), RIGHT (2).
/// LEFT uses the RIGHT row — horizontal flipping is deferred to a later phase.
pub mod char_rows {
    pub const DOWN: usize = 0;
    pub const UP: usize = 1;
    pub const RIGHT: usize = 2;
    pub const LEFT: usize = 2; // uses RIGHT row until flip is implemented
}
