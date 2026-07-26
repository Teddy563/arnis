//! Coarse lowland grid: which parts of the map sit low and flat.
//!
//! Sunflower fields cluster in low, open plains in real agriculture. This samples the
//! terrain height on a coarse lattice, marks cells below the ~35th percentile as
//! lowland, and lets the farm-crop picker boost sunflower plots there.

use std::sync::RwLock;

use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::ground::Ground;

const CELL: i32 = 128;

struct LowlandGrid {
    min_x: i32,
    min_z: i32,
    w: i32,
    h: i32,
    low: Vec<bool>,
}

static GRID: RwLock<Option<LowlandGrid>> = RwLock::new(None);

/// Build the lowland grid from the run's terrain. Call once per generation.
pub fn set_from_ground(ground: &Ground, xzbbox: &XZBBox) {
    let min_x = xzbbox.min_x();
    let min_z = xzbbox.min_z();
    let w = ((xzbbox.max_x() - min_x) / CELL + 2).max(1);
    let h = ((xzbbox.max_z() - min_z) / CELL + 2).max(1);
    let mut heights = Vec::with_capacity((w * h) as usize);
    for gz in 0..h {
        for gx in 0..w {
            let x = (gx * CELL + CELL / 2).min(xzbbox.max_x() - min_x);
            let z = (gz * CELL + CELL / 2).min(xzbbox.max_z() - min_z);
            heights.push(ground.level(XZPoint::new(x, z)));
        }
    }
    let mut sorted = heights.clone();
    sorted.sort_unstable();
    let thr = sorted[(sorted.len() as f64 * 0.35) as usize % sorted.len()];
    let low = heights.iter().map(|&y| y <= thr).collect();
    *GRID.write().unwrap() = Some(LowlandGrid { min_x, min_z, w, h, low });
}

/// True when world point (x, z) sits in the low ~third of the terrain.
pub fn is_lowland(x: i32, z: i32) -> bool {
    let guard = GRID.read().unwrap();
    let Some(g) = guard.as_ref() else {
        return false;
    };
    let gx = ((x - g.min_x) / CELL).clamp(0, g.w - 1);
    let gz = ((z - g.min_z) / CELL).clamp(0, g.h - 1);
    g.low[(gz * g.w + gx) as usize]
}
