//! Work units: a large One World selection cut into region-group pieces.
//!
//! The cut lines sit on a lattice of `512 * n` blocks anchored at block 0 of
//! the world's frame, not at the selection, so every piece owns whole region
//! files (no two pieces write one `r.X.Z.mca`) and two jobs over overlapping
//! selections cut the same lines. Each piece's lat/lon bbox snaps back to
//! exactly its rectangle, so a run given that bbox builds that piece.

use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::projection::{llbbox_for_rect, snap_bbox_to_chunks, WebMercatorProjection};

const REGION_BLOCKS: i32 = 512;

#[derive(Clone, Debug)]
pub struct WorkUnit {
    /// Position in the plan, which is also the build order.
    pub index: u32,
    /// Lattice cell, in groups of `n` regions.
    pub gx: i32,
    pub gz: i32,
    pub rect: XZBBox,
    pub bbox: LLBBox,
    /// Region files the piece touches.
    pub regions: u64,
}

impl WorkUnit {
    pub fn chunks(&self) -> u64 {
        let w = (self.rect.max_x() - self.rect.min_x() + 1) as u64 / 16;
        let h = (self.rect.max_z() - self.rect.min_z() + 1) as u64 / 16;
        w * h
    }

    /// `min_lat,min_lng,max_lat,max_lng` at full f64 precision, so a child
    /// parses back the very bbox that snaps to `rect`.
    pub fn bbox_arg(&self) -> String {
        let (lo, hi) = (self.bbox.min(), self.bbox.max());
        format!("{},{},{},{}", lo.lat(), lo.lng(), hi.lat(), hi.lng())
    }
}

/// Cuts `world` (already snapped to chunks) into pieces of at most `n` x `n`
/// regions, centre first.
pub fn plan_units(
    proj: &WebMercatorProjection,
    world: &XZBBox,
    n: i32,
) -> Result<Vec<WorkUnit>, String> {
    if n < 1 {
        return Err("a work unit is at least one region".to_string());
    }
    let side = REGION_BLOCKS * n;
    let (gx0, gx1) = (
        world.min_x().div_euclid(side),
        world.max_x().div_euclid(side),
    );
    let (gz0, gz1) = (
        world.min_z().div_euclid(side),
        world.max_z().div_euclid(side),
    );
    let mut units = Vec::new();
    for gz in gz0..=gz1 {
        for gx in gx0..=gx1 {
            let rect = XZBBox::rect_from_min_max(
                (gx * side).max(world.min_x()),
                (gz * side).max(world.min_z()),
                (gx * side + side - 1).min(world.max_x()),
                (gz * side + side - 1).min(world.max_z()),
            )?;
            let regions = |a: i32, b: i32| {
                (b.div_euclid(REGION_BLOCKS) - a.div_euclid(REGION_BLOCKS) + 1) as u64
            };
            units.push(WorkUnit {
                index: 0,
                gx,
                gz,
                regions: regions(rect.min_x(), rect.max_x()) * regions(rect.min_z(), rect.max_z()),
                bbox: llbbox_for_rect(proj, &rect)?,
                rect,
            });
        }
    }
    // Centre out, in doubled block units to stay integral; ties go row-major.
    let (cx, cz) = (
        i64::from(world.min_x()) + i64::from(world.max_x()),
        i64::from(world.min_z()) + i64::from(world.max_z()),
    );
    units.sort_by_key(|u| {
        let ux = i64::from(u.rect.min_x()) + i64::from(u.rect.max_x()) - cx;
        let uz = i64::from(u.rect.min_z()) + i64::from(u.rect.max_z()) - cz;
        (ux * ux + uz * uz, u.gz, u.gx)
    });
    for (i, u) in units.iter_mut().enumerate() {
        u.index = i as u32;
    }
    Ok(units)
}

/// `--plan-units N`: prints the plan as one JSON line and touches nothing.
/// A world that does not exist yet is planned in the frame its first run
/// would give it (centred on the request).
pub fn print_plan(
    world_dir: &std::path::Path,
    requested: &LLBBox,
    args: &crate::args::Args,
    n: i32,
) -> Result<(), String> {
    let proj = crate::one_world::frame_for(world_dir, requested, args)?;
    let (world, _) = snap_bbox_to_chunks(&proj, requested)?;
    let mut existing = 0;
    let units: Vec<_> = plan_units(&proj, &world, n)?
        .iter()
        .map(|u| {
            let have = crate::one_world::existing_chunks(world_dir, &u.rect);
            existing += have;
            serde_json::json!({
                "index": u.index,
                "key": format!("{},{}", u.gx, u.gz),
                "rect": [u.rect.min_x(), u.rect.min_z(), u.rect.max_x(), u.rect.max_z()],
                "bbox": u.bbox_arg(),
                "chunks": u.chunks(),
                "regions": u.regions,
                "existing_chunks": have,
            })
        })
        .collect();
    let plan = serde_json::json!({
        "v": 1,
        "type": "plan",
        "unit_regions": n,
        "rect": [world.min_x(), world.min_z(), world.max_x(), world.max_z()],
        "units": units,
        "existing_chunks": existing,
    });
    println!("{plan}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucharest() -> (WebMercatorProjection, XZBBox) {
        let proj = WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        (proj, snap_bbox_to_chunks(&proj, &req).unwrap().0)
    }

    fn r(b: &XZBBox) -> [i32; 4] {
        [b.min_x(), b.min_z(), b.max_x(), b.max_z()]
    }

    #[test]
    fn plan_units_cover_without_overlap() {
        let (proj, world) = bucharest();
        for n in [1, 2, 4] {
            let units = plan_units(&proj, &world, n).unwrap();
            assert!(units.len() > 1, "n={n}");
            let area: u64 = units.iter().map(WorkUnit::chunks).sum();
            let (w, h) = (
                world.max_x() - world.min_x() + 1,
                world.max_z() - world.min_z() + 1,
            );
            assert_eq!(
                area,
                (w as u64 / 16) * (h as u64 / 16),
                "n={n}: gap or overlap"
            );
            for (i, a) in units.iter().enumerate() {
                let [x0, z0, x1, z1] = r(&a.rect);
                assert!(x0 >= world.min_x() && x1 <= world.max_x());
                assert!(z0 >= world.min_z() && z1 <= world.max_z());
                for b in &units[i + 1..] {
                    let [bx0, bz0, bx1, bz1] = r(&b.rect);
                    let apart = x1 < bx0 || bx1 < x0 || z1 < bz0 || bz1 < z0;
                    assert!(apart, "n={n}: pieces {} and {} overlap", a.index, b.index);
                }
                // Whole region files: no two pieces share an r.X.Z.mca.
                assert_eq!(x0.div_euclid(512 * n), x1.div_euclid(512 * n));
                assert_eq!(z0.div_euclid(512 * n), z1.div_euclid(512 * n));
            }
        }
    }

    #[test]
    fn plan_units_is_deterministic() {
        let (proj, world) = bucharest();
        let key = |u: &[WorkUnit]| u.iter().map(|u| (u.index, r(&u.rect))).collect::<Vec<_>>();
        let a = plan_units(&proj, &world, 1).unwrap();
        assert_eq!(key(&a), key(&plan_units(&proj, &world, 1).unwrap()));
        assert!(a.iter().enumerate().all(|(i, u)| u.index as usize == i));
    }

    #[test]
    fn a_piece_bbox_snaps_back_to_its_rect() {
        let (proj, world) = bucharest();
        for u in plan_units(&proj, &world, 1).unwrap() {
            let parsed = LLBBox::from_str(&u.bbox_arg()).unwrap();
            assert_eq!(parsed, u.bbox);
            assert_eq!(
                r(&snap_bbox_to_chunks(&proj, &parsed).unwrap().0),
                r(&u.rect)
            );
        }
    }

    /// The overlay draws `bounds`, so it must sit where `rect` does: pieces meet on region
    /// boundaries, tile the job, and `bounds` projects back to `rect` within a block.
    #[test]
    fn the_piece_grid_is_what_the_overlay_draws() {
        let req = LLBBox::new(44.40, 26.00, 44.52, 26.22).unwrap();
        for scale in [1.0, 0.5, 0.25, 0.05] {
            let proj = WebMercatorProjection::new(44.46, 26.11, scale);
            let world = snap_bbox_to_chunks(&proj, &req).unwrap().0;
            for n in [1, 2] {
                let side = 512 * n;
                let units = plan_units(&proj, &world, n).unwrap();
                let mut area = 0u64;
                for (i, a) in units.iter().enumerate() {
                    let [x0, z0, x1, z1] = r(&a.rect);
                    // Interior edges are region boundaries; the job's own edges are chunks.
                    assert!(x0 == world.min_x() || x0 % side == 0, "s={scale} n={n}");
                    assert!(z0 == world.min_z() || z0 % side == 0, "s={scale} n={n}");
                    assert!(
                        x1 == world.max_x() || (x1 + 1) % side == 0,
                        "s={scale} n={n}"
                    );
                    assert!(
                        z1 == world.max_z() || (z1 + 1) % side == 0,
                        "s={scale} n={n}"
                    );
                    area += a.chunks();
                    for b in &units[i + 1..] {
                        let [bx0, bz0, bx1, bz1] = r(&b.rect);
                        assert!(
                            x1 < bx0 || bx1 < x0 || z1 < bz0 || bz1 < z0,
                            "s={scale} n={n}"
                        );
                    }
                    let (lo, hi) = (a.bbox.min(), a.bbox.max());
                    let back = [
                        proj.x_for_lon(lo.lng()),
                        proj.z_for_lat(hi.lat()),
                        proj.x_for_lon(hi.lng()) - 1.0,
                        proj.z_for_lat(lo.lat()) - 1.0,
                    ];
                    for (b, want) in back.iter().zip([x0, z0, x1, z1]) {
                        assert!(
                            (b - f64::from(want)).abs() <= 1.0,
                            "s={scale} n={n}: {b} vs {want}"
                        );
                    }
                }
                let (w, h) = (
                    world.max_x() - world.min_x() + 1,
                    world.max_z() - world.min_z() + 1,
                );
                assert_eq!(
                    area,
                    (w as u64 / 16) * (h as u64 / 16),
                    "s={scale} n={n}: gap"
                );
            }
        }
    }
}
