//! Bundled bushes (10 species × 6 variants), scattered on farmland fields.
//!
//! Each schematic is a small foliage clump around a short bark pole; the pole sits
//! within the leaves' decay distance so `persistent=false` leaves survive in-game.
//! Placement mirrors `rocks`: anchors from the stable cell list, position-hashed
//! rotation and variant pick → identical across tile seams.

use std::sync::OnceLock;

use super::schematic::{load_structure, place_structure, StructureSchematic};
use crate::land_cover::coord_hash;
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

/// Minimum gap between two bushes (they are ~5×5 clumps).
const SPACING: i32 = 5;
/// Hard cap so a huge field never turns into a thicket.
const CAP: u64 = 160;

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

/// Scatter bushes across a farmland field at random rotations.
///
/// `density` is a relative amount (bushes per ~1200 cells); `0` disables the family.
pub fn scatter_bushes(editor: &mut WorldEditor, cells: &[(i32, i32)], density: u8) {
    if density == 0 {
        return;
    }
    let pool = bushes();
    if pool.is_empty() {
        return;
    }
    let n = cells.len();
    if n == 0 {
        return;
    }
    let target = ((n as u64 * density as u64) / 1200).clamp(1, CAP) as usize;
    let mut placed: Vec<(i32, i32)> = Vec::new();
    let max_attempts = target as u32 * 8 + 8;
    let mut t: u32 = 0;
    while placed.len() < target && t < max_attempts {
        let h = coord_hash(t as i32 + 1, (n as i32) ^ 0x00C7_2B19);
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
    fn bush_assets_parse() {
        assert_eq!(bushes().len(), 60, "all 60 bush schems should parse");
        for b in bushes() {
            assert!(!b.voxels.is_empty(), "bush parsed to zero voxels");
        }
    }
}
