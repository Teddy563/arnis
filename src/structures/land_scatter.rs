//! Chunk-based rock/bush scatter over UNTAGGED land (satellite cropland/grassland).
//!
//! Mirrors the field scatter: every 16×16 chunk rolls a percentage for one rock-or-bush
//! at a jittered spot, but eligibility comes from ESA land cover instead of an OSM
//! polygon — this is what fills the "missing data" plains. Skips roads, buildings,
//! villages and water. Purely position-hashed → identical across tile seams.

use super::schematic::place_scatter_piece;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::floodfill_cache::BuildingFootprintBitmap;
use crate::ground::Ground;
use crate::land_cover::{self, coord_hash};
use crate::world_editor::WorldEditor;

/// Percent of chunks that receive one piece.
const PCT: u64 = 20;
const SALT: i32 = 0x00E1_57A7;

#[allow(clippy::too_many_arguments)]
pub fn scatter_untagged_chunks(
    editor: &mut WorldEditor,
    ground: &Ground,
    xzbbox: &XZBBox,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    road_mask: &BuildingFootprintBitmap,
    building_footprints: &BuildingFootprintBitmap,
    residential_footprint: &BuildingFootprintBitmap,
    rocks_on: bool,
    bushes_on: bool,
) {
    if (!rocks_on && !bushes_on) || !ground.has_land_cover() {
        return;
    }
    for cz in (min_z >> 4)..=(max_z >> 4) {
        for cx in (min_x >> 4)..=(max_x >> 4) {
            let h = coord_hash(cx ^ SALT, cz.wrapping_mul(31) ^ SALT);
            if h % 100 >= PCT {
                continue;
            }
            let bx = (cx << 4) + ((h >> 17) % 16) as i32;
            let bz = (cz << 4) + ((h >> 22) % 16) as i32;
            if bx < min_x || bx > max_x || bz < min_z || bz > max_z {
                continue;
            }
            let cover = ground.cover_class(XZPoint::new(bx - xzbbox.min_x(), bz - xzbbox.min_z()));
            if cover != land_cover::LC_CROPLAND && cover != land_cover::LC_GRASSLAND {
                continue;
            }
            if road_mask.contains(bx, bz)
                || building_footprints.contains(bx, bz)
                || residential_footprint.contains(bx, bz)
                || editor.is_lc_water(bx, bz)
            {
                continue;
            }
            place_scatter_piece(editor, bx, bz, rocks_on, bushes_on, h);
        }
    }
}
