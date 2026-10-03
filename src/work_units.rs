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
    pub index: usize,
    /// Lattice cell, in groups of `n` regions.
    pub gx: i32,
    pub gz: i32,
    pub rect: XZBBox,
    pub llbbox: LLBBox,
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
        let (lo, hi) = (self.llbbox.min(), self.llbbox.max());
        format!("{},{},{},{}", lo.lat(), lo.lng(), hi.lat(), hi.lng())
    }
}

/// Snaps `requested` to chunks once and cuts it into pieces of at most
/// `n` x `n` regions, centre first. Returns the snapped selection too.
pub fn plan_units(
    proj: &WebMercatorProjection,
    requested: &LLBBox,
    n: i32,
) -> Result<(XZBBox, Vec<WorkUnit>), String> {
    if n < 1 {
        return Err("a work unit is at least one region".to_string());
    }
    let (sel, _) = snap_bbox_to_chunks(proj, requested)?;
    let side = REGION_BLOCKS * n;
    let (gx0, gx1) = (sel.min_x().div_euclid(side), sel.max_x().div_euclid(side));
    let (gz0, gz1) = (sel.min_z().div_euclid(side), sel.max_z().div_euclid(side));
    let mut units = Vec::new();
    for gz in gz0..=gz1 {
        for gx in gx0..=gx1 {
            let rect = XZBBox::rect_from_min_max(
                (gx * side).max(sel.min_x()),
                (gz * side).max(sel.min_z()),
                (gx * side + side - 1).min(sel.max_x()),
                (gz * side + side - 1).min(sel.max_z()),
            )?;
            let llbbox = llbbox_for_rect(proj, &rect)?;
            units.push(WorkUnit {
                index: 0,
                gx,
                gz,
                rect,
                llbbox,
            });
        }
    }
    // Centre out, in doubled block units to stay integral; ties go row-major.
    let (cx, cz) = (
        i64::from(sel.min_x()) + i64::from(sel.max_x()),
        i64::from(sel.min_z()) + i64::from(sel.max_z()),
    );
    units.sort_by_key(|u| {
        let ux = i64::from(u.rect.min_x()) + i64::from(u.rect.max_x()) - cx;
        let uz = i64::from(u.rect.min_z()) + i64::from(u.rect.max_z()) - cz;
        (ux * ux + uz * uz, u.gz, u.gx)
    });
    for (i, u) in units.iter_mut().enumerate() {
        u.index = i;
    }
    Ok((sel, units))
}

/// `--plan-units N`: prints the plan as one JSON line and touches nothing.
/// A world that does not exist yet is planned in the frame its first run
/// would give it.
pub fn print_plan(
    world_dir: &std::path::Path,
    requested: &LLBBox,
    args: &crate::args::Args,
    n: i32,
) -> Result<(), String> {
    let proj = crate::one_world::frame_for(world_dir, requested, args)?;
    let (sel, units) = plan_units(&proj, requested, n)?;
    let rect = |r: &XZBBox| [r.min_x(), r.min_z(), r.max_x(), r.max_z()];
    let units: Vec<_> = units
        .iter()
        .map(|u| {
            serde_json::json!({
                "index": u.index,
                "key": format!("{},{}", u.gx, u.gz),
                "rect": rect(&u.rect),
                "bbox": u.bbox_arg(),
                "chunks": u.chunks(),
                "existing_chunks": crate::one_world::existing_chunks(world_dir, &u.rect),
            })
        })
        .collect();
    let plan = serde_json::json!({
        "v": 1,
        "type": "plan",
        "unit_regions": n,
        "rect": rect(&sel),
        "units": units,
    });
    println!("{plan}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(b: &XZBBox) -> (i32, i32, i32, i32) {
        (b.min_x(), b.min_z(), b.max_x(), b.max_z())
    }

    fn bucharest() -> (WebMercatorProjection, LLBBox) {
        let proj = WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        (proj, req)
    }

    #[test]
    fn units_tile_the_selection_on_the_region_lattice() {
        let (proj, req) = bucharest();
        for n in [1, 2, 4] {
            let (sel, units) = plan_units(&proj, &req, n).unwrap();
            let cells: u64 = units.iter().map(|u| u.chunks()).sum();
            let (w, h) = (
                (sel.max_x() - sel.min_x() + 1) / 16,
                (sel.max_z() - sel.min_z() + 1) / 16,
            );
            assert_eq!(cells, (w * h) as u64, "n={n}: gap or overlap");
            for u in &units {
                // Interior edges on multiples of 512n, also left of the origin.
                let (x0, z0, x1, z1) = r(&u.rect);
                assert!(x0 == sel.min_x() || x0.rem_euclid(512 * n) == 0);
                assert!(z0 == sel.min_z() || z0.rem_euclid(512 * n) == 0);
                assert!(x1 == sel.max_x() || (x1 + 1).rem_euclid(512 * n) == 0);
                assert!(z1 == sel.max_z() || (z1 + 1).rem_euclid(512 * n) == 0);
                assert_eq!(x0.div_euclid(512 * n), x1.div_euclid(512 * n));
            }
        }
        // The selection straddles block 0 in both axes, so negatives are covered.
        let (sel, _) = plan_units(&proj, &req, 1).unwrap();
        assert!(sel.min_x() < 0 && sel.max_x() > 0 && sel.min_z() < 0 && sel.max_z() > 0);
    }

    #[test]
    fn each_piece_bbox_snaps_back_to_its_rect_and_neighbours_touch() {
        let (proj, req) = bucharest();
        let (_, units) = plan_units(&proj, &req, 1).unwrap();
        for u in &units {
            let parsed = LLBBox::from_str(&u.bbox_arg()).unwrap();
            assert_eq!(parsed, u.llbbox, "bbox_arg must round-trip exactly");
            let (again, _) = snap_bbox_to_chunks(&proj, &parsed).unwrap();
            assert_eq!(r(&again), r(&u.rect));
        }
        // Pair invariant: the east neighbour of a piece is found by its lattice
        // cell, and must start one block past it on the same rows.
        let mut pairs = 0;
        for a in &units {
            if let Some(b) = units.iter().find(|b| b.gx == a.gx + 1 && b.gz == a.gz) {
                assert_eq!(b.rect.min_x(), a.rect.max_x() + 1);
                assert_eq!(
                    (b.rect.min_z(), b.rect.max_z()),
                    (a.rect.min_z(), a.rect.max_z())
                );
                pairs += 1;
            }
        }
        assert!(pairs > 0, "no adjacent pair was checked");
    }

    #[test]
    fn a_tiny_request_is_one_unit_of_one_chunk() {
        let proj = WebMercatorProjection::new(10.0, 10.0, 1.0);
        let req = LLBBox::new(10.0, 10.0, 10.000001, 10.000001).unwrap();
        let (_, units) = plan_units(&proj, &req, 4).unwrap();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].chunks(), 1);
    }

    #[test]
    fn far_from_the_origin_pieces_still_align() {
        let proj = WebMercatorProjection::new(0.0, 0.0, 1.0);
        let req = LLBBox::new(10.0, 20.0, 10.02, 20.03).unwrap();
        let (_, units) = plan_units(&proj, &req, 2).unwrap();
        assert!(units.len() > 1);
        for u in &units {
            assert!(u.rect.min_x() > 1_000_000);
            assert_eq!(
                u.rect.min_x().div_euclid(1024),
                u.rect.max_x().div_euclid(1024)
            );
        }
    }

    #[test]
    fn the_centre_piece_comes_first_and_the_order_is_stable() {
        let (proj, req) = bucharest();
        let (sel, units) = plan_units(&proj, &req, 1).unwrap();
        // An inner piece first, a corner piece last.
        let (f, l) = (r(&units[0].rect), r(&units[units.len() - 1].rect));
        assert!(f.0 > sel.min_x() && f.1 > sel.min_z() && f.2 < sel.max_x() && f.3 < sel.max_z());
        assert!(
            (l.0 == sel.min_x() || l.2 == sel.max_x())
                && (l.1 == sel.min_z() || l.3 == sel.max_z())
        );
        let (_, again) = plan_units(&proj, &req, 1).unwrap();
        let keys = |u: &[WorkUnit]| u.iter().map(|u| (u.gx, u.gz)).collect::<Vec<_>>();
        assert_eq!(keys(&units), keys(&again));
        assert!(units.iter().enumerate().all(|(i, u)| u.index == i));
    }
}
