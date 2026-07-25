//! Bundled rock formations (andesite/tuff), scattered in small numbers on farmland.
//!
//! Reuses the generic Sponge stamping engine. Placement anchors are sampled from the
//! stable field-cell list and rotated by a position-derived hash, so the same world
//! tile resolves identically from any tile → seam-safe.

use std::sync::OnceLock;

use super::schematic::{load_structure, StructureSchematic};

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

/// The parsed rock pool, used by the chunk-based scatter.
pub(crate) fn pool() -> &'static [StructureSchematic] {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rock_assets_parse() {
        assert_eq!(pool().len(), 8, "all 8 rock schems should parse");
        for r in pool() {
            assert!(!r.voxels.is_empty(), "rock parsed to zero voxels");
        }
    }
}
