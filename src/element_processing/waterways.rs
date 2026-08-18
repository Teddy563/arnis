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

/// Widest channel a `width=*` tag may ask for. Every renderer of a waterway walks its width
/// squared per centreline point, so an unbounded tag value hangs generation - and OSM does
/// carry mistyped widths (a `width=1000` on a stream is a survey unit error, not a river).
const MAX_WATERWAY_WIDTH: i32 = 128;

/// Effective channel width for a way (type default + optional `width` tag).
///
/// The tag is parsed as a float and the unit suffix dropped, so `width=3 m` reads as 3
/// rather than falling back to the type default. Values below 1, non-finite ones, and
/// anything unparseable fall back; the rest are clamped to `MAX_WATERWAY_WIDTH`.
fn waterway_width(way: &ProcessedWay) -> i32 {
    let wt = way.tags.get("waterway").map(String::as_str).unwrap_or("");
    let tagged = way
        .tags
        .get("width")
        .and_then(|s| s.trim().split(' ').next())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|w| w.is_finite())
        .map(|w| w.round() as i64);
    match tagged {
        Some(w) if w >= 1 => w.min(i64::from(MAX_WATERWAY_WIDTH)) as i32,
        _ => get_waterway_width(wt),
    }
}

/// False for `waterway=*` values that describe a structure or a point rather than a channel.
///
/// Outlining these draws a canal down the crest of a dam or along a weir, which is how a
/// barrage ends up with a river running across the top of it.
fn is_channel_waterway(waterway_type: &str) -> bool {
    !matches!(
        waterway_type,
        "dam"
            | "weir"
            | "lock_gate"
            | "waterfall"
            | "rapids"
            | "boatyard"
            | "fuel"
            | "dock"
            | "riverbank"
            | "water_point"
            | "turning_point"
            | "sluice_gate"
            | "fish_pass"
            | "security_lock"
            | "milestone"
            | "check_dam"
            | "floating_barrier"
    )
}

/// True if a waterway way is below grade (culvert/tunnel) and should not be drawn.
///
/// Covers any `tunnel=*` that is not an explicit negative, and any negative `layer`, not
/// just -1..-3: a culvert drawn as open water cuts a channel straight through its bank.
fn is_subgrade(way: &ProcessedWay) -> bool {
    if way
        .tags
        .get("tunnel")
        .is_some_and(|v| !matches!(v.as_str(), "no" | "0" | "false"))
    {
        return true;
    }
    way.tags
        .get("layer")
        .and_then(|l| l.trim().parse::<i32>().ok())
        .is_some_and(|l| l < 0)
}

/// Channel half-width, capped on small maps so a 1:10 stream/river stays a thin strip instead of a
/// fat ribbon. `create_water_channel` draws to `half_width + 1`, so hw=0 => 3 wide, hw=1 => 5 wide.
fn scaled_half_width(width: i32, scale: f64) -> i32 {
    let hw = (width / 2).max(0);
    if scale < 0.3 {
        0 // <= 3 wide at small scale (e.g. 1:10)
    } else if scale < 0.7 {
        hw.min(1) // <= 5 wide
    } else {
        hw
    }
}

/// The single test both the draw and the tree-water field use.
///
/// These two must agree cell for cell - the module contract is that a cell the draw fills
/// with water is a cell trees are kept off - so the decision lives in one place rather than
/// being spelled out twice and drifting.
fn is_drawable_waterway(way: &ProcessedWay) -> bool {
    let Some(waterway_type) = way.tags.get("waterway") else {
        return false;
    };
    is_channel_waterway(waterway_type) && !is_subgrade(way)
}

pub fn generate_waterways(editor: &mut WorldEditor, element: &ProcessedWay, scale: f64) {
    if !is_drawable_waterway(element) {
        return;
    }
    let half_width = scaled_half_width(waterway_width(element), scale);
    for nodes_pair in element.nodes.windows(2) {
        let prev_node = nodes_pair[0].xz();
        let current_node = nodes_pair[1].xz();
        let seg_water_y = editor
            .get_water_level(prev_node.x, prev_node.z)
            .min(editor.get_water_level(current_node.x, current_node.z));
        let points = bresenham_line(
            prev_node.x,
            0,
            prev_node.z,
            current_node.x,
            0,
            current_node.z,
        );
        for (bx, _, bz) in points {
            create_water_channel(editor, bx, bz, half_width, seg_water_y);
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
    half_width: i32,
    flat_water_y: i32,
) {
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
    scale: f64,
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
        // `dock` used to be excluded here alone; it is now one of the structure values
        // is_channel_waterway rejects, so both paths drop it for the same reason.
        if !is_drawable_waterway(way) {
            continue;
        }
        let half_width = scaled_half_width(waterway_width(way), scale);
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

#[cfg(test)]
mod waterway_gate_tests {
    use super::*;
    use std::collections::HashMap;

    fn way(tags: &[(&str, &str)]) -> ProcessedWay {
        ProcessedWay {
            id: 1,
            nodes: Vec::new(),
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<String, String>>(),
            unclipped_bounds: None,
            unclipped_polygon_area: None,
        }
    }

    /// A dam or weir outlined as a channel puts a river along the crest of the structure.
    #[test]
    fn structures_are_not_channels() {
        for kind in ["dam", "weir", "waterfall", "lock_gate", "dock", "check_dam"] {
            assert!(
                !is_drawable_waterway(&way(&[("waterway", kind)])),
                "waterway={kind} must not be drawn as a channel"
            );
        }
    }

    #[test]
    fn real_channels_still_draw() {
        for kind in ["river", "stream", "canal", "ditch", "drain", "brook"] {
            assert!(
                is_drawable_waterway(&way(&[("waterway", kind)])),
                "waterway={kind} must still be drawn"
            );
        }
    }

    /// A culvert drawn as open water cuts a channel straight through the bank it runs under.
    #[test]
    fn culverts_and_negative_layers_are_subgrade() {
        assert!(!is_drawable_waterway(&way(&[
            ("waterway", "stream"),
            ("tunnel", "culvert")
        ])));
        assert!(!is_drawable_waterway(&way(&[
            ("waterway", "stream"),
            ("tunnel", "yes")
        ])));
        // Deeper than the old -1..-3 window, which silently drew these.
        assert!(!is_drawable_waterway(&way(&[
            ("waterway", "stream"),
            ("layer", "-4")
        ])));
    }

    #[test]
    fn explicit_non_tunnels_and_positive_layers_still_draw() {
        for tags in [
            vec![("waterway", "stream"), ("tunnel", "no")],
            vec![("waterway", "stream"), ("layer", "0")],
            vec![("waterway", "stream"), ("layer", "1")],
        ] {
            assert!(is_drawable_waterway(&way(&tags)), "{tags:?} must draw");
        }
    }

    /// Width is walked squared per centreline point, so a mistyped tag hangs generation.
    #[test]
    fn width_tag_is_clamped_and_unit_suffixes_parse() {
        assert_eq!(
            waterway_width(&way(&[("waterway", "stream"), ("width", "4")])),
            4
        );
        assert_eq!(
            waterway_width(&way(&[("waterway", "stream"), ("width", "3 m")])),
            3
        );
        assert_eq!(
            waterway_width(&way(&[("waterway", "stream"), ("width", "100000")])),
            MAX_WATERWAY_WIDTH
        );
    }

    /// Anything unusable falls back to the type default rather than to zero.
    #[test]
    fn unusable_width_tags_fall_back_to_the_type_default() {
        let default = get_waterway_width("river");
        for bad in ["", "wide", "0", "-5", "nan", "inf"] {
            assert_eq!(
                waterway_width(&way(&[("waterway", "river"), ("width", bad)])),
                default,
                "width={bad:?} should fall back"
            );
        }
    }
}
