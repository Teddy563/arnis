//! Bundled bushes (10 species × 6 variants), scattered on farmland fields.
//!
//! Each schematic is a small foliage clump around a short bark pole; the pole sits
//! within the leaves' decay distance so `persistent=false` leaves survive in-game.
//! Placement mirrors `rocks`: anchors from the stable cell list, position-hashed
//! rotation and variant pick → identical across tile seams.

use std::sync::OnceLock;

use super::schematic::{load_structure, scatter_pool, StructureSchematic};
use crate::world_editor::WorldEditor;

static BUSH_BYTES: [&[u8]; 60] = [
    include_bytes!("../../assets/structures/acaciabush1.schem"),
    include_bytes!("../../assets/structures/acaciabush2.schem"),
    include_bytes!("../../assets/structures/acaciabush3.schem"),
    include_bytes!("../../assets/structures/acaciabush4.schem"),
    include_bytes!("../../assets/structures/acaciabush5.schem"),
    include_bytes!("../../assets/structures/acaciabush6.schem"),
    include_bytes!("../../assets/structures/azaleabush1.schem"),
    include_bytes!("../../assets/structures/azaleabush2.schem"),
    include_bytes!("../../assets/structures/azaleabush3.schem"),
    include_bytes!("../../assets/structures/azaleabush4.schem"),
    include_bytes!("../../assets/structures/azaleabush5.schem"),
    include_bytes!("../../assets/structures/azaleabush6.schem"),
    include_bytes!("../../assets/structures/birchbush1.schem"),
    include_bytes!("../../assets/structures/birchbush2.schem"),
    include_bytes!("../../assets/structures/birchbush3.schem"),
    include_bytes!("../../assets/structures/birchbush4.schem"),
    include_bytes!("../../assets/structures/birchbush5.schem"),
    include_bytes!("../../assets/structures/birchbush6.schem"),
    include_bytes!("../../assets/structures/cherrybush1.schem"),
    include_bytes!("../../assets/structures/cherrybush2.schem"),
    include_bytes!("../../assets/structures/cherrybush3.schem"),
    include_bytes!("../../assets/structures/cherrybush4.schem"),
    include_bytes!("../../assets/structures/cherrybush5.schem"),
    include_bytes!("../../assets/structures/cherrybush6.schem"),
    include_bytes!("../../assets/structures/darkoakbush1.schem"),
    include_bytes!("../../assets/structures/darkoakbush2.schem"),
    include_bytes!("../../assets/structures/darkoakbush3.schem"),
    include_bytes!("../../assets/structures/darkoakbush4.schem"),
    include_bytes!("../../assets/structures/darkoakbush5.schem"),
    include_bytes!("../../assets/structures/darkoakbush6.schem"),
    include_bytes!("../../assets/structures/flowering_azaleabush1.schem"),
    include_bytes!("../../assets/structures/flowering_azaleabush2.schem"),
    include_bytes!("../../assets/structures/flowering_azaleabush3.schem"),
    include_bytes!("../../assets/structures/flowering_azaleabush4.schem"),
    include_bytes!("../../assets/structures/flowering_azaleabush5.schem"),
    include_bytes!("../../assets/structures/flowering_azaleabush6.schem"),
    include_bytes!("../../assets/structures/junglebush1.schem"),
    include_bytes!("../../assets/structures/junglebush2.schem"),
    include_bytes!("../../assets/structures/junglebush3.schem"),
    include_bytes!("../../assets/structures/junglebush4.schem"),
    include_bytes!("../../assets/structures/junglebush5.schem"),
    include_bytes!("../../assets/structures/junglebush6.schem"),
    include_bytes!("../../assets/structures/mangrovebush1.schem"),
    include_bytes!("../../assets/structures/mangrovebush2.schem"),
    include_bytes!("../../assets/structures/mangrovebush3.schem"),
    include_bytes!("../../assets/structures/mangrovebush4.schem"),
    include_bytes!("../../assets/structures/mangrovebush5.schem"),
    include_bytes!("../../assets/structures/mangrovebush6.schem"),
    include_bytes!("../../assets/structures/oakbush1.schem"),
    include_bytes!("../../assets/structures/oakbush2.schem"),
    include_bytes!("../../assets/structures/oakbush3.schem"),
    include_bytes!("../../assets/structures/oakbush4.schem"),
    include_bytes!("../../assets/structures/oakbush5.schem"),
    include_bytes!("../../assets/structures/oakbush6.schem"),
    include_bytes!("../../assets/structures/sprucebush1.schem"),
    include_bytes!("../../assets/structures/sprucebush2.schem"),
    include_bytes!("../../assets/structures/sprucebush3.schem"),
    include_bytes!("../../assets/structures/sprucebush4.schem"),
    include_bytes!("../../assets/structures/sprucebush5.schem"),
    include_bytes!("../../assets/structures/sprucebush6.schem"),
];

/// Hard cap so a huge field never turns into a thicket.
const CAP: usize = 120;

fn bushes() -> &'static [StructureSchematic] {
    static CELL: OnceLock<Vec<StructureSchematic>> = OnceLock::new();
    CELL.get_or_init(|| {
        BUSH_BYTES
            .iter()
            .filter_map(|b| match load_structure(b) {
                Ok(s) => Some(s.base_anchored()),
                Err(e) => {
                    eprintln!("bush schem load failed: {e}");
                    None
                }
            })
            .collect()
    })
}

/// Scatter bushes across a farmland field at random rotations, evenly spread and
/// gently clumped (bushes grow in small groups).
///
/// `density` is bushes per 512×512 region of field area; `0` disables the family.
pub fn scatter_bushes(editor: &mut WorldEditor, cells: &[(i32, i32)], density: u8) {
    scatter_pool(editor, cells, bushes(), density, CAP, true, 0x00C7_2B19);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bush_assets_parse() {
        assert_eq!(bushes().len(), 60, "all 60 bush schems should parse");
        for b in bushes() {
            assert!(!b.voxels.is_empty(), "bush parsed to zero voxels");
        }
    }
}
