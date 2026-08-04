//! Coarse lowland grid: which parts of the map sit low and flat.
//!
//! Sunflower fields cluster in low, open plains in real agriculture. This samples the
//! terrain height on a coarse lattice and marks the low band, letting the farm-crop
//! picker boost sunflower plots there.
//!
//! SEAM-CRITICAL, two ways:
//!   * the lattice is anchored to the WORLD (`div_euclid(CELL)` on absolute coords),
//!     never to the tile's bbox, so a point lands in the same lattice cell in every
//!     tile;
//!   * the height threshold comes from the GLOBAL elevation affine (Meld's
//!     `--elevation-min/--elevation-max` lock), exactly like the snowline in
//!     `ground::compute_snow_threshold`. The previous per-tile 35th-percentile made a
//!     mountain tile and a plains tile disagree about the same field, so crops flipped
//!     along the seam. Without elevation data (flat worlds / no lock) it falls back to
//!     the per-tile percentile, which is what a single-tile run always saw.

use std::collections::HashMap;
use std::sync::RwLock;

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;

const CELL: i32 = 128;
/// Lowland = the bottom this-much of the world's terrain span. Applied to the global
/// span, so it names the same real elevation band in every tile.
const LOW_FRACTION: f64 = 0.18;

struct LowlandGrid {
    /// World lattice cell -> low?
    low: HashMap<(i32, i32), bool>,
}

static GRID: RwLock<Option<LowlandGrid>> = RwLock::new(None);

/// Build the lowland grid from the run's terrain. Call once per generation.
pub fn set_from_ground(ground: &Ground, xzbbox: &XZBBox) {
    let (min_x, min_z) = (xzbbox.min_x(), xzbbox.min_z());
    let (max_x, max_z) = (xzbbox.max_x(), xzbbox.max_z());
    // Every world lattice cell that overlaps this tile, sampled at its true lattice
    // centre (clamped into the tile only as a last resort near the data edge, where
    // the elevation grid the tile fetched still covers the seam halo).
    let (gx0, gx1) = (min_x.div_euclid(CELL), max_x.div_euclid(CELL));
    let (gz0, gz1) = (min_z.div_euclid(CELL), max_z.div_euclid(CELL));
    let mut samples: Vec<((i32, i32), i32)> = Vec::new();
    for gz in gz0..=gz1 {
        for gx in gx0..=gx1 {
            let cx = (gx * CELL + CELL / 2).clamp(min_x, max_x);
            let cz = (gz * CELL + CELL / 2).clamp(min_z, max_z);
            samples.push(((gx, gz), ground.level(XZPoint::new(cx, cz))));
        }
    }
    if samples.is_empty() {
        *GRID.write().unwrap() = None;
        return;
    }
    // Global band when the elevation affine is available (Meld always locks it), else
    // this tile's own 35th percentile — the original single-tile behaviour.
    let threshold = match ground.low_band_threshold_y(LOW_FRACTION) {
        Some(y) => y,
        None => {
            let mut sorted: Vec<i32> = samples.iter().map(|&(_, y)| y).collect();
            sorted.sort_unstable();
            sorted[(sorted.len() as f64 * 0.35) as usize % sorted.len()]
        }
    };
    let low = samples
        .into_iter()
        .map(|(key, y)| (key, y <= threshold))
        .collect();
    *GRID.write().unwrap() = Some(LowlandGrid { low });
}

/// True when world point (x, z) sits in the low band of the terrain. Unknown lattice
/// cells (outside the tile that built the grid) read as not-low.
pub fn is_lowland(x: i32, z: i32) -> bool {
    let guard = GRID.read().unwrap();
    let Some(g) = guard.as_ref() else {
        return false;
    };
    g.low
        .get(&(x.div_euclid(CELL), z.div_euclid(CELL)))
        .copied()
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lookups must map a world point to its world lattice cell, independent of which
    /// tile built the grid.
    #[test]
    fn lookup_is_world_anchored() {
        let mut low = HashMap::new();
        low.insert((3, -2), true);
        *GRID.write().unwrap() = Some(LowlandGrid { low });
        // 3*128 .. 3*128+127 all resolve to lattice cell 3; -2 covers -256..-129.
        assert!(is_lowland(3 * CELL, -2 * CELL));
        assert!(is_lowland(3 * CELL + 127, -CELL - 1));
        assert!(!is_lowland(4 * CELL, -2 * CELL));
        *GRID.write().unwrap() = None;
    }
}
