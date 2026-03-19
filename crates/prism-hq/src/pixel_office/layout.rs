use std::collections::HashSet;

use serde::Deserialize;

use super::characters::Direction;

// ── JSON deserialization types ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct LayoutJson {
    cols: usize,
    rows: usize,
    tiles: Vec<u8>,
    furniture: Vec<FurnitureJson>,
}

#[derive(Deserialize)]
struct FurnitureJson {
    uid: String,
    #[serde(rename = "type")]
    furniture_type: String,
    col: i32,
    row: i32,
}

// ── public types ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct FurniturePlacement {
    pub uid: String,
    /// Base asset ID (e.g. "DESK_FRONT"). Never contains ":left".
    pub asset_id: String,
    /// True for ":left" mirrored variants.
    pub mirror: bool,
    pub col: i32,
    pub row: i32,
}

#[derive(Clone, Debug)]
pub struct Zone {
    pub col_min: i32,
    pub col_max: i32,
    pub row_min: i32,
    pub row_max: i32,
}

impl Zone {
    pub fn contains(&self, col: i32, row: i32) -> bool {
        col >= self.col_min
            && col <= self.col_max
            && row >= self.row_min
            && row <= self.row_max
    }

    /// Collect all walkable tiles within this zone.
    pub fn tiles_in(&self, walkable: &HashSet<(i32, i32)>) -> Vec<(i32, i32)> {
        walkable
            .iter()
            .filter(|&&(c, r)| self.contains(c, r))
            .copied()
            .collect()
    }
}

#[derive(Clone, Debug)]
pub struct OfficeZones {
    pub library: Zone,
    pub computer: Zone,
    pub meeting: Zone,
    pub lounge: Zone,
}

#[derive(Clone, Debug)]
pub struct Station {
    pub tile: (i32, i32),
    pub face_dir: Direction,
    /// UID of the linked PC furniture (for desk stations).
    pub furniture_uid: Option<String>,
    pub occupied_by: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StationRegistry {
    pub desk_seats: Vec<Station>,
    pub library_spots: Vec<Station>,
    pub lounge_spots: Vec<Station>,
    pub meeting_spots: Vec<Station>,
}

impl StationRegistry {
    /// Claim the first free station in a list, returning its tile.
    pub fn claim(stations: &mut Vec<Station>, agent_name: &str) -> Option<(i32, i32)> {
        if let Some(station) = stations.iter_mut().find(|s| s.occupied_by.is_none()) {
            station.occupied_by = Some(agent_name.to_string());
            Some(station.tile)
        } else {
            None
        }
    }

    /// Release any station in a list that is occupied by `agent_name`.
    pub fn release(stations: &mut Vec<Station>, agent_name: &str) {
        for station in stations.iter_mut() {
            if station.occupied_by.as_deref() == Some(agent_name) {
                station.occupied_by = None;
            }
        }
    }

    /// Release all stations across all categories for `agent_name`.
    pub fn release_all(&mut self, agent_name: &str) {
        Self::release(&mut self.desk_seats, agent_name);
        Self::release(&mut self.library_spots, agent_name);
        Self::release(&mut self.lounge_spots, agent_name);
        Self::release(&mut self.meeting_spots, agent_name);
    }
}

#[derive(Clone, Debug)]
pub struct OfficeLayout {
    pub cols: usize,
    pub rows: usize,
    pub tiles: Vec<u8>,
    pub furniture: Vec<FurniturePlacement>,
    pub walkable_tiles: HashSet<(i32, i32)>,
    pub zones: OfficeZones,
    pub stations: StationRegistry,
}

// ── loader ─────────────────────────────────────────────────────────────────────

impl OfficeLayout {
    pub fn load() -> Self {
        let json_str =
            include_str!("../../../../assets/pixel_office/default-layout-1.json");
        let layout_json: LayoutJson =
            serde_json::from_str(json_str).expect("default-layout-1.json is always valid");

        // Parse furniture placements.
        let furniture: Vec<FurniturePlacement> = layout_json
            .furniture
            .iter()
            .map(|f| {
                let (asset_id, mirror) = if f.furniture_type.ends_with(":left") {
                    (
                        f.furniture_type[..f.furniture_type.len() - 5].to_string(),
                        true,
                    )
                } else {
                    (f.furniture_type.clone(), false)
                };
                FurniturePlacement {
                    uid: f.uid.clone(),
                    asset_id,
                    mirror,
                    col: f.col,
                    row: f.row,
                }
            })
            .collect();

        // Walkable = any tile that is not 0 (wall) and not 255 (void).
        let mut walkable_tiles = HashSet::new();
        for row in 0..layout_json.rows {
            for col in 0..layout_json.cols {
                let tile = layout_json.tiles[row * layout_json.cols + col];
                if tile != 0 && tile != 255 {
                    walkable_tiles.insert((col as i32, row as i32));
                }
            }
        }

        // Zone definitions derived from the layout geometry.
        // Left room (floor type 7):  cols 1–9, rows 11–20.
        // Right room (floor 1/9):    cols 11–18, rows 11–20.
        let zones = OfficeZones {
            // Top of left room — in front of bookshelves on the north wall.
            library: Zone { col_min: 1, col_max: 9, row_min: 11, row_max: 11 },
            // Desk row in left room.
            computer: Zone { col_min: 1, col_max: 9, row_min: 12, row_max: 14 },
            // Table + chairs in lower left room.
            meeting: Zone { col_min: 1, col_max: 9, row_min: 15, row_max: 20 },
            // Entire right room (sofas, coffee table).
            lounge: Zone { col_min: 11, col_max: 18, row_min: 11, row_max: 20 },
        };

        // Station registry — hardcoded seats tied to known furniture positions.
        // Desk stations: character stands one row south of desk, facing Up.
        // PC UIDs link each desk to its monitor.
        let stations = StationRegistry {
            library_spots: vec![
                Station {
                    tile: (2, 11),
                    face_dir: Direction::Up,
                    furniture_uid: Some("f-1773354700513-f1zs".into()),
                    occupied_by: None,
                },
                Station {
                    tile: (3, 11),
                    face_dir: Direction::Up,
                    furniture_uid: Some("f-1773354700513-f1zs".into()),
                    occupied_by: None,
                },
                Station {
                    tile: (7, 11),
                    face_dir: Direction::Up,
                    furniture_uid: Some("f-1773354693077-f7aj".into()),
                    occupied_by: None,
                },
                Station {
                    tile: (8, 11),
                    face_dir: Direction::Up,
                    furniture_uid: Some("f-1773354693077-f7aj".into()),
                    occupied_by: None,
                },
            ],
            desk_seats: vec![
                // Left desk (col 2): PC uid "f-1773356782055-vp70" at (3, 12).
                Station {
                    tile: (2, 13),
                    face_dir: Direction::Up,
                    furniture_uid: Some("f-1773356782055-vp70".into()),
                    occupied_by: None,
                },
                Station {
                    tile: (3, 13),
                    face_dir: Direction::Up,
                    furniture_uid: Some("f-1773356782055-vp70".into()),
                    occupied_by: None,
                },
                // Right desk (col 6): PC uid "f-1773356781294-b69z" at (7, 12).
                Station {
                    tile: (6, 13),
                    face_dir: Direction::Up,
                    furniture_uid: Some("f-1773356781294-b69z".into()),
                    occupied_by: None,
                },
                Station {
                    tile: (7, 13),
                    face_dir: Direction::Up,
                    furniture_uid: Some("f-1773356781294-b69z".into()),
                    occupied_by: None,
                },
            ],
            lounge_spots: vec![
                Station {
                    tile: (12, 14),
                    face_dir: Direction::Right,
                    furniture_uid: None,
                    occupied_by: None,
                },
                Station {
                    tile: (17, 14),
                    face_dir: Direction::Left,
                    furniture_uid: None,
                    occupied_by: None,
                },
                Station {
                    tile: (13, 15),
                    face_dir: Direction::Up,
                    furniture_uid: None,
                    occupied_by: None,
                },
                Station {
                    tile: (14, 15),
                    face_dir: Direction::Up,
                    furniture_uid: None,
                    occupied_by: None,
                },
                Station {
                    tile: (15, 15),
                    face_dir: Direction::Up,
                    furniture_uid: None,
                    occupied_by: None,
                },
                Station {
                    tile: (14, 17),
                    face_dir: Direction::Down,
                    furniture_uid: None,
                    occupied_by: None,
                },
                Station {
                    tile: (15, 17),
                    face_dir: Direction::Down,
                    furniture_uid: None,
                    occupied_by: None,
                },
                Station {
                    tile: (14, 18),
                    face_dir: Direction::Down,
                    furniture_uid: None,
                    occupied_by: None,
                },
            ],
            meeting_spots: vec![
                Station {
                    tile: (3, 16),
                    face_dir: Direction::Right,
                    furniture_uid: None,
                    occupied_by: None,
                },
                Station {
                    tile: (3, 17),
                    face_dir: Direction::Right,
                    furniture_uid: None,
                    occupied_by: None,
                },
                Station {
                    tile: (3, 18),
                    face_dir: Direction::Right,
                    furniture_uid: None,
                    occupied_by: None,
                },
                Station {
                    tile: (7, 16),
                    face_dir: Direction::Left,
                    furniture_uid: None,
                    occupied_by: None,
                },
                Station {
                    tile: (7, 17),
                    face_dir: Direction::Left,
                    furniture_uid: None,
                    occupied_by: None,
                },
                Station {
                    tile: (7, 18),
                    face_dir: Direction::Left,
                    furniture_uid: None,
                    occupied_by: None,
                },
            ],
        };

        Self {
            cols: layout_json.cols,
            rows: layout_json.rows,
            tiles: layout_json.tiles,
            furniture,
            walkable_tiles,
            zones,
            stations,
        }
    }
}
