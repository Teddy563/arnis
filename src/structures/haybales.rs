//! Procedural hay-bale bundles on harvested farm plots.
//!
//! Small clusters of 1-3 hay bales (single / pair / stacked pair / L of three),
//! budgeted per 512×512 region like the rock/bush scatter, placed only on wheat or
//! fallow plots and never on tracks. Purely position-hashed → tile-seam safe.

use crate::block_definitions::HAY_BALE;
use crate::element_processing::field_texture::{FarmCrop, FieldCategory, FieldProfile};
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;
use std::collections::HashSet;

/// Bundles per 512×512 region of field area.
const PER_REGION: f64 = 2.0;
/// Hard cap per field element.
const CAP: usize = 80;

/// Scatter hay-bale bundles across a farmland element's cells.
pub fn scatter_haybales(editor: &mut WorldEditor, cells: &[(i32, i32)], profile: &FieldProfile) {
    if cells.is_empty() || !profile.is_active() {
        return;
    }
    let n = cells.len();
    let target = ((n as f64 / 262_144.0) * PER_REGION).round().clamp(0.0, CAP as f64) as usize;
    if target == 0 {
        return;
    }
    let set: HashSet<(i32, i32)> = cells.iter().copied().collect();
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &(x, z) in cells {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_z = min_z.min(z);
        max_z = max_z.max(z);
    }
    let area = (max_x - min_x + 1) as i64 * (max_z - min_z + 1) as i64;
    let spacing = ((area as f64 / target as f64).sqrt() as i32).max(4);
    // Pure position test (no editor borrow); water is checked at placement time.
    let bale_ok = |x: i32, z: i32| -> bool {
        if !set.contains(&(x, z)) {
            return false;
        }
        let c = profile.cell_at(x, z);
        c.cat == FieldCategory::Farm
            && !c.is_track
            && matches!(c.crop, Some(FarmCrop::Wheat) | Some(FarmCrop::Fallow))
    };
    let mut placed = 0usize;
    let mut gz = min_z;
    while gz <= max_z && placed < target {
        let mut gx = min_x;
        while gx <= max_x && placed < target {
            let h = coord_hash(gx ^ 0x0000_4A7B, gz.wrapping_mul(41) ^ 0x0000_4A7B);
            let cx = gx + (h % spacing as u64) as i32;
            let cz = gz + ((h >> 20) % spacing as u64) as i32;
            if bale_ok(cx, cz) && !editor.is_lc_water(cx, cz) {
                // Arrangement by hash: single / pair / stacked pair / L of three.
                let arr = (h >> 5) & 3;
                editor.set_block(HAY_BALE, cx, 1, cz, None, None);
                match arr {
                    1 => {
                        if bale_ok(cx + 1, cz) {
                            editor.set_block(HAY_BALE, cx + 1, 1, cz, None, None);
                        }
                    }
                    2 => {
                        if bale_ok(cx + 1, cz) {
                            editor.set_block(HAY_BALE, cx + 1, 1, cz, None, None);
                        }
                        editor.set_block(HAY_BALE, cx, 2, cz, None, None);
                    }
                    3 => {
                        if bale_ok(cx + 1, cz) {
                            editor.set_block(HAY_BALE, cx + 1, 1, cz, None, None);
                        }
                        if bale_ok(cx, cz + 1) {
                            editor.set_block(HAY_BALE, cx, 1, cz + 1, None, None);
                        }
                    }
                    _ => {}
                }
                placed += 1;
            }
            gx += spacing;
        }
        gz += spacing;
    }
}
