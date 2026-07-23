//! Bundled rock formations (andesite/tuff), scattered in small numbers on farmland.
//!
//! Reuses the generic Sponge stamping engine. Placement anchors are sampled from the
//! stable field-cell list and rotated by a position-derived hash, so the same world
//! tile resolves identically from any tile → seam-safe.

use std::sync::OnceLock;

use super::schematic::{load_structure, scatter_pool, StructureSchematic};
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

/// Hard cap so a huge field never fills with rocks.
const CAP: usize = 60;

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

/// Scatter rocks across a farmland field at random rotations, evenly distributed.
///
/// `density` is rocks per 512×512 region of field area; `0` disables the family.
pub fn scatter_rocks(editor: &mut WorldEditor, cells: &[(i32, i32)], density: u8) {
    scatter_pool(editor, cells, rocks(), density, CAP, false, 0x00A5_1C3D);
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
