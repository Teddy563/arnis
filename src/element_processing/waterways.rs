//! Line waterways (OSM `waterway=river/stream/canal/...`). These are drawn as a flat ribbon of
//! surface water during element processing (like upstream Arnis) - they sit AT the terrain level,
//! so the floating-veg sweep clears grass off them and they never sink below the land. The depth
//! carving is left to the AREA-water path (ESA LC_WATER + `natural=water` polygons), which is what
//! the big rivers/lakes already use; trying to carve a bowl into a thin line just grooved the wide
//! water and broke the sweep, so it was reverted.
//!
//! `compute_waterway_field` records the same footprint cells into a set that is folded into the
//! tree water-gate (`tree::in_water_mask`), so trees never root on a river line - the only thing
//! kept from the field experiment.

use std::collections::HashMap;

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::floodfill_cache::RoadMaskBitmap;
use crate::ground::Ground;
use crate::land_cover::LC_WATER;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::water_depth::carve_water_column_with_flags;
use crate::world_editor::WorldEditor;

/// Bank tolerance: water still fills a cell up to this many blocks above the segment water level
/// (avoids gaps on gentle slopes). Shared by the draw + the tree-gate field so they match.
const BANK_TOLERANCE: i32 = 2;

/// Channel width (blocks) by waterway type, before any `width` tag override. A touch wider than
/// upstream so thin streams read as a real channel, not a single line.
fn get_waterway_width(waterway_type: &str) -> i32 {
    match waterway_type {
        "river" => 10,
        "canal" => 8,
        "stream" => 5,
        "fairway" => 14,
        "flowline" => 3,
        "brook" => 4,
        "ditch" => 3,
        "drain" => 2,
        _ => 6,
    }
}

/// Parse the effective channel width for a waterway way (type default + optional `width` tag).
fn waterway_width(way: &ProcessedWay) -> i32 {
    let wt = way.tags.get("waterway").map(String::as_str).unwrap_or("");
    let mut width = get_waterway_width(wt);
    if let Some(ws) = way.tags.get("width") {
        width = ws
            .parse::<i32>()
            .unwrap_or_else(|_| ws.parse::<f32>().map(|f| f as i32).unwrap_or(width));
    }
    width
}

/// True if a waterway way is below grade (culvert/tunnel) and should not be drawn.
fn is_subgrade(way: &ProcessedWay) -> bool {
    matches!(
        way.tags.get("layer").map(|s| s.as_str()),
        Some("-1") | Some("-2") | Some("-3")
    )
}

pub fn generate_waterways(editor: &mut WorldEditor, element: &ProcessedWay) {
    if !element.tags.contains_key("waterway") || is_subgrade(element) {
        return;
    }
    let width = waterway_width(element);
    for nodes_pair in element.nodes.windows(2) {
        let prev_node = nodes_pair[0].xz();
        let current_node = nodes_pair[1].xz();
        // Flat water level for this segment (min of both endpoints).
        let seg_water_y = editor
            .get_water_level(prev_node.x, prev_node.z)
            .min(editor.get_water_level(current_node.x, current_node.z));
        let points = bresenham_line(prev_node.x, 0, prev_node.z, current_node.x, 0, current_node.z);
        for (bx, _, bz) in points {
            create_water_channel(editor, bx, bz, width, seg_water_y);
        }
    }
}

/// Draw a flat water channel of `width` centred on `(center_x, center_z)` at `flat_water_y`.
/// Surface-only (no depth), place-if-absent so the area-water carve can still deepen a lake the
/// channel runs into.
fn create_water_channel(
    editor: &mut WorldEditor,
    center_x: i32,
    center_z: i32,
    width: i32,
    flat_water_y: i32,
) {
    let half_width = width / 2;
    for x in (center_x - half_width - 1)..=(center_x + half_width + 1) {
        for z in (center_z - half_width - 1)..=(center_z + half_width + 1) {
            if (x - center_x).abs().max((z - center_z).abs()) > half_width + 1 {
                continue;
            }
            let ground_y = editor.get_ground_level(x, z);
            let water_y = if ground_y <= flat_water_y {
                Some(flat_water_y)
            } else if ground_y <= flat_water_y + BANK_TOLERANCE
                && !editor.block_exists_absolute(x, ground_y, z)
            {
                Some(ground_y)
            } else {
                None
            };
            if let Some(water_y) = water_y {
                editor.set_block_absolute(WATER, x, water_y, z, None, None);
                editor.set_block_absolute(
                    AIR,
                    x,
                    water_y + 1,
                    z,
                    Some(&[GRASS, WHEAT, CARROTS, POTATOES]),
                    None,
                );
            }
        }
    }
}

/// Line-waterway cells: `(x,z) -> (surface_y, depth)`. Block-resolution, so it catches narrow
/// channels the coarse ESA grid misses. Used both to keep trees off river lines (`contains`) and to
/// carve a shallow channel below the flat ribbon (`carve_waterway_region`).
pub struct WaterwayField {
    cells: HashMap<(i32, i32), (i32, i32)>,
}

impl WaterwayField {
    /// True if a line waterway carries water at this cell.
    pub fn contains(&self, x: i32, z: i32) -> bool {
        self.cells.contains_key(&(x, z))
    }
}

/// Hard cap on a channel carve (matches water_depth MAX_WATER_DEPTH so the carve never panics).
const MAX_CHANNEL_DEPTH: i32 = 6;

/// Shallow, scale-aware channel depth. The outer ring is a 1-block lip so the bank isn't a wall.
fn channel_depth(scale: f64, dist: i32, half_width: i32) -> i32 {
    let base = if scale < 0.3 {
        1
    } else if scale < 0.7 {
        2
    } else {
        3
    };
    if dist >= half_width {
        1.min(base)
    } else {
        base
    }
}

/// Record every line-waterway cell with its water surface + shallow carve depth. Pure function of
/// the elements + `Ground`, built once before tiling. The surface is `max(ground, seg_water_y)` so
/// it never sits BELOW the terrain (no sunken water); the carve only deepens the bed below it.
pub fn compute_waterway_field(
    elements: &[ProcessedElement],
    ground: &Ground,
    xzbbox: &XZBBox,
    scale: f64,
) -> WaterwayField {
    let off_x = xzbbox.min_x();
    let off_z = xzbbox.min_z();
    let water_level = |x: i32, z: i32| ground.water_level(XZPoint::new(x - off_x, z - off_z));
    let ground_level = |x: i32, z: i32| ground.level(XZPoint::new(x - off_x, z - off_z));

    let mut cells: HashMap<(i32, i32), (i32, i32)> = HashMap::new();
    for el in elements {
        let ProcessedElement::Way(way) = el else {
            continue;
        };
        if way.tags.get("waterway").map(String::as_str) == Some("dock") {
            continue;
        }
        if !way.tags.contains_key("waterway") || is_subgrade(way) {
            continue;
        }
        let half_width = waterway_width(way) / 2;
        for pair in way.nodes.windows(2) {
            let a = pair[0].xz();
            let b = pair[1].xz();
            let seg_water_y = water_level(a.x, a.z).min(water_level(b.x, b.z));
            for (bx, _, bz) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z) {
                for x in (bx - half_width - 1)..=(bx + half_width + 1) {
                    for z in (bz - half_width - 1)..=(bz + half_width + 1) {
                        let dist = (x - bx).abs().max((z - bz).abs());
                        if dist > half_width + 1 {
                            continue;
                        }
                        let gy = ground_level(x, z);
                        if gy > seg_water_y + BANK_TOLERANCE {
                            continue;
                        }
                        // Surface never below terrain -> no sunken water.
                        let surface = gy.max(seg_water_y);
                        // Carve down to the terrain floor (surface - gy) PLUS the shallow channel
                        // depth, so water never floats over a dip and the floating-veg sweep (which
                        // probes ground level) always finds the column. Capped so the carve is safe.
                        let depth = (channel_depth(scale, dist, half_width) + (surface - gy))
                            .min(MAX_CHANNEL_DEPTH);
                        cells
                            .entry((x, z))
                            .and_modify(|e| {
                                if surface < e.0 {
                                    e.0 = surface;
                                }
                                if depth > e.1 {
                                    e.1 = depth;
                                }
                            })
                            .or_insert((surface, depth));
                    }
                }
            }
        }
    }
    WaterwayField { cells }
}

/// Deepen each recorded line-waterway cell into a real (shallow) channel, AFTER ground generation.
/// Skips cells that are already ESA `LC_WATER` (the area-water carve owns those - so a line running
/// through a lake never grooves it) and road/bridge decks. Reuses the shared
/// `carve_water_column_with_flags`, which lays the varied SAND/GRAVEL/DIRT/CLAY bed and (since the
/// blacklist there) never cuts an overhanging tree. Vertical-only writes => tile/seam-safe.
#[allow(clippy::too_many_arguments)]
pub fn carve_waterway_region(
    editor: &mut WorldEditor,
    field: &WaterwayField,
    ground: &Ground,
    xzbbox: &XZBBox,
    road_mask: &RoadMaskBitmap,
    iter_min_x: i32,
    iter_max_x: i32,
    iter_min_z: i32,
    iter_max_z: i32,
) {
    let off_x = xzbbox.min_x();
    let off_z = xzbbox.min_z();
    for (&(x, z), &(surface, depth)) in &field.cells {
        if x < iter_min_x || x > iter_max_x || z < iter_min_z || z > iter_max_z {
            continue;
        }
        if road_mask.contains(x, z) {
            continue;
        }
        // Let the area-water carve own LC_WATER cells (a line crossing a lake must not groove it).
        if ground.cover_class(XZPoint::new(x - off_x, z - off_z)) == LC_WATER {
            continue;
        }
        let near_bridge = (-2..=2).any(|dx: i32| {
            (-2..=2).any(|dz: i32| !(dx == 0 && dz == 0) && road_mask.contains(x + dx, z + dz))
        });
        carve_water_column_with_flags(editor, x, z, surface, depth, near_bridge, depth);
    }
}
