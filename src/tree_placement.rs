#![allow(dead_code)] // wired into data_processing in the next step.
//! Schematic tree placement pass. A tile-invariant, constant min-spacing grid over the
//! ESA tree-cover mask: each grid cell holds at most one tree (so trunks never touch),
//! with species chosen by geography, size by scale, variant + rotation by position seed.
//! Trees are skipped on roads, water, and building footprints, and stamped place-if-absent
//! so canopies never clip a building. Every choice is a pure function of position, so a
//! tree renders identically from any tile (seam-safe; the Meld seam buffer covers canopy
//! overhang into neighbouring tiles).

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::floodfill_cache::{CoordinateBitmap, RoadMaskBitmap};
use crate::ground::Ground;
use crate::ground_generation::value_noise_01;
use crate::land_cover::{coord_hash, LC_TREE_COVER, LC_WATER};
use crate::schematic::place_schematic_tree;
use crate::tree_library::{TreeLibrary, TreeSize};
use crate::water_depth::BigWaterField;
use crate::world_editor::WorldEditor;

/// Minimum trunk spacing in blocks (the grid cell size). Constant across scales so the
/// spacing in the actual generation is the same at 1:1 and 1:10.
const MIN_TRUNK_SPACING: i32 = 4;
/// Forest-density noise scale: low values are clearings, high values are thickets.
const DENSITY_SCALE: i32 = 32;
/// Below this density-noise value a cell is a clearing (no tree).
const CLEARING_CUTOFF: f64 = 0.28;
/// Patch noise scale for species stands (birch groves, dark-oak pockets).
const PATCH_SCALE: i32 = 96;

/// Choose a species key (matching the library's folders) from geography + position seed.
/// Oak-dominant lowland with birch groves and rare dark-oak pockets; spruce on steep or
/// boreal ground; jungle in the tropics; swamp oak next to water.
fn pick_species(px: i32, pz: i32, slope: i32, latitude: f64) -> &'static str {
    let abs_lat = latitude.abs();
    // Boreal or montane: spruce.
    if abs_lat > 55.0 || slope > 4 {
        return "spruce";
    }
    // Tropical: jungle.
    if abs_lat < 23.0 {
        return "jungle";
    }
    // Temperate: oak matrix with birch groves + rare dark-oak pockets (patch + sprinkle).
    let patch = value_noise_01(px, pz, PATCH_SCALE);
    let roll = coord_hash(px + 7, pz + 13) % 100;
    if patch > 0.72 {
        if roll < 70 {
            "birch"
        } else {
            "oak"
        }
    } else if patch < 0.12 {
        if roll < 55 {
            "dark_oak"
        } else {
            "oak"
        }
    } else if roll < 12 {
        "birch"
    } else if roll < 18 {
        "dark_oak"
    } else {
        "oak"
    }
}

/// Choose a size tier from the scale band (medium common at full scale; small-only on
/// very small maps). Position-seeded.
fn pick_size(px: i32, pz: i32, scale: f64) -> TreeSize {
    let roll = coord_hash(px + 101, pz + 233) % 100;
    if scale >= 0.5 {
        if roll < 20 {
            TreeSize::Small
        } else if roll < 80 {
            TreeSize::Medium
        } else {
            TreeSize::Big
        }
    } else if scale >= 0.2 {
        if roll < 60 {
            TreeSize::Small
        } else {
            TreeSize::Medium
        }
    } else {
        TreeSize::Small
    }
}

/// Pick a library entry index for (species, size), falling back across sizes then to oak.
fn pick_variant(
    lib: &TreeLibrary,
    species: &str,
    size: TreeSize,
    px: i32,
    pz: i32,
) -> Option<usize> {
    let h = coord_hash(px + 313, pz + 727) as usize;
    let order = [size, TreeSize::Medium, TreeSize::Small, TreeSize::Big];
    for sp in [species, "oak"] {
        for &sz in &order {
            let c = lib.candidates(sp, sz);
            if !c.is_empty() {
                return Some(c[h % c.len()]);
            }
        }
    }
    None
}

/// Canopy halo (blocks) by which each tile expands its placement bounds so a tree near a
/// tile edge is stamped from both adjacent tiles (place-if-absent makes the overlap
/// identical, so the canopy is completed across the seam). Must not exceed the tile editor
/// halo; covers the widest tree footprint radius in the pack.
pub const TREE_TILE_HALO: i32 = 8;

/// Place schematic trees over ESA tree cover within the inclusive `[iter_*]` block range
/// (intersected with `xzbbox`). Returns the count stamped. Every choice is a pure function
/// of position, so a tree is identical from any tile; per-tile callers expand their bounds
/// by `TREE_TILE_HALO` so a boundary tree's canopy is written from both sides.
#[allow(clippy::too_many_arguments)]
pub fn place_schematic_trees_region(
    editor: &mut WorldEditor,
    ground: &Ground,
    lib: &TreeLibrary,
    road_mask: &RoadMaskBitmap,
    big_water_field: &BigWaterField,
    building_footprints: &CoordinateBitmap,
    xzbbox: &XZBBox,
    scale: f64,
    latitude: f64,
    iter_min_x: i32,
    iter_max_x: i32,
    iter_min_z: i32,
    iter_max_z: i32,
) -> u64 {
    let off_x = xzbbox.min_x();
    let off_z = xzbbox.min_z();
    let x0 = iter_min_x.max(xzbbox.min_x());
    let x1 = iter_max_x.min(xzbbox.max_x());
    let z0 = iter_min_z.max(xzbbox.min_z());
    let z1 = iter_max_z.min(xzbbox.max_z());
    let s = MIN_TRUNK_SPACING;

    let mut placed = 0u64;
    // Global grid (aligned to the world, not the tile) so cells are identical across tiles.
    let mut cx = x0.div_euclid(s) * s;
    while cx <= x1 {
        let mut cz = z0.div_euclid(s) * s;
        while cz <= z1 {
            let (ccx, ccz) = (cx, cz);
            cz += s;
            // One jittered candidate point per cell.
            let h = coord_hash(ccx, ccz);
            let px = ccx + (h % s as u64) as i32;
            let pz = ccz + ((h / 7) % s as u64) as i32;
            if px < x0 || px > x1 || pz < z0 || pz > z1 {
                continue;
            }
            let coord = XZPoint::new(px - off_x, pz - off_z);
            // Forest mask: ESA tree cover only.
            if ground.cover_class(coord) != LC_TREE_COVER {
                continue;
            }
            // Clearings.
            if value_noise_01(px, pz, DENSITY_SCALE) < CLEARING_CUTOFF {
                continue;
            }
            // Avoidance: roads, buildings, water.
            if road_mask.contains(px, pz)
                || building_footprints.contains(px, pz)
                || big_water_field.depth_at(px, pz) > 0
                || ground.cover_class(coord) == LC_WATER
            {
                continue;
            }
            let slope = ground.slope(coord);
            let species = pick_species(px, pz, slope, latitude);
            let size = pick_size(px, pz, scale);
            let Some(idx) = pick_variant(lib, species, size, px, pz) else {
                continue;
            };
            let rot = (coord_hash(px ^ 0x5bd1, pz ^ 0x9e37) % 4) as u8;
            let base_y = ground.level(coord);
            place_schematic_tree(editor, &lib.entries[idx].schem, px, pz, base_y, rot);
            placed += 1;
        }
        cx += s;
    }
    placed
}

/// Whole-bbox pass (the non-tiled path). Stamps every cell once and logs the count.
#[allow(clippy::too_many_arguments)]
pub fn place_schematic_trees_pass(
    editor: &mut WorldEditor,
    ground: &Ground,
    lib: &TreeLibrary,
    road_mask: &RoadMaskBitmap,
    big_water_field: &BigWaterField,
    building_footprints: &CoordinateBitmap,
    xzbbox: &XZBBox,
    scale: f64,
    latitude: f64,
) {
    let placed = place_schematic_trees_region(
        editor,
        ground,
        lib,
        road_mask,
        big_water_field,
        building_footprints,
        xzbbox,
        scale,
        latitude,
        xzbbox.min_x(),
        xzbbox.max_x(),
        xzbbox.min_z(),
        xzbbox.max_z(),
    );
    println!("Tree placement: stamped {placed} schematic trees");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn species_is_deterministic_and_climate_aware() {
        // Boreal / steep -> spruce; tropical -> jungle.
        assert_eq!(pick_species(10, 20, 6, 45.0), "spruce");
        assert_eq!(pick_species(10, 20, 0, 70.0), "spruce");
        assert_eq!(pick_species(10, 20, 0, 5.0), "jungle");
        // Temperate is one of the lowland set, and stable for the same coord.
        let a = pick_species(123, 456, 0, 45.0);
        let b = pick_species(123, 456, 0, 45.0);
        assert_eq!(a, b);
        assert!(matches!(a, "oak" | "birch" | "dark_oak"));
    }

    #[test]
    fn size_respects_scale_band() {
        // Small maps never pick Big.
        for i in 0..200 {
            assert_ne!(pick_size(i, i * 3, 0.1), TreeSize::Big);
            assert_ne!(pick_size(i, i * 3, 0.3), TreeSize::Big);
        }
    }
}
