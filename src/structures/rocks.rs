//! Bundled rock formations (andesite/tuff), scattered in small numbers on farmland.
//!
//! Reuses the generic Sponge stamping engine. Placement anchors are sampled from the
//! stable field-cell list and rotated by a position-derived hash, so the same world
//! tile resolves identically from any tile → seam-safe.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;

static ROCK_BYTES: [&[u8]; 8] = [
    include_bytes!("../../assets/structures/rock1.schem"),
    include_bytes!("../../assets/structures/rock2.schem"),
    include_bytes!("../../assets/structures/rock3.schem"),
    include_bytes!("../../assets/structures/rock4.schem"),
    include_bytes!("../../assets/structures/rock5.schem"),
    include_bytes!("../../assets/structures/rock6.schem"),
    include_bytes!("../../assets/structures/rock7.schem"),
    include_bytes!("../../assets/structures/rock8.schem"),
];

/// Minimum gap between two rocks (also their rough footprint).
const SPACING: i32 = 10;
/// Hard cap so a huge field never fills with rocks.
const CAP: u64 = 50;

fn rocks() -> &'static [StructureSchematic] {
    static CELL: OnceLock<Vec<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| {
        ROCK_BYTES
            .iter()
            .filter_map(|b| match load_structure(b) {
                Ok(s) => Some(s.base_anchored()),
                Err(e) => {
                    eprintln!("rock schem load failed: {e}");
                    None
                }
            })
            .collect()
    })
}

/// Scatter rocks across a farmland field at random rotations.
///
/// `density` is a relative amount (rocks per ~3000 cells); `0` disables the family.
pub fn scatter_rocks(editor: &mut WorldEditor, cells: &[(i32, i32)], density: u8) {
    if density == 0 {
        return;
    }
    let pool = rocks();
    if pool.is_empty() {
        return;
    }
    let n = cells.len();
    if n == 0 {
        return;
    }
    let target = ((n as u64 * density as u64) / 3000).clamp(1, CAP) as usize;
    let mut placed: Vec<(i32, i32)> = Vec::new();
    let max_attempts = target as u32 * 8 + 8;
    let mut t: u32 = 0;
    while placed.len() < target && t < max_attempts {
        let h = coord_hash(t as i32 + 1, (n as i32) ^ 0x00A5_1C3D);
        t += 1;
        let (ax, az) = cells[(h % n as u64) as usize];
        if editor.is_lc_water(ax, az) {
            continue;
        }
        if placed
            .iter()
            .any(|&(px, pz)| (px - ax).abs() < SPACING && (pz - az).abs() < SPACING)
        {
            continue;
        }
        let schem = &pool[((h >> 11) as usize) % pool.len()];
        let rot = ((h >> 5) & 3) as u8;
        let base_y = editor.get_absolute_y(ax, 1, az);
        place_structure(editor, schem, ax, az, base_y, rot, None);
        placed.push((ax, az));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rock_assets_parse() {
        assert_eq!(rocks().len(), 8, "all 8 rock schems should parse");
        for r in rocks() {
            assert!(!r.voxels.is_empty(), "rock parsed to zero voxels");
        }
    }
}
