//! Work units: a large One World selection cut into region-group pieces.
//!
//! The cut lines sit on a lattice of `512 * n` blocks anchored at block 0 of
//! the world's frame, not at the selection, so every piece owns whole region
//! files (no two pieces write one `r.X.Z.mca`) and two jobs over overlapping
//! selections cut the same lines. Each piece's lat/lon bbox snaps back to
//! exactly its rectangle, so a run given that bbox builds that piece.

use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::projection::{llbbox_for_rect, snap_bbox_to_chunks, snap_edge, WebMercatorProjection};

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

/// How a selection becomes whole cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SnapMode {
    /// The whole cells that fit inside the selection.
    #[default]
    FitInside,
    /// The selection grown outward to whole cells.
    Cover,
}

/// A selection snapped to whole cells of the One World it joins.
pub struct RegionSnap {
    /// The bbox to generate: the run's own chunk snap turns it into `rect`.
    pub bbox: LLBBox,
    /// What the run builds, in the frame it will use: whole cells.
    pub rect: XZBBox,
    pub frame: WebMercatorProjection,
    /// The world does not exist yet, so the run must be given the frame's
    /// origin as `--origin`; from the bbox alone it would centre elsewhere.
    pub new_world: bool,
    /// Fit Inside found no whole cell on a side, so that side is one cell
    /// around the selection's centre.
    pub fallback: bool,
}

/// Snaps `requested` to whole cells of `n` x `n` regions, the pieces a job
/// is cut into, on the lattice anchored at block 0 that `plan_units` cuts on.
/// `mode` picks the cells inside the selection or the ones covering it, and
/// `square` makes both sides the same number of cells (the smaller count
/// inside, the larger one covering), centred.
///
/// An existing world, or a new one given `--origin`, snaps to its own
/// lattice. Otherwise the new world's block (0, 0) is placed so the cells
/// are centred on the request: covering, it is the junction at the centre,
/// the same number of cells either side; inside, a side with an odd count
/// moves it half a cell, so the centre is the middle of a cell (still a
/// region junction for an even `n`).
///
/// The bbox sits half a chunk inside each cell line, and the run's
/// outward chunk snap puts the edges back on the lines. That needs the
/// run's frame to be this one, which for a new world means `--origin`.
pub fn snap_to_cells(
    world_dir: &std::path::Path,
    requested: &LLBBox,
    args: &crate::args::Args,
    n: i32,
    mode: SnapMode,
    square: bool,
) -> Result<RegionSnap, String> {
    const MARGIN: f64 = 8.0;
    // A cell edge this close outside the selection still counts as inside,
    // so a snapped bbox (half a chunk in from its lines) snaps to itself.
    const SLACK: f64 = 16.0;
    if n < 1 {
        return Err("a cell is at least one region".to_string());
    }
    let r = f64::from(REGION_BLOCKS * n);
    let new_world = crate::one_world::Manifest::load(world_dir)?.is_none();
    let mut proj = crate::one_world::frame_for(world_dir, requested, args)?;
    let x_w = proj.x_for_lon(requested.min().lng());
    let x_e = proj.x_for_lon(requested.max().lng());
    let z_n = proj.z_for_lat(requested.max().lat());
    let z_s = proj.z_for_lat(requested.min().lat());
    let mut fallback = false;
    // Both counts at least one, flagging the side that had none.
    let mut at_least_one = |c: f64| {
        fallback |= c < 1.0;
        c.max(1.0)
    };
    let (x0, x1, z0, z1) = match (new_world && args.origin.is_none(), mode) {
        (true, SnapMode::Cover) => {
            let half = |a: f64, b: f64| (a.abs().max(b.abs()) / r).ceil().max(1.0);
            let (mut kx, mut kz) = (half(x_w, x_e), half(z_n, z_s));
            if square {
                kx = kx.max(kz);
                kz = kx;
            }
            (-kx, kx, -kz, kz)
        }
        (true, SnapMode::FitInside) => {
            let count = |a: f64, b: f64| ((b - a + 2.0 * SLACK) / r).floor();
            let (mut cx, mut cz) = (count(x_w, x_e), count(z_n, z_s));
            if square {
                cx = cx.min(cz);
                cz = cx;
            }
            let (cx, cz) = (at_least_one(cx), at_least_one(cz));
            // ponytail: the shifted origin changes the frame's scale by about
            // 1e-4 of the selection's size, so "inside" holds to that much.
            let shift = |c: f64| if c % 2.0 == 1.0 { r / 2.0 } else { 0.0 };
            let (dx, dz) = (shift(cx), shift(cz));
            if dx != 0.0 || dz != 0.0 {
                proj = crate::one_world::new_frame(args, proj.lat_for_z(dz), proj.lon_for_x(dx));
            }
            let lo = |c: f64| -(c / 2.0).ceil();
            (lo(cx), lo(cx) + cx, lo(cz), lo(cz) + cz)
        }
        (false, SnapMode::Cover) => {
            let lo = |v: f64| (f64::from(snap_edge(v, false)) / r).floor();
            let hi = |v: f64, from: f64| (f64::from(snap_edge(v, true)) / r).ceil().max(from + 1.0);
            let (mut x0, mut z0) = (lo(x_w), lo(z_n));
            let (mut x1, mut z1) = (hi(x_e, x0), hi(z_s, z0));
            if square {
                let c = (x1 - x0).max(z1 - z0);
                x0 -= ((c - (x1 - x0)) / 2.0).floor();
                z0 -= ((c - (z1 - z0)) / 2.0).floor();
                (x1, z1) = (x0 + c, z0 + c);
            }
            (x0, x1, z0, z1)
        }
        (false, SnapMode::FitInside) => {
            let (mut x0, mut z0) = (((x_w - SLACK) / r).ceil(), ((z_n - SLACK) / r).ceil());
            let mut cx = ((x_e + SLACK) / r).floor() - x0;
            let mut cz = ((z_s + SLACK) / r).floor() - z0;
            if square {
                let c = cx.min(cz);
                x0 += ((cx - c) / 2.0).floor();
                z0 += ((cz - c) / 2.0).floor();
                (cx, cz) = (c, c);
            }
            // No whole cell on a side: the one under the selection's centre.
            if cx < 1.0 {
                x0 = ((x_w + x_e) / 2.0 / r).floor();
            }
            if cz < 1.0 {
                z0 = ((z_n + z_s) / 2.0 / r).floor();
            }
            let (cx, cz) = (at_least_one(cx), at_least_one(cz));
            (x0, x0 + cx, z0, z0 + cz)
        }
    };
    let bbox = LLBBox::new(
        proj.lat_for_z(z1 * r - MARGIN),
        proj.lon_for_x(x0 * r + MARGIN),
        proj.lat_for_z(z0 * r + MARGIN),
        proj.lon_for_x(x1 * r - MARGIN),
    )?;
    let (rect, _) = snap_bbox_to_chunks(&proj, &bbox)?;
    Ok(RegionSnap {
        bbox,
        rect,
        frame: proj,
        new_world,
        fallback,
    })
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

    /// A new world's snap is whole regions either side of the request's
    /// centre, and the run's own frame and chunk snap give back exactly them.
    #[test]
    fn a_selection_snaps_to_whole_cells_around_a_junction() {
        use clap::Parser;
        let dir = tempfile::tempdir().unwrap();
        let (_, req) = bucharest();
        for scale in [1.0, 0.5] {
            let args = crate::args::Args::parse_from(["arnis", &format!("--scale={scale}")]);
            let snap = snap_to_cells(dir.path(), &req, &args, 1, SnapMode::Cover, false).unwrap();
            let (x0, z0, x1, z1) = r(&snap.rect);
            assert!(snap.new_world);
            assert_eq!((x0, z0), (-(x1 + 1), -(z1 + 1)), "symmetric about (0, 0)");
            assert_eq!(x0.rem_euclid(512), 0);
            assert_eq!(z0.rem_euclid(512), 0);
            // The frame the run will make is the one the edges were placed in.
            let centre = (req.min().lat() + req.max().lat()) / 2.0;
            assert!((snap.frame.origin_lat - centre).abs() < 1e-6);
            let (_, units) = plan_units(&snap.frame, &snap.bbox, 1).unwrap();
            assert!(units.iter().all(|u| u.chunks() == 32 * 32), "whole regions");
            for n in [2, 4, 8] {
                let snap =
                    snap_to_cells(dir.path(), &req, &args, n, SnapMode::Cover, false).unwrap();
                let (x0, z0, x1, z1) = r(&snap.rect);
                assert_eq!((x0, z0), (-(x1 + 1), -(z1 + 1)));
                let (_, units) = plan_units(&snap.frame, &snap.bbox, n).unwrap();
                let whole = 32 * 32 * (n * n) as u64;
                assert!(units.iter().all(|u| u.chunks() == whole), "whole cells");
                assert_eq!(units.len() % 4, 0, "even cells per side");
            }
        }
    }

    /// A selection some 30 km tall, run with the origin its snap returns,
    /// builds whole cells on every edge and puts block (0, 0) at that origin.
    #[test]
    fn a_tall_selection_is_whole_cells_on_every_edge() {
        use clap::Parser;
        let dir = tempfile::tempdir().unwrap();
        for lat in [10.0, 45.0, 60.0, -45.0] {
            let req = LLBBox::new(lat - 0.135, 26.0, lat + 0.135, 26.05).unwrap();
            for n in [1, 4] {
                let args = crate::args::Args::parse_from(["arnis"]);
                let snap =
                    snap_to_cells(dir.path(), &req, &args, n, SnapMode::Cover, false).unwrap();
                let origin = format!("{},{}", snap.frame.origin_lat, snap.frame.origin_lon);
                let run = crate::args::Args::parse_from(["arnis", "--origin", &origin]);
                let frame = crate::one_world::frame_for(dir.path(), &snap.bbox, &run).unwrap();
                assert_eq!(
                    (frame.origin_lat, frame.origin_lon),
                    (snap.frame.origin_lat, snap.frame.origin_lon)
                );
                let (rect, _) = snap_bbox_to_chunks(&frame, &snap.bbox).unwrap();
                assert_eq!(r(&rect), r(&snap.rect), "lat {lat} cells {n}");
                let cell = 512 * n;
                let (x0, z0, x1, z1) = r(&rect);
                for v in [x0, z0, x1 + 1, z1 + 1] {
                    assert_eq!(v.rem_euclid(cell), 0, "lat {lat} cells {n}: {:?}", r(&rect));
                }
                assert_eq!((x0, z0), (-(x1 + 1), -(z1 + 1)), "centred on (0, 0)");
                assert!(z1 + 1 - z0 >= 30_000, "covers the request");
                // Snapping again in the pinned frame changes nothing.
                let again =
                    snap_to_cells(dir.path(), &snap.bbox, &run, n, SnapMode::Cover, false).unwrap();
                assert_eq!(r(&again.rect), r(&rect));
            }
        }
    }

    /// Request edges in `frame`: west, east, north, south.
    fn edges(frame: &WebMercatorProjection, req: &LLBBox) -> (f64, f64, f64, f64) {
        (
            frame.x_for_lon(req.min().lng()),
            frame.x_for_lon(req.max().lng()),
            frame.z_for_lat(req.max().lat()),
            frame.z_for_lat(req.min().lat()),
        )
    }

    /// Cells per side of a snapped rect.
    fn cells(rect: &XZBBox, n: i32) -> (i32, i32) {
        let (x0, z0, x1, z1) = r(rect);
        ((x1 + 1 - x0) / (512 * n), (z1 + 1 - z0) / (512 * n))
    }

    /// Fit Inside on a new world: the most whole cells inside the request,
    /// centred on it, on cell lines in the frame the run is pinned to; with
    /// none inside, one cell around the centre.
    #[test]
    fn fit_inside_keeps_whole_cells_inside_and_centred() {
        use clap::Parser;
        let dir = tempfile::tempdir().unwrap();
        let (_, req) = bucharest();
        let args = crate::args::Args::parse_from(["arnis"]);
        for n in [1, 2, 4] {
            let snap =
                snap_to_cells(dir.path(), &req, &args, n, SnapMode::FitInside, false).unwrap();
            let cover = snap_to_cells(dir.path(), &req, &args, n, SnapMode::Cover, false).unwrap();
            let (x0, z0, x1, z1) = r(&snap.rect);
            let (w, e, no, so) = edges(&snap.frame, &req);
            let cell = 512 * n;
            let (cx, cz) = cells(&snap.rect, n);
            // About 1990 x 2000 blocks: three regions, one 1024 cell, no 2048 one.
            let want = [3, 1, 1][n.trailing_zeros() as usize];
            assert_eq!((cx, cz), (want, want), "n {n}");
            assert_eq!(snap.fallback, n == 4, "n {n}");
            if !snap.fallback {
                let tol = 17.0;
                assert!(
                    f64::from(x0) >= w - tol && f64::from(x1 + 1) <= e + tol,
                    "n {n}"
                );
                assert!(
                    f64::from(z0) >= no - tol && f64::from(z1 + 1) <= so + tol,
                    "n {n}"
                );
                let (ccx, ccz) = cells(&cover.rect, n);
                assert!(ccx >= cx && ccz >= cz);
            }
            for v in [x0, z0, x1 + 1, z1 + 1] {
                assert_eq!(v.rem_euclid(cell), 0, "n {n}: on cell lines");
            }
            let mid = |a: i32, b: i32| f64::from(a + b + 1) / 2.0;
            assert!(
                (mid(x0, x1) - (w + e) / 2.0).abs() < 4.0,
                "n {n}: centred east-west"
            );
            assert!(
                (mid(z0, z1) - (no + so) / 2.0).abs() < 4.0,
                "n {n}: centred north-south"
            );
            // The run, pinned to the snap's origin, builds exactly this.
            let origin = format!("{},{}", snap.frame.origin_lat, snap.frame.origin_lon);
            let run = crate::args::Args::parse_from(["arnis", "--origin", &origin]);
            let frame = crate::one_world::frame_for(dir.path(), &snap.bbox, &run).unwrap();
            let (rect, _) = snap_bbox_to_chunks(&frame, &snap.bbox).unwrap();
            assert_eq!(r(&rect), r(&snap.rect), "n {n}");
            let again = snap_to_cells(dir.path(), &snap.bbox, &run, n, SnapMode::FitInside, false);
            assert_eq!(
                r(&again.unwrap().rect),
                r(&snap.rect),
                "n {n}: snaps to itself"
            );
        }
    }

    /// Square: the same count both ways, the smaller one inside and the
    /// larger one covering, on a new world and on a fixed lattice.
    #[test]
    fn square_selection_has_equal_sides() {
        use clap::Parser;
        let dir = tempfile::tempdir().unwrap();
        // About 12 km east-west by 3.3 km north-south.
        let req = LLBBox::new(44.43, 26.0, 44.46, 26.15).unwrap();
        for pinned in [false, true] {
            let args = if pinned {
                crate::args::Args::parse_from(["arnis", "--origin", "44.40,25.97"])
            } else {
                crate::args::Args::parse_from(["arnis"])
            };
            for mode in [SnapMode::FitInside, SnapMode::Cover] {
                let free = snap_to_cells(dir.path(), &req, &args, 1, mode, false).unwrap();
                let sq = snap_to_cells(dir.path(), &req, &args, 1, mode, true).unwrap();
                let ((fx, fz), (sx, sz)) = (cells(&free.rect, 1), cells(&sq.rect, 1));
                assert!(fx > fz, "{mode:?} pinned {pinned}: wide to start with");
                assert_eq!(sx, sz, "{mode:?} pinned {pinned}");
                let want = if mode == SnapMode::FitInside { fz } else { fx };
                assert_eq!(sx, want, "{mode:?} pinned {pinned}");
                // Centred on the request, to within a cell on a fixed lattice.
                let (w, e, _, _) = edges(&sq.frame, &req);
                let mid = f64::from(sq.rect.min_x() + sq.rect.max_x() + 1) / 2.0;
                let tol = if pinned { 512.0 } else { 4.0 };
                assert!(
                    (mid - (w + e) / 2.0).abs() <= tol,
                    "{mode:?} pinned {pinned}"
                );
            }
        }
    }

    /// On a fixed lattice Fit Inside keeps the cells inside, and with none
    /// inside takes the one under the request's centre.
    #[test]
    fn fit_inside_on_a_fixed_lattice() {
        use clap::Parser;
        let dir = tempfile::tempdir().unwrap();
        let (_, req) = bucharest();
        let args = crate::args::Args::parse_from(["arnis", "--origin", "44.40,26.0"]);
        let snap = snap_to_cells(dir.path(), &req, &args, 1, SnapMode::FitInside, false).unwrap();
        let (w, e, no, so) = edges(&snap.frame, &req);
        let (x0, z0, x1, z1) = r(&snap.rect);
        assert!(!snap.fallback);
        assert!(f64::from(x0) >= w - 16.0 && f64::from(x1 + 1) <= e + 16.0);
        assert!(f64::from(z0) >= no - 16.0 && f64::from(z1 + 1) <= so + 16.0);
        // One more cell either way would leave the request.
        assert!(f64::from(x0 - 512) < w - 16.0 && f64::from(x1 + 513) > e + 16.0);
        let again = snap_to_cells(dir.path(), &snap.bbox, &args, 1, SnapMode::FitInside, false);
        assert_eq!(r(&again.unwrap().rect), r(&snap.rect));
        let one = snap_to_cells(dir.path(), &req, &args, 8, SnapMode::FitInside, false).unwrap();
        assert!(one.fallback);
        assert_eq!(cells(&one.rect, 8), (1, 1));
        let (cx, cz) = ((w + e) / 2.0, (no + so) / 2.0);
        let (x0, z0, x1, z1) = r(&one.rect);
        assert!(f64::from(x0) <= cx && cx <= f64::from(x1 + 1));
        assert!(f64::from(z0) <= cz && cz <= f64::from(z1 + 1));
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
