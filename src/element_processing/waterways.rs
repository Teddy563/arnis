//! Line waterways (OSM `waterway=river/stream/canal/...`). Unlike water AREAS (polygons / ESA
//! LC_WATER), line waterways are not in the land-cover grid, so they get their own field + carve.
//!
//! Two phases, mirroring the area-water machinery:
//!  1. `compute_waterway_field` rasterizes every waterway line (Bresenham + width) ONCE, before the
//!     parallel tile loop, into a per-cell `(water_y, depth)` map. Only cells that actually carry
//!     water (terrain at/below the segment water level + a bank tolerance) are recorded; the cross
//!     section is always >= 3 wide with a sloped bowl floor (>= 1 deep everywhere, deeper toward
//!     the centre). Union-to-deepest on overlap so confluences are deterministic.
//!  2. `carve_waterway_region` runs AFTER ground generation (same sites as the LC-water carve) and
//!     calls the shared `carve_water_column_with_flags`, so a line river gets the identical
//!     SAND/GRAVEL/DIRT/CLAY bed the lakes get - over filled terrain, so no raw stone slot.
//!
//! The field is a pure function of the element geometry + `Ground`, shared immutably across tiles
//! like `big_water_field`/`road_mask`, so it is seam-safe.

use std::collections::HashMap;

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::bresenham::bresenham_line;
use crate::floodfill_cache::RoadMaskBitmap;
use crate::ground::Ground;
use crate::osm_parser::ProcessedElement;
use crate::water_depth::{carve_water_column_with_flags, SMALL_SCALE_MAX_DEPTH};
use crate::world_editor::WorldEditor;

/// Channel width (blocks) by waterway type, before any `width` tag override.
fn get_waterway_width(waterway_type: &str) -> i32 {
    match waterway_type {
        "river" => 8,
        "canal" => 6,
        "stream" => 3,
        "fairway" => 12,
        "flowline" => 2,
        "brook" => 2,
        "ditch" => 2,
        "drain" => 1,
        _ => 4,
    }
}

/// Per-cell channel record: the flat water-surface Y + the carve depth at that cell.
pub struct WaterwayField {
    cells: HashMap<(i32, i32), (i32, i32)>, // (x,z) -> (water_y, depth)
}

impl WaterwayField {
    /// True if a line waterway carries water at this cell (used to keep trees off rivers).
    pub fn contains(&self, x: i32, z: i32) -> bool {
        self.cells.contains_key(&(x, z))
    }
}

/// Rasterize all line waterways into a `WaterwayField`. Pure function of the elements + `Ground`.
pub fn compute_waterway_field(
    elements: &[ProcessedElement],
    ground: &Ground,
    xzbbox: &XZBBox,
) -> WaterwayField {
    const BANK_TOLERANCE: i32 = 2;
    let off_x = xzbbox.min_x();
    let off_z = xzbbox.min_z();
    let water_level = |x: i32, z: i32| ground.water_level(XZPoint::new(x - off_x, z - off_z));
    let ground_level = |x: i32, z: i32| ground.level(XZPoint::new(x - off_x, z - off_z));

    let mut cells: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
    for el in elements {
        let ProcessedElement::Way(way) = el else {
            continue;
        };
        let Some(wt) = way.tags.get("waterway") else {
            continue;
        };
        if wt == "dock" {
            continue; // docks are handled as water areas
        }
        // Skip below-grade / culverted waterways (matches the old generate_waterways).
        if matches!(
            way.tags.get("layer").map(|s| s.as_str()),
            Some("-1") | Some("-2") | Some("-3")
        ) {
            continue;
        }
        let mut width = get_waterway_width(wt);
        if let Some(ws) = way.tags.get("width") {
            width = ws.parse::<i32>().unwrap_or_else(|_| {
                ws.parse::<f32>().map(|f| f as i32).unwrap_or(width)
            });
        }
        // >= 3 wide always (no 1-cell underwater line): half_width >= 1.
        let half_width = (width / 2).max(1);

        for pair in way.nodes.windows(2) {
            let a = pair[0].xz();
            let b = pair[1].xz();
            // Flat surface per segment (min of endpoints) - same as the old per-segment level.
            let seg_water_y = water_level(a.x, a.z).min(water_level(b.x, b.z));
            for (bx, _, bz) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z) {
                for x in (bx - half_width)..=(bx + half_width) {
                    for z in (bz - half_width)..=(bz + half_width) {
                        let dist = (x - bx).abs().max((z - bz).abs());
                        if dist > half_width {
                            continue;
                        }
                        // Only where the channel actually holds water (terrain not above it).
                        if ground_level(x, z) > seg_water_y + BANK_TOLERANCE {
                            continue;
                        }
                        // Bowl floor: >= 1 deep across the whole >=3-wide section, deeper toward
                        // the centre, capped. inner = how far in from the channel edge.
                        let inner = half_width - dist;
                        let depth = (1 + inner).min(SMALL_SCALE_MAX_DEPTH);
                        cells
                            .entry((x, z))
                            .and_modify(|e| {
                                if depth > e.1 {
                                    e.1 = depth;
                                }
                                if seg_water_y < e.0 {
                                    e.0 = seg_water_y;
                                }
                            })
                            .or_insert((seg_water_y, depth));
                    }
                }
            }
        }
    }
    WaterwayField { cells }
}

/// Carve every recorded waterway cell within `[iter_min..=iter_max]`, reusing the shared
/// `carve_water_column_with_flags` so line rivers get the same bed/banks as area water. Runs
/// AFTER ground generation (the bed force-overwrites the filled terrain - no stone slot). Writes
/// are vertical-only per `(x,z)`, so the per-tile bounds make it eviction-/seam-safe.
pub fn carve_waterway_region(
    editor: &mut WorldEditor,
    field: &WaterwayField,
    road_mask: &RoadMaskBitmap,
    iter_min_x: i32,
    iter_max_x: i32,
    iter_min_z: i32,
    iter_max_z: i32,
) {
    for (&(x, z), &(water_y, depth)) in &field.cells {
        if x < iter_min_x || x > iter_max_x || z < iter_min_z || z > iter_max_z {
            continue;
        }
        // Keep bridge/causeway decks; water still flows under them via the deck being road_mask.
        if road_mask.contains(x, z) {
            continue;
        }
        let near_bridge = (-2..=2).any(|dx: i32| {
            (-2..=2).any(|dz: i32| !(dx == 0 && dz == 0) && road_mask.contains(x + dx, z + dz))
        });
        carve_water_column_with_flags(editor, x, z, water_y, depth, near_bridge, depth);
    }
}
