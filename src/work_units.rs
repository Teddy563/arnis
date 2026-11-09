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
    /// Lattice cell, in groups of `size` regions.
    pub gx: i32,
    pub gz: i32,
    /// Side of the lattice cell, in regions.
    pub size: i32,
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

    /// `rx,rz,s`: the piece's north-west region and its lattice size, which no other piece
    /// of any plan on this world shares unless it covers the same regions.
    pub fn region_key(&self) -> String {
        format!(
            "{},{},{}",
            self.rect.min_x().div_euclid(REGION_BLOCKS),
            self.rect.min_z().div_euclid(REGION_BLOCKS),
            self.size
        )
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
#[cfg(test)]
pub fn plan_units(
    proj: &WebMercatorProjection,
    world: &XZBBox,
    n: i32,
) -> Result<Vec<WorkUnit>, String> {
    plan_cells(proj, world, n, false)
}

/// Edge strips thinner than this join their neighbour under `--snap-regions merge`.
const SLIVER_BLOCKS: i32 = REGION_BLOCKS / 4;

/// One axis of the lattice over `lo..=hi`: `(cell, first, last)` per cell, clipped.
fn segments(lo: i32, hi: i32, side: i32, merge: bool) -> Vec<(i32, i32, i32)> {
    let mut v: Vec<_> = (lo.div_euclid(side)..=hi.div_euclid(side))
        .map(|g| (g, (g * side).max(lo), (g * side + side - 1).min(hi)))
        .collect();
    let thin = |s: &(i32, i32, i32)| s.2 - s.1 + 1 < SLIVER_BLOCKS;
    if merge && v.len() > 1 && thin(&v[0]) {
        let first = v.remove(0);
        v[0].1 = first.1;
    }
    if merge && v.len() > 1 && thin(&v[v.len() - 1]) {
        let last = v.pop().unwrap_or_default();
        let k = v.len() - 1;
        v[k].2 = last.2;
    }
    v
}

fn plan_cells(
    proj: &WebMercatorProjection,
    world: &XZBBox,
    n: i32,
    merge: bool,
) -> Result<Vec<WorkUnit>, String> {
    if n < 1 {
        return Err("a work unit is at least one region".to_string());
    }
    let side = REGION_BLOCKS * n;
    let mut units = Vec::new();
    for (gz, z0, z1) in segments(world.min_z(), world.max_z(), side, merge) {
        for &(gx, x0, x1) in &segments(world.min_x(), world.max_x(), side, merge) {
            let rect = XZBBox::rect_from_min_max(x0, z0, x1, z1)?;
            let regions = |a: i32, b: i32| {
                (b.div_euclid(REGION_BLOCKS) - a.div_euclid(REGION_BLOCKS) + 1) as u64
            };
            units.push(WorkUnit {
                index: 0,
                gx,
                gz,
                size: n,
                regions: regions(rect.min_x(), rect.max_x()) * regions(rect.min_z(), rect.max_z()),
                bbox: llbbox_for_rect(proj, &rect)?,
                rect,
            });
        }
    }
    order_centre_out(world, &mut units);
    Ok(units)
}

/// Numbers `units` from the centre of `world` out, in doubled block units to stay integral;
/// ties go row-major.
fn order_centre_out(world: &XZBBox, units: &mut [WorkUnit]) {
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
}

/// A job's pieces and the block rectangle they tile, from the selection and the job flags.
pub struct Plan {
    /// The selection snapped to chunks, as a run without the job flags would build it.
    pub requested: XZBBox,
    /// What the job builds: `requested` after `--snap-regions` and `--margin-regions`.
    pub rect: XZBBox,
    pub units: Vec<WorkUnit>,
    /// The job's folder name: one plan, one folder.
    pub name: String,
}

/// The plan a job runs and `--plan-units` prints: both call this, so they agree.
pub fn plan(
    proj: &WebMercatorProjection,
    selection: &LLBBox,
    units: &crate::args::UnitArgs,
    n: i32,
) -> Result<Plan, String> {
    use crate::args::SnapRegions;
    let (requested, _) = snap_bbox_to_chunks(proj, selection)?;
    let side = REGION_BLOCKS * n;
    let axis = |lo: i32, hi: i32| match units.snap_regions {
        Some(SnapRegions::Cover) => (
            lo.div_euclid(side) * side,
            (hi.div_euclid(side) + 1) * side - 1,
        ),
        Some(SnapRegions::Fit) => {
            let (a, b) = ((lo + side - 1).div_euclid(side), (hi + 1).div_euclid(side));
            // Not one whole piece inside: the one under the centre.
            let (a, b) = if b > a {
                (a, b)
            } else {
                let c = (lo + hi).div_euclid(2).div_euclid(side);
                (c, c + 1)
            };
            (a * side, b * side - 1)
        }
        _ => (lo, hi),
    };
    let m = units.margin_regions * REGION_BLOCKS;
    let (x0, x1) = axis(requested.min_x(), requested.max_x());
    let (z0, z1) = axis(requested.min_z(), requested.max_z());
    let rect = XZBBox::rect_from_min_max(x0 - m, z0 - m, x1 + m, z1 + m)?;
    let merge = units.snap_regions == Some(SnapRegions::Merge);
    let cut = plan_cells(proj, &rect, n, merge)?;
    let mut name = format!(
        "{}_{}_{}_{}_n{n}",
        rect.min_x(),
        rect.min_z(),
        rect.max_x(),
        rect.max_z()
    );
    // Merged edges cut other pieces than a plain job over the same rectangle.
    if merge {
        name.push_str("_merge");
    }
    Ok(Plan {
        requested,
        rect,
        units: cut,
        name,
    })
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
    let Plan {
        requested: asked,
        rect: world,
        units,
        ..
    } = plan(&proj, requested, &args.units, n)?;
    let mut existing = 0;
    let units: Vec<_> = units
        .iter()
        .map(|u| {
            let have = crate::one_world::existing_chunks(world_dir, &u.rect);
            existing += have;
            serde_json::json!({
                "index": u.index,
                "key": format!("{},{}", u.gx, u.gz),
                "size": u.size,
                "region_key": u.region_key(),
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
        "requested": [asked.min_x(), asked.min_z(), asked.max_x(), asked.max_z()],
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

    /// Doc 18's example: a new world centred on the request, 2x2-region pieces.
    fn doc18() -> (WebMercatorProjection, LLBBox) {
        let req = LLBBox::new(44.425, 26.08, 44.445, 26.13).unwrap();
        (WebMercatorProjection::new(44.435, 26.105, 1.0), req)
    }

    fn sizes(units: &[WorkUnit]) -> Vec<(i32, i32)> {
        let mut v: Vec<_> = units
            .iter()
            .map(|u| {
                let [x0, z0, x1, z1] = r(&u.rect);
                (x1 - x0 + 1, z1 - z0 + 1)
            })
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn without_the_job_flags_the_plan_is_todays() {
        let (proj, req) = doc18();
        let p = plan(&proj, &req, &Default::default(), 2).unwrap();
        let world = snap_bbox_to_chunks(&proj, &req).unwrap().0;
        assert_eq!(r(&p.rect), r(&world));
        assert_eq!(r(&p.rect), [-2000, -1120, 1999, 1119]);
        let today = plan_units(&proj, &world, 2).unwrap();
        let key = |u: &[WorkUnit]| u.iter().map(|u| (u.index, r(&u.rect))).collect::<Vec<_>>();
        assert_eq!(key(&p.units), key(&today));
        assert_eq!(p.name, "-2000_-1120_1999_1119_n2");
        // The 16 pieces doc 18 found, slivers and all.
        let mut want = [(1024, 1024); 4].to_vec();
        want.extend([(976, 1024); 4]);
        want.extend([(1024, 96); 4]);
        want.extend([(976, 96); 4]);
        want.sort_unstable();
        assert_eq!(sizes(&p.units), want);
    }

    #[test]
    fn snapped_jobs_are_whole_pieces() {
        use crate::args::{SnapRegions, UnitArgs};
        let (proj, req) = doc18();
        let with = |snap, margin| UnitArgs {
            snap_regions: Some(snap),
            margin_regions: margin,
            ..Default::default()
        };
        // Cover: every piece is exactly n x n regions.
        for (n, z, pieces) in [(1, 1536, 48), (2, 2048, 16), (4, 2048, 4)] {
            let p = plan(&proj, &req, &with(SnapRegions::Cover, 0), n).unwrap();
            let side = 512 * n;
            assert!(sizes(&p.units).iter().all(|&s| s == (side, side)), "n={n}");
            assert_eq!(r(&p.rect), [-2048, -z, 2047, z - 1], "n={n}");
            assert_eq!(p.units.len(), pieces, "n={n}");
        }
        // Fit: whole pieces inside the selection.
        let p = plan(&proj, &req, &with(SnapRegions::Fit, 0), 2).unwrap();
        assert_eq!(r(&p.rect), [-1024, -1024, 1023, 1023]);
        assert_eq!(sizes(&p.units), [(1024, 1024); 4]);
        // Nothing whole fits: the piece under the centre.
        let p = plan(&proj, &req, &with(SnapRegions::Fit, 0), 4).unwrap();
        assert_eq!(r(&p.rect), [-2048, -2048, -1, -1]);
        // A margin of one region on every side.
        let p = plan(&proj, &req, &with(SnapRegions::Cover, 1), 2).unwrap();
        assert_eq!(r(&p.rect), [-2560, -2560, 2559, 2559]);
        assert_eq!(p.name, "-2560_-2560_2559_2559_n2");
    }

    #[test]
    fn merged_slivers_leave_no_thin_edge() {
        use crate::args::{SnapRegions, UnitArgs};
        let (proj, req) = doc18();
        let units = UnitArgs {
            snap_regions: Some(SnapRegions::Merge),
            ..Default::default()
        };
        let p = plan(&proj, &req, &units, 2).unwrap();
        assert_eq!(r(&p.rect), [-2000, -1120, 1999, 1119]);
        assert!(p.name.ends_with("_merge"));
        let mut want = [(1024, 1120); 4].to_vec();
        want.extend([(976, 1120); 4]);
        want.sort_unstable();
        assert_eq!(sizes(&p.units), want);
        // Still a tiling, and no region file shared.
        let area: u64 = p.units.iter().map(WorkUnit::chunks).sum();
        assert_eq!(area, 250 * 140);
        for (i, a) in p.units.iter().enumerate() {
            for b in &p.units[i + 1..] {
                let ([ax0, az0, ax1, az1], [bx0, bz0, bx1, bz1]) = (r(&a.rect), r(&b.rect));
                assert!(ax1 < bx0 || bx1 < ax0 || az1 < bz0 || bz1 < az0);
                assert!(
                    ax1.div_euclid(512) < bx0.div_euclid(512)
                        || bx1.div_euclid(512) < ax0.div_euclid(512)
                        || az1.div_euclid(512) < bz0.div_euclid(512)
                        || bz1.div_euclid(512) < az0.div_euclid(512)
                );
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
        // The second is thin and far from the projection's origin.
        let reqs = [
            LLBBox::new(44.40, 26.00, 44.52, 26.22).unwrap(),
            LLBBox::new(44.80, 25.30, 44.82, 25.90).unwrap(),
        ];
        for (req, scale) in reqs
            .iter()
            .flat_map(|r| [1.0, 0.5, 0.25, 0.05].map(|s| (r, s)))
        {
            let proj = WebMercatorProjection::new(44.46, 26.11, scale);
            let world = snap_bbox_to_chunks(&proj, req).unwrap().0;
            if scale == 0.05 && req.min().lat() > 44.7 {
                assert!(plan_units(&proj, &world, 1).unwrap().len() > 1);
            }
            for n in [1, 2] {
                let side = 512 * n;
                let units = plan_units(&proj, &world, n).unwrap();
                let mut area = 0u64;
                for (i, a) in units.iter().enumerate() {
                    let [x0, z0, x1, z1] = r(&a.rect);
                    assert!(
                        x0 >= world.min_x() && x1 <= world.max_x(),
                        "s={scale} n={n}"
                    );
                    assert!(
                        z0 >= world.min_z() && z1 <= world.max_z(),
                        "s={scale} n={n}"
                    );
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
