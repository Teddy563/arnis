//! Coarse road-bearing grid: which way do the roads run, per area of the map.
//!
//! Field parcels align to their access roads in real agricultural land. This module
//! aggregates the direction of OSM highway segments into a coarse grid (doubled-angle
//! sums, so a road and its reverse count as the same bearing); the field-texture
//! orientation domains then take their rotation from the dominant nearby road instead
//! of a hash. Falls back to the hashed angle where there are no roads.

use std::sync::RwLock;

use crate::coordinate_system::cartesian::XZBBox;
use crate::osm_parser::ProcessedElement;

/// Grid cell edge (blocks).
const CELL: i32 = 128;

struct BearingGrid {
    min_x: i32,
    min_z: i32,
    w: i32,
    h: i32,
    // Doubled-angle direction sums per cell (bearing-fold: theta == theta+180).
    sx: Vec<f64>,
    sz: Vec<f64>,
}

static GRID: RwLock<Option<BearingGrid>> = RwLock::new(None);

/// Build the bearing grid from this run's OSM elements. Call once per generation,
/// before element/ground processing.
pub fn set_from_elements(elements: &[ProcessedElement], xzbbox: &XZBBox) {
    let min_x = xzbbox.min_x();
    let min_z = xzbbox.min_z();
    let w = ((xzbbox.max_x() - min_x) / CELL + 2).max(1);
    let h = ((xzbbox.max_z() - min_z) / CELL + 2).max(1);
    let mut grid = BearingGrid {
        min_x,
        min_z,
        w,
        h,
        sx: vec![0.0; (w as usize) * (h as usize)],
        sz: vec![0.0; (w as usize) * (h as usize)],
    };
    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        let Some(hw) = way.tags.get("highway") else {
            continue;
        };
        // Skip point-like/footway noise; keep everything that reads as a road/lane.
        if matches!(
            hw.as_str(),
            "street_lamp" | "crossing" | "bus_stop" | "steps"
        ) {
            continue;
        }
        for pair in way.nodes.windows(2) {
            let (x1, z1) = (pair[0].x, pair[0].z);
            let (x2, z2) = (pair[1].x, pair[1].z);
            let dx = (x2 - x1) as f64;
            let dz = (z2 - z1) as f64;
            let len = (dx * dx + dz * dz).sqrt();
            if len < 1.0 {
                continue;
            }
            let theta = dz.atan2(dx);
            let (c2, s2) = ((2.0 * theta).cos() * len, (2.0 * theta).sin() * len);
            // Stamp the segment's bearing into every grid cell it passes through.
            let steps = (len / CELL as f64).ceil() as i32 + 1;
            for i in 0..=steps {
                let t = i as f64 / steps as f64;
                let px = x1 as f64 + dx * t;
                let pz = z1 as f64 + dz * t;
                let gx = ((px as i32 - min_x) / CELL).clamp(0, w - 1);
                let gz = ((pz as i32 - min_z) / CELL).clamp(0, h - 1);
                let idx = (gz * w + gx) as usize;
                grid.sx[idx] += c2;
                grid.sz[idx] += s2;
            }
        }
    }
    *GRID.write().unwrap() = Some(grid);
}

/// Dominant road bearing near world point (x, z), as (sin, cos) of the grid angle —
/// ready to rotate parcel coordinates. None when no road is near enough to matter.
pub fn bearing_at(x: i32, z: i32) -> Option<(f64, f64)> {
    let guard = GRID.read().unwrap();
    let grid = guard.as_ref()?;
    let gx = (x - grid.min_x) / CELL;
    let gz = (z - grid.min_z) / CELL;
    // 3x3 neighbourhood sum so a road one cell over still orients its surroundings.
    let (mut sx, mut sz) = (0.0f64, 0.0f64);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let cx = gx + dx;
            let cz = gz + dz;
            if cx < 0 || cz < 0 || cx >= grid.w || cz >= grid.h {
                continue;
            }
            let idx = (cz * grid.w + cx) as usize;
            sx += grid.sx[idx];
            sz += grid.sz[idx];
        }
    }
    // Need a meaningful amount of road (in blocks of accumulated length).
    if (sx * sx + sz * sz).sqrt() < 40.0 {
        return None;
    }
    let theta = 0.5 * sz.atan2(sx);
    Some((theta.sin(), theta.cos()))
}
