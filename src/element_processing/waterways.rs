//! Line waterways (OSM `waterway=river/stream/canal/...`). Drawn as a flat ribbon of surface water
//! (upstream Arnis behaviour): they sit AT the terrain level, so the floating-veg sweep clears grass
//! off them and they never sink below the land. Depth carving is left to the AREA-water path
//! (ESA LC_WATER + `natural=water` polygons) - line-river depth carving was tried several times and
//! always grooved the wide water / cut trees / floated, so it stays reverted.
//!
//! `compute_waterway_field` records the footprint cells into a set that is folded into the tree
//! water-gate (`tree::in_water_mask`), so trees never root on a river line.

use std::collections::HashSet;

use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;
use crate::osm_parser::{ProcessedElement, ProcessedWay};
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

/// Effective channel width for a way (type default + optional `width` tag).
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

/// The set of line-waterway cells (used only to keep trees off river lines). Block-resolution, so
/// it catches narrow channels the coarse ESA grid + the surface-Y water probe both miss.
pub struct WaterwayField {
    cells: HashSet<(i32, i32)>,
}

impl WaterwayField {
    /// True if a line waterway carries water at this cell.
    pub fn contains(&self, x: i32, z: i32) -> bool {
        self.cells.contains(&(x, z))
    }
}

/// Record every cell a line waterway draws water into (mirrors `create_water_channel`'s footprint),
/// for the tree water-gate. Pure function of the elements + `Ground`, built once before tiling.
pub fn compute_waterway_field(
    elements: &[ProcessedElement],
    ground: &Ground,
    xzbbox: &XZBBox,
) -> WaterwayField {
    let off_x = xzbbox.min_x();
    let off_z = xzbbox.min_z();
    let water_level = |x: i32, z: i32| ground.water_level(XZPoint::new(x - off_x, z - off_z));
    let ground_level = |x: i32, z: i32| ground.level(XZPoint::new(x - off_x, z - off_z));

    let mut cells: HashSet<(i32, i32)> = HashSet::new();
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
                        if (x - bx).abs().max((z - bz).abs()) > half_width + 1 {
                            continue;
                        }
                        if ground_level(x, z) <= seg_water_y + BANK_TOLERANCE {
                            cells.insert((x, z));
                        }
                    }
                }
            }
        }
    }
    WaterwayField { cells }
}
