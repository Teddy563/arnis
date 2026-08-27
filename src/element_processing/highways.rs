use crate::args::Args;
use crate::block_definitions::*;
use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::element_processing::bridge_styles::{
    decorate_bridge_above_deck, place_bridge_support_below_deck, BridgePathSample, BridgeStyle,
};
use crate::element_processing::bridges::{BridgeStructureMap, BridgeSurfaceMap};
use crate::element_processing::get_nearest_non_road_block;
use crate::element_processing::surfaces::{
    get_blocks_for_surface, get_blocks_for_surface_way, semirandom_surface,
};
use crate::floodfill_cache::{CoordinateBitmap, FloodFillCache, RoadMaskBitmap};
use crate::osm_parser::{ProcessedElement, ProcessedWay};
use crate::world_editor::WorldEditor;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Upper bound on `block_range` used by wide-road width flattening. The
/// stamp is `2 * block_range + 1`; with `MAX_BLOCK_RANGE = 8` we can sort
/// up to 17 samples on the stack. Keep this generous — a `debug_assert`
/// below catches it if a caller ever exceeds it.
const MAX_BLOCK_RANGE: usize = 8;

/// Median of the ground levels along the road's width-perpendicular
/// strip at one along-length coordinate. Pure primitive — no along-length
/// smoothing. Callers should use `perpendicular_median_ground_y` unless
/// they specifically need the unsmoothed value.
#[inline]
fn perpendicular_median_raw(
    editor: &WorldEditor,
    set_x: i32,
    set_z: i32,
    centerline_x: i32,
    centerline_z: i32,
    block_range: i32,
    dir_horizontal: bool,
) -> i32 {
    debug_assert!(block_range as usize <= MAX_BLOCK_RANGE);
    let len = 2 * block_range as usize + 1;
    // Stack buffer keeps this allocation-free on a hot path that runs
    // millions of times for a city-scale bbox.
    let mut ys = [0i32; 2 * MAX_BLOCK_RANGE + 1];
    if dir_horizontal {
        for (i, t) in (-block_range..=block_range).enumerate() {
            ys[i] = editor.get_ground_level(set_x, centerline_z + t);
        }
    } else {
        for (i, t) in (-block_range..=block_range).enumerate() {
            ys[i] = editor.get_ground_level(centerline_x + t, set_z);
        }
    }
    ys[..len].sort_unstable();
    ys[len / 2]
}

/// Precompute one perpendicular-median Y per axial position in a
/// centerline's stamp. Hot-loop optimization: inside a single centerline
/// point's `(2b+1) × (2b+1)` stamp, every cell that shares a given axial
/// offset (dx for horizontal travel, dz for vertical travel) produces
/// the same target Y — `perpendicular_median_ground_y` ignores the
/// cross-axis position entirely. Computing it once per axial value and
/// reading from this table in the inner loop cuts `get_ground_level`
/// call count by a factor of `2b+1` on the main road-stamp path.
///
/// The table layout maps axial offset `a ∈ [-block_range, block_range]`
/// to index `(a + block_range) as usize`. `out.len()` must be at least
/// `2 * block_range + 1`.
#[inline]
fn precompute_row_medians(
    editor: &WorldEditor,
    centerline_x: i32,
    centerline_z: i32,
    block_range: i32,
    dir_horizontal: bool,
    out: &mut [i32],
) {
    debug_assert!(block_range as usize <= MAX_BLOCK_RANGE);
    let len = 2 * block_range as usize + 1;
    debug_assert!(out.len() >= len);
    for (i, slot) in out[..len].iter_mut().enumerate() {
        let axial = -block_range + i as i32;
        let (sx, sz) = if dir_horizontal {
            (centerline_x + axial, centerline_z)
        } else {
            (centerline_x, centerline_z + axial)
        };
        *slot = perpendicular_median_ground_y(
            editor,
            sx,
            sz,
            centerline_x,
            centerline_z,
            block_range,
            dir_horizontal,
        );
    }
}

/// Median of the ground levels along the road's width-perpendicular strip
/// **at this specific cell's along-length coordinate**. Does NOT sample
/// anything in the travel direction, so the target Y varies naturally
/// along the length of the road (terrain-following) while staying
/// identical across the width at any given length position — meaning
/// every block in one lateral cross-section sits flat (not pitched
/// sideways down a slope).
///
/// A 3-tap median along the road's length axis is layered on top, purely
/// to kill 1-cell terrain noise that would otherwise leave single-block
/// potholes in the road surface (e.g. `…1 1 0 1 1…` → `…1 1 1 1 1…`).
/// A monotone ramp is unaffected because the 3-tap median of any
/// monotonic triple is the middle value.
///
/// - `set_x, set_z` — the cell whose Y we're computing.
/// - `centerline_x, centerline_z` — the current centerline bresenham point.
///   Only the axis perpendicular to travel is used (e.g. `centerline_z`
///   for a horizontal-dominant segment); the cell's own along-length
///   coordinate drives the other axis, which is what makes the sampling
///   cell-specific instead of centerline-specific.
/// - `dir_horizontal` — true when `|dx_segment| >= |dz_segment|`, telling
///   us travel is x-dominant (so perpendicular sampling runs along z).
#[inline]
fn perpendicular_median_ground_y(
    editor: &WorldEditor,
    set_x: i32,
    set_z: i32,
    centerline_x: i32,
    centerline_z: i32,
    block_range: i32,
    dir_horizontal: bool,
) -> i32 {
    let (prev_x, prev_z, next_x, next_z) = if dir_horizontal {
        (set_x - 1, set_z, set_x + 1, set_z)
    } else {
        (set_x, set_z - 1, set_x, set_z + 1)
    };
    let t_prev = perpendicular_median_raw(
        editor,
        prev_x,
        prev_z,
        centerline_x,
        centerline_z,
        block_range,
        dir_horizontal,
    );
    let t_curr = perpendicular_median_raw(
        editor,
        set_x,
        set_z,
        centerline_x,
        centerline_z,
        block_range,
        dir_horizontal,
    );
    let t_next = perpendicular_median_raw(
        editor,
        next_x,
        next_z,
        centerline_x,
        centerline_z,
        block_range,
        dir_horizontal,
    );
    let mut arr = [t_prev, t_curr, t_next];
    arr.sort_unstable();
    arr[1]
}

// ---- Road longitudinal grading (`--road-grade on`) ----
//
// With the flag off none of this runs: `compute_road_profile` returns `None`
// on its first line, `flatten_width` keeps its legacy `block_range >= 1`
// form, and the placement loop takes the unchanged `precompute_row_medians`
// path. Flag-off output is byte-identical.

/// Absolute-`tds` spacing of the hard profile anchors, in stations (K).
///
/// Anchors are the tile-invariance mechanism, not a smoothing knob. Every
/// station whose way-intrinsic `tds` is a multiple of K is pinned to its own
/// terrain sample and the profile is solved independently inside each
/// anchor-delimited window, which bounds the reach of any single DEM sample
/// to K stations. That matters because `Ground::get_data_coordinates` clamps
/// reads to the render bbox: a sample taken outside one Meld cell's bbox is
/// edge-clamped, and clamped differently in the neighbouring cell. Unbounded,
/// the slope clamp would carry that difference the whole length of the way
/// and across the cell; bounded, it dies at the next anchor. `tds` counts
/// from the way's first node and ways are assigned to tiles whole, so the
/// anchor stations are identical in every tile and every cell.
const GRADE_ANCHOR_PERIOD: usize = 64;

/// Max-grade denominator N by highway class: at most one block of climb per
/// N blocks of run. `None` means the class is never graded — `highway=steps`
/// is stairs, and stairs are supposed to step.
///
/// `*_link` ramps inherit their parent class. Classes outside the table take
/// the residential tier rather than going ungraded, so an exotic `highway=*`
/// value never silently keeps the contour steps this pass exists to remove.
fn road_grade_denominator(highway_type: &str) -> Option<u32> {
    match highway_type.strip_suffix("_link").unwrap_or(highway_type) {
        "steps" => None,
        "motorway" | "trunk" | "primary" => Some(12),
        "secondary" | "tertiary" => Some(8),
        "footway" | "path" | "track" => Some(4),
        _ => Some(6),
    }
}

/// Max grade `g` in blocks per block of run for this class at this scale.
///
/// `N_eff = max(2, round(N * scale))`: N is expressed in real-world blocks,
/// so at 1:2 an unscaled 1-in-12 motorway limit would read as 1-in-6 in world
/// blocks. The floor of 2 keeps the rounded profile from stepping every
/// single block at tiny scales, where one block is the whole road.
fn road_grade_step(highway_type: &str, scale: f64) -> Option<f64> {
    let n = road_grade_denominator(highway_type)?;
    let n_eff = ((n as f64 * scale).round() as i64).max(2) as f64;
    Some(1.0 / n_eff)
}

/// One centerline station, indexed by `tds`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct GradeStation {
    x: i32,
    z: i32,
    /// Travel direction of the segment this station came from; selects the
    /// axis the width strip is sampled along, mirroring `dir_horizontal` in
    /// the placement loop.
    dir_horizontal: bool,
}

/// Station list for a way, indexed exactly as the placement loop indexes
/// `tds`.
///
/// The mapping reproduced here is `tds = cumulative_distance_from_start +
/// point_index`, with `cumulative_distance_from_start += segment_length - 1`
/// after each segment, and `skip_first == 0` for every way that can be graded
/// (only bridges and bridge ramps skip, and those are never graded). That
/// `- 1` makes a segment's first point share its `tds` with the previous
/// segment's last point, so a shared node coordinate is written TWICE at the
/// SAME index. This builder must OVERWRITE there and never append: one stray
/// push shifts every later station by one and desyncs `profile[tds]` from
/// placement for the entire rest of the way.
///
/// At a shared station the later segment's `dir_horizontal` wins, matching
/// the placement loop's own last-write ordering. Both writes carry the same
/// coordinate, so only the sampling axis is ever at stake.
fn road_grade_stations(nodes: &[crate::osm_parser::ProcessedNode]) -> Vec<GradeStation> {
    let mut stations: Vec<GradeStation> = Vec::new();
    let mut cumulative_distance_from_start: usize = 0;
    for pair in nodes.windows(2) {
        let points = bresenham_line(pair[0].x, 0, pair[0].z, pair[1].x, 0, pair[1].z);
        let segment_length = points.len();
        let dir_horizontal = (pair[1].x - pair[0].x).abs() >= (pair[1].z - pair[0].z).abs();
        for (point_index, (x, _, z)) in points.iter().enumerate() {
            let tds = cumulative_distance_from_start + point_index;
            let station = GradeStation {
                x: *x,
                z: *z,
                dir_horizontal,
            };
            match stations.get_mut(tds) {
                Some(slot) => *slot = station,
                None => {
                    debug_assert_eq!(tds, stations.len(), "station list must stay dense in tds");
                    stations.push(station);
                }
            }
        }
        cumulative_distance_from_start += segment_length - 1;
    }
    stations
}

/// `tds` of every way node: node 0 at 0, then the running sum of each
/// segment's `segment_length - 1`, which is exactly `max(|dx|, |dz|)`. Same
/// accumulation as `road_grade_stations`, without re-rasterizing.
fn road_grade_node_stations(nodes: &[crate::osm_parser::ProcessedNode]) -> Vec<usize> {
    let mut out = Vec::with_capacity(nodes.len());
    let mut cumulative: usize = 0;
    out.push(0);
    for pair in nodes.windows(2) {
        let dx = (pair[1].x - pair[0].x).unsigned_abs() as usize;
        let dz = (pair[1].z - pair[0].z).unsigned_abs() as usize;
        cumulative += dx.max(dz);
        out.push(cumulative);
    }
    out
}

/// Median of the PRE-ROUND terrain surface across the road's width strip at
/// one station — the float twin of `perpendicular_median_raw`'s geometry
/// (`2 * block_range + 1` samples on the axis perpendicular to travel).
///
/// Reads `terrain_level_f64`, never `get_ground_level`. `get_ground_level`
/// folds in the road override map, which would make this way's Y depend on
/// which roads were processed before it — the order- and tile-coupled
/// feedback loop this pass exists to sever. Sampling the float field rather
/// than `terrain_level` is the other half of the fix: the 0.5-contour
/// crossings that rounding creates ARE the mid-segment steps being removed,
/// so the profile has to be built before they exist.
fn grade_strip_median(
    editor: &WorldEditor,
    station: GradeStation,
    block_range: i32,
) -> Option<f64> {
    debug_assert!(block_range as usize <= MAX_BLOCK_RANGE);
    let len = 2 * block_range as usize + 1;
    let mut ys = [0.0f64; 2 * MAX_BLOCK_RANGE + 1];
    for (i, t) in (-block_range..=block_range).enumerate() {
        let (sx, sz) = if station.dir_horizontal {
            (station.x, station.z + t)
        } else {
            (station.x + t, station.z)
        };
        ys[i] = editor.terrain_level_f64(sx, sz)?;
    }
    ys[..len].sort_by(f64::total_cmp);
    Some(ys[len / 2])
}

/// Junction pin: median of the pre-round terrain surface over a fixed 3x3
/// window at the node.
///
/// Deliberately independent of road width, class and way direction, so every
/// way meeting at this node — in every tile and every Meld cell — derives the
/// identical pin from (node coords, DEM) alone. That is what makes crossing
/// roads agree at the shared column with no ordering input of any kind.
fn grade_junction_pin(editor: &WorldEditor, x: i32, z: i32) -> Option<f64> {
    let mut ys = [0.0f64; 9];
    let mut i = 0;
    for dx in -1..=1 {
        for dz in -1..=1 {
            ys[i] = editor.terrain_level_f64(x + dx, z + dz)?;
            i += 1;
        }
    }
    ys.sort_by(f64::total_cmp);
    Some(ys[4])
}

/// Slope-limited profile over ONE anchor-delimited window.
///
/// `constraints` is `(index within the window, value)` in ascending index
/// order and includes the window's boundary anchors. Closed form, no
/// iteration to a fixpoint, so the result cannot depend on an iteration
/// count:
/// - per-edge grade caps `gstep`, relaxed to the constraint-pair linear grade
///   on any interval whose two constraints are steeper than `g` — an
///   infeasible pair must not make the solve unsatisfiable, and a relaxed
///   interval is still never steeper than the ungraded terrain it replaces;
/// - envelopes `L[i] = max_j(c_j - d_ij)`, `U[i] = min_j(c_j + d_ij)`;
/// - `A[i] = min_j(base[j] + d_ij)`, the largest g-Lipschitz minorant of
///   base, and `B[i] = max_j(base[j] - d_ij)`, the smallest majorant;
/// - `p = clamp((A + B) / 2, L, U)`.
///
/// `d_ij` is the sum of `gstep` over the edges between i and j, so each field
/// is a two-sweep O(n) infimal convolution. Min, max and clamp of g-Lipschitz
/// functions are g-Lipschitz, so `p` respects the grade everywhere; at a
/// constraint `L == U == c`, so constraints are held exactly. `d_ij` is
/// symmetric in i and j and the two sweeps are mirror images of each other,
/// so reversing the input mirrors the output bit for bit — which matters
/// because OSM way direction is arbitrary.
fn grade_solve_window(base: &[f64], constraints: &[(usize, f64)], g: f64) -> Vec<f64> {
    let m = base.len();
    if m == 0 {
        return Vec::new();
    }

    // Per-edge grade cap. Edge e joins stations e and e + 1.
    let mut gstep = vec![g; m.saturating_sub(1)];
    for pair in constraints.windows(2) {
        let (ia, ca) = pair[0];
        let (ib, cb) = pair[1];
        if ib <= ia {
            continue;
        }
        let needed = (cb - ca).abs() / (ib - ia) as f64;
        if needed > g {
            for step in &mut gstep[ia..ib] {
                *step = needed;
            }
        }
    }

    let mut upper = vec![f64::INFINITY; m];
    let mut lower = vec![f64::NEG_INFINITY; m];
    for &(i, c) in constraints {
        upper[i] = c;
        lower[i] = c;
    }
    let mut minorant = base.to_vec();
    let mut majorant = base.to_vec();

    for i in 1..m {
        let step = gstep[i - 1];
        upper[i] = upper[i].min(upper[i - 1] + step);
        lower[i] = lower[i].max(lower[i - 1] - step);
        minorant[i] = minorant[i].min(minorant[i - 1] + step);
        majorant[i] = majorant[i].max(majorant[i - 1] - step);
    }
    for i in (0..m.saturating_sub(1)).rev() {
        let step = gstep[i];
        upper[i] = upper[i].min(upper[i + 1] + step);
        lower[i] = lower[i].max(lower[i + 1] - step);
        minorant[i] = minorant[i].min(minorant[i + 1] + step);
        majorant[i] = majorant[i].max(majorant[i + 1] - step);
    }

    let mut profile: Vec<f64> = (0..m)
        .map(|i| {
            let mid = 0.5 * (minorant[i] + majorant[i]);
            // `L <= U` holds by the triangle inequality along the relaxed
            // constraint chain; `.max().min()` rather than `clamp()` so float
            // jitter can never turn into a panic.
            mid.max(lower[i]).min(upper[i])
        })
        .collect();

    // Restate constraints bit-exactly. The clamp above already lands on them
    // to within float epsilon, but epsilon is not enough here: two ways
    // meeting at a junction whose pin sits on an exact .5 would round to
    // different integers off a 1-ulp difference — the very disagreement the
    // pin exists to prevent.
    for &(i, c) in constraints {
        profile[i] = c;
    }
    profile
}

/// Whole-way profile: anchors every `GRADE_ANCHOR_PERIOD` stations, junction
/// pins as extra hard constraints, one independent solve per anchor-delimited
/// window.
///
/// Windows share their boundary anchor station and both solves reproduce the
/// constraint value there exactly, so the concatenation is continuous. A pin
/// landing on an anchor index REPLACES that anchor: the pin is the value
/// every way at that junction must share, and being a hard constraint it
/// bounds DEM influence exactly as well as the anchor it replaces.
fn grade_profile(base: &[f64], pins: &BTreeMap<usize, f64>, g: f64) -> Vec<f64> {
    let n = base.len();
    if n == 0 {
        return Vec::new();
    }

    let mut constraints: BTreeMap<usize, f64> = (0..n)
        .step_by(GRADE_ANCHOR_PERIOD)
        .map(|i| (i, base[i]))
        .collect();
    for (&i, &v) in pins {
        if i < n {
            constraints.insert(i, v);
        }
    }

    let mut bounds: Vec<usize> = (0..n).step_by(GRADE_ANCHOR_PERIOD).collect();
    if bounds[bounds.len() - 1] != n - 1 {
        bounds.push(n - 1);
    }

    let mut profile = vec![0.0f64; n];
    if bounds.len() == 1 {
        profile[0] = constraints[&0];
        return profile;
    }
    for w in bounds.windows(2) {
        let (lo, hi) = (w[0], w[1]);
        let local: Vec<(usize, f64)> = constraints
            .range(lo..=hi)
            .map(|(&i, &c)| (i - lo, c))
            .collect();
        let solved = grade_solve_window(&base[lo..=hi], &local, g);
        profile[lo..=hi].copy_from_slice(&solved);
    }
    profile
}

/// Per-way longitudinal profile in float blocks, indexed by `tds`, or `None`
/// when the way is not graded: flag off, bridge or bridge ramp (their deck Y
/// comes from `y_at` and they must never register ground overrides),
/// `highway=steps`, fewer than two nodes, or no terrain data at all.
///
/// Rounded to i32 only at placement. Rounding a slope-limited profile puts
/// its 1-block steps at least `N_eff` blocks apart — the voxel-minimum ramp —
/// instead of clustering them on terrain contour crossings.
#[allow(clippy::too_many_arguments)]
fn compute_road_profile(
    editor: &WorldEditor,
    way: &ProcessedWay,
    highway_type: &str,
    args: &Args,
    connectivity: &HighwayConnectivityMap,
    block_range: i32,
    is_bridge: bool,
    total_bresenham_length: usize,
) -> Option<Vec<f64>> {
    if args.road_grade != "on" || is_bridge || way.nodes.len() < 2 {
        return None;
    }
    let g = road_grade_step(highway_type, args.scale)?;

    let stations = road_grade_stations(&way.nodes);
    debug_assert_eq!(
        stations.len(),
        total_bresenham_length,
        "station list must span exactly the placement loop's tds range"
    );
    if stations.len() != total_bresenham_length {
        // A desync would misalign `profile[tds]` from placement for the whole
        // way; fall back to the legacy median path rather than place garbage.
        return None;
    }

    let sample_range = block_range.clamp(0, MAX_BLOCK_RANGE as i32);
    let mut base = Vec::with_capacity(stations.len());
    for station in &stations {
        base.push(grade_strip_median(editor, *station, sample_range)?);
    }

    let mut pins: BTreeMap<usize, f64> = BTreeMap::new();
    for (node, &tds) in way
        .nodes
        .iter()
        .zip(road_grade_node_stations(&way.nodes).iter())
    {
        if tds < base.len() && connectivity.is_junction((node.x, node.z)) {
            if let Some(pin) = grade_junction_pin(editor, node.x, node.z) {
                pins.insert(tds, pin);
            }
        }
    }

    Some(grade_profile(&base, &pins, g))
}

/// Default block-mix used for road surfaces when no `surface=*` tag is
/// present. Kept as a constant so the `semirandom_surface` call sites read
/// consistently across the file.
const DEFAULT_ROAD_MIX: &[Block] = &[GRAY_CONCRETE_POWDER, CYAN_TERRACOTTA];

/// Blocks that a road write must NOT overwrite. Intentionally narrow:
/// - `GRAY_CONCRETE_POWDER`, `CYAN_TERRACOTTA`: the default asphalt mix,
///   preserved so two asphalt roads overlapping produce a consistent
///   surface instead of re-rolling the hash per pass.
/// - `WHITE_CONCRETE`: preserves lane stripes and zebra crossings from
///   being erased when a later road pass crosses them.
/// - `BLACK_CONCRETE`: not produced by highways directly, but widely
///   placed by other element processors — schoolyards in `leisure.rs`,
///   gas-station / parking forecourts in `amenities.rs`, some landuse
///   patches. A highway shouldn't paint over those.
///
/// Any other hard-surface block a way places (`SMOOTH_STONE` for
/// pedestrian footways, `BRICK`, `OAK_PLANKS`, `LIGHT_GRAY_CONCRETE`,
/// `STONE_BRICKS`, etc.) is left out so major roads can freely pave
/// over them when their footprints overlap, keeping the road surface
/// clean end-to-end.
const ROAD_PROTECTED_SURFACES: &[Block] = &[
    BLACK_CONCRETE,
    GRAY_CONCRETE_POWDER,
    CYAN_TERRACOTTA,
    WHITE_CONCRETE,
    // Bridge module furniture must survive parallel side-deck ways.
    WARPED_STAIRS,
    WARPED_TRAPDOOR,
    WARPED_SLAB,
    STRIPPED_WARPED_STEM,
    STRIPPED_WARPED_HYPHAE,
    SEA_LANTERN,
    ANDESITE_WALL,
    SMOOTH_SANDSTONE_STAIRS,
];
// Parking-on-road overlap is handled differently: amenities render AFTER
// highways (`data_processing.rs:172` then `:197`), so parking is the
// final writer. Parking's own whitelist now includes `WHITE_CONCRETE`,
// so when road dividers paint white on top of road asphalt and then
// parking flood-fills the area, parking re-paints over WHITE too —
// erasing the "lane stripes through parking spots" without needing
// to blacklist GRAY_CONCRETE here (which would also block roads from
// overwriting footway-laid GRAY_CONCRETE at sidewalk/road junctions).

/// True when the way should render as a pedestrian walkway
/// rather than asphalt.
fn is_pedestrian_way(element: &ProcessedElement) -> bool {
    let tags = element.tags();
    if let Some(h) = tags.get("highway") {
        if matches!(h.as_str(), "footway" | "pedestrian" | "steps") {
            return true;
        }
    }
    // `footway=*` subtag (sidewalk, crossing, access_aisle, traffic_island,
    // yes, …) implies a pedestrian way. Exclude the explicit `footway=no`,
    // which is occasionally used on roads to assert "this is not a footway".
    matches!(tags.get("footway").map(|s| s.as_str()), Some(v) if v != "no")
}

/// Decide whether to skip this OSM highway element under the active
/// `--road-detail` setting.
///
///   "max"     → never skip (current full-detail behaviour).
///   "compact" → skip pedestrian-grade highways
///               (footway / path / cycleway / steps / corridor /
///                pedestrian / platform / bus_stop), skip crossing
///               markers, skip footway=crossing zebra ways.
///   "none"    → skip every highway element (terrain-only worlds).
///
/// Compact mode targets low-scale renders (1 block ≥ 1.5 m) where
/// pedestrian features compress onto the same blocks as vehicle roads
/// and produce dotted-checker visual noise at intersections.
fn road_detail_skip(highway_type: &str, tags: &HashMap<String, String>, detail: &str) -> bool {
    match detail {
        "compact" => {
            // Pedestrian-grade + low-traffic highway types collapse
            // visually at low scale and add overdraw at intersections
            // without giving the road network extra legibility.
            // `service` (driveways, parking aisles) + `track`
            // (agricultural roads) join the skip list at this resolution.
            if matches!(
                highway_type,
                "footway"
                    | "path"
                    | "cycleway"
                    | "steps"
                    | "corridor"
                    | "pedestrian"
                    | "platform"
                    | "bus_stop"
                    | "service"
                    | "track"
            ) {
                return true;
            }
            // Crossing markers (zebra, traffic-signal nodes, etc.) sit
            // on top of vehicle lanes and produce checker patterns at
            // sub-meter block resolution.
            if highway_type == "crossing" {
                return true;
            }
            if tags.get("crossing").is_some() {
                return true;
            }
            if matches!(tags.get("footway").map(|s| s.as_str()), Some("crossing")) {
                return true;
            }
            false
        }
        _ => false, // "max" never skips
    }
}

/// Type alias for highway connectivity map
/// Highway node connectivity, built once per render from ALL parsed elements.
///
/// Two views over the same node scan, kept separate so the legacy slope
/// logic stays byte-identical with road grading off OR on:
/// - `endpoint_layers`: way ENDPOINTS only — the historical map consumed by
///   `should_add_slope_at_node`. Folding mid-way nodes in here would change
///   slope decisions for overpass ramps, i.e. change output with the flag off.
/// - `node_occurrences`: occurrence count over ALL nodes of ALL highway ways
///   (G5). A coordinate seen >= 2 times is a junction — including mid-way
///   T-junctions — used for road-grade junction pins. A pure function of the
///   parsed element set: whole-element tile assignment plus Meld's
///   seam-expanded fetch give every tile/cell the same junction set.
pub struct HighwayConnectivityMap {
    endpoint_layers: HashMap<(i32, i32), Vec<i32>>,
    node_occurrences: HashMap<(i32, i32), u32>,
}

impl HighwayConnectivityMap {
    /// Legacy endpoint view: layers of ways starting/ending at this coord.
    fn endpoint_layers(&self, coord: &(i32, i32)) -> Option<&Vec<i32>> {
        self.endpoint_layers.get(coord)
    }

    /// Legacy "no connectivity information" probe (endpoint view only).
    fn endpoints_is_empty(&self) -> bool {
        self.endpoint_layers.is_empty()
    }

    /// True when >= 2 highway-way node occurrences share this coordinate
    /// (mid-way nodes included). Closed ways count their closure coordinate
    /// twice and self-intersections count each visit — both deterministic,
    /// both legitimately pinned by road grading.
    pub fn is_junction(&self, coord: (i32, i32)) -> bool {
        self.node_occurrences.get(&coord).copied().unwrap_or(0) >= 2
    }
}

// 4-connected stair fill from `prev` (exclusive) to `curr` (inclusive).
fn stair_fill_cells(prev: (i32, i32), curr: (i32, i32)) -> Vec<(i32, i32)> {
    let mut cells = Vec::with_capacity(2);
    let (mut x, mut z) = prev;
    while x != curr.0 || z != curr.1 {
        if x != curr.0 {
            x += (curr.0 - x).signum();
            cells.push((x, z));
        }
        if z != curr.1 {
            z += (curr.1 - z).signum();
            cells.push((x, z));
        }
    }
    if cells.is_empty() {
        cells.push(curr);
    }
    cells
}

// Absolute base Y for a node feature; deck Y on a bridge, else terrain + layer_boost.
// `bridge_radius`: 0 = exact (lamps, bus stops, on-road signal head), >0 = nearby (off-road
// signal pole/bars where the anchor sits next to the deck rather than on it).
#[inline]
fn node_feature_base_y(
    editor: &WorldEditor,
    bridge_surface: &BridgeSurfaceMap,
    x: i32,
    z: i32,
    layer_boost: i32,
    bridge_radius: i32,
) -> i32 {
    bridge_surface
        .nearby_deck_y(x, z, bridge_radius)
        .unwrap_or_else(|| editor.get_absolute_y(x, layer_boost, z))
}

/// Generates highways with elevation support based on layer tags and connectivity analysis
#[allow(clippy::too_many_arguments)]
pub fn generate_highways(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &HighwayConnectivityMap,
    flood_fill_cache: &FloodFillCache,
    road_mask: &RoadMaskBitmap,
    bridge_structures: &BridgeStructureMap,
    bridge_surface: &BridgeSurfaceMap,
    tunnel_internal_endpoints: &TunnelInternalEndpoints,
    tunnel_cells: &mut Vec<HighwayTunnelCell>,
) {
    // Publish the flag to the override-fold semantics in `world_editor`. Here
    // rather than at the render entry point because highway processing is the
    // only writer of the road-override map, so this necessarily runs before
    // the first `register_road_surface_y` and before any tile merge, on every
    // path (sequential and tiled alike). `set_road_grade` is a first-value-
    // wins `OnceLock`, so the repeat calls are an atomic load each.
    crate::world_editor::set_road_grade(args.road_grade == "on");

    // Highway tunnels render a covered shell instead of a surface road.
    if let ProcessedElement::Way(way) = element {
        if renders_as_highway_tunnel(way) {
            if args.road_detail == "none" {
                return;
            }
            generate_highway_tunnel_shell(
                editor,
                way,
                args,
                tunnel_internal_endpoints,
                tunnel_cells,
            );
            return;
        }
    }
    generate_highways_internal(
        editor,
        element,
        args,
        highway_connectivity,
        flood_fill_cache,
        road_mask,
        bridge_structures,
        bridge_surface,
    );
}

/// Build a connectivity map for highway endpoints to determine where slopes are needed,
/// plus the all-node occurrence view used for road-grade junction pins.
pub fn build_highway_connectivity_map(elements: &[ProcessedElement]) -> HighwayConnectivityMap {
    let mut connectivity_map: HashMap<(i32, i32), Vec<i32>> = HashMap::new();
    let mut node_occurrences: HashMap<(i32, i32), u32> = HashMap::new();

    for element in elements {
        if let ProcessedElement::Way(way) = element {
            if way.tags.contains_key("highway") {
                let layer_value = way
                    .tags
                    .get("layer")
                    .and_then(|layer| layer.parse::<i32>().ok())
                    .unwrap_or(0);

                // Treat negative layers as ground level (0) for connectivity
                let layer_value = if layer_value < 0 { 0 } else { layer_value };

                // Add connectivity for start and end nodes
                if !way.nodes.is_empty() {
                    let start_node = &way.nodes[0];
                    let end_node = &way.nodes[way.nodes.len() - 1];

                    let start_coord = (start_node.x, start_node.z);
                    let end_coord = (end_node.x, end_node.z);

                    connectivity_map
                        .entry(start_coord)
                        .or_default()
                        .push(layer_value);
                    connectivity_map
                        .entry(end_coord)
                        .or_default()
                        .push(layer_value);
                }

                // All-node view (endpoints included) so mid-way T-junctions
                // pin too. Kept out of `connectivity_map` — see the struct doc.
                for node in &way.nodes {
                    *node_occurrences.entry((node.x, node.z)).or_insert(0) += 1;
                }
            }
        }
    }

    HighwayConnectivityMap {
        endpoint_layers: connectivity_map,
        node_occurrences,
    }
}

// ---- Highway tunnels ----

pub struct HighwayTunnelCell {
    pub x: i32,
    pub z: i32,
    pub road_y: i32,
    pub half_width: i32,
    pub covered: bool,
    pub terrain_y: i32,
    pub palette: &'static [Block],
    pub light: bool,
}

pub type TunnelInternalEndpoints = HashSet<(i32, i32)>;

const TUNNEL_CEIL_OFFSET: i32 = 5; // roof height above the road
const TUNNEL_COVER_DROP: i32 = 7; // min cover to earn a roof
const TUNNEL_RAMP_STEP: i32 = 1; // max descent per cell
const TUNNEL_RAMP_RUN: i32 = 3; // portal ramp run per 1 block drop
const TUNNEL_LAYER_DROP: i32 = 7; // extra depth per layer below -1
const TUNNEL_LIGHT_INTERVAL: usize = 8;

// Cracked/mossy stone-brick speckle for tunnel walls and roof.
fn tunnel_shell_block(x: i32, y: i32, z: i32) -> Block {
    let h = (x as u32)
        .wrapping_mul(73856093)
        .wrapping_add((y as u32).wrapping_mul(19349663))
        .wrapping_add((z as u32).wrapping_mul(83492791));
    match h % 100 {
        0..=14 => CRACKED_STONE_BRICKS,
        15..=17 => MOSSY_STONE_BRICKS,
        _ => STONE_BRICKS,
    }
}

// A highway way that should render as an underground tunnel.
fn renders_as_highway_tunnel(way: &ProcessedWay) -> bool {
    if !way.tags.contains_key("highway") || way.nodes.len() < 2 {
        return false;
    }
    if way.tags.get("tunnel").map(String::as_str) != Some("yes") {
        return false;
    }
    if way.tags.get("indoor").map(String::as_str) == Some("yes")
        || way.tags.get("area").map(String::as_str) == Some("yes")
    {
        return false;
    }
    if way
        .tags
        .get("level")
        .and_then(|l| l.parse::<i32>().ok())
        .is_some_and(|l| l < 0)
    {
        return false;
    }
    !matches!(
        way.tags.get("highway").map(String::as_str),
        Some("street_lamp" | "crossing" | "bus_stop" | "proposed" | "construction" | "razed")
    )
}

// Endpoints shared by 2+ tunnel ways; these stay at depth instead of ramping up.
pub fn collect_tunnel_internal_endpoints(elements: &[ProcessedElement]) -> TunnelInternalEndpoints {
    let mut counts: HashMap<(i32, i32), u32> = HashMap::new();
    for elem in elements {
        let ProcessedElement::Way(w) = elem else {
            continue;
        };
        if !renders_as_highway_tunnel(w) {
            continue;
        }
        let s = &w.nodes[0];
        let e = &w.nodes[w.nodes.len() - 1];
        *counts.entry((s.x, s.z)).or_default() += 1;
        if (e.x, e.z) != (s.x, s.z) {
            *counts.entry((e.x, e.z)).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter_map(|(k, c)| (c > 1).then_some(k))
        .collect()
}

// Phase 1: place the tunnel shell and record cells; the interior is carved in phase 2.
pub fn generate_highway_tunnel_shell(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    args: &Args,
    internal_endpoints: &TunnelInternalEndpoints,
    tunnel_cells: &mut Vec<HighwayTunnelCell>,
) {
    let Some(highway_type) = way.tags.get("highway") else {
        return;
    };

    // Centerline cells, consecutive duplicates dropped.
    let mut pts: Vec<(i32, i32)> = Vec::new();
    for w in way.nodes.windows(2) {
        for (bx, _, bz) in &bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z) {
            if pts.last() != Some(&(*bx, *bz)) {
                pts.push((*bx, *bz));
            }
        }
    }
    if pts.len() < 2 {
        return;
    }
    let n = pts.len();
    let last = n - 1;

    // Raw DEM keeps road_y identical when a way is reprocessed across tiles.
    let terrain_ys: Vec<i32> = pts
        .iter()
        .map(|&(x, z)| {
            editor
                .terrain_level(x, z)
                .unwrap_or_else(|| editor.get_ground_level(x, z))
        })
        .collect();
    let start_ground = terrain_ys[0];
    let end_ground = terrain_ys[last];
    let start_internal = internal_endpoints.contains(&pts[0]);
    let end_internal = internal_endpoints.contains(&pts[last]);
    let denom = last.max(1) as f32;
    let layer = way
        .tags
        .get("layer")
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    // Saturating: `layer` is an untrusted tag and could be i32::MIN.
    let below = (-(layer.saturating_add(1))).max(0);
    let layer_extra = below.saturating_mul(TUNNEL_LAYER_DROP);

    let half_width = highway_block_range(highway_type, &way.tags, args.scale);
    let wall_off = half_width + 1;

    // As deep as cover needs, ramping down from open portals.
    let mut road_y: Vec<i32> = Vec::with_capacity(n);
    #[allow(clippy::needless_range_loop)]
    for i in 0..n {
        let t = i as f32 / denom;
        let grade = (start_ground as f32 + (end_ground - start_ground) as f32 * t).round() as i32;
        let cover_tgt = terrain_ys[i] - TUNNEL_COVER_DROP;
        // Deepen the target, not the portal, so the ramp still reaches ground.
        let desired = grade.min(cover_tgt).saturating_sub(layer_extra);
        let ramp_s = if start_internal {
            i32::MIN
        } else {
            start_ground - i as i32 / TUNNEL_RAMP_RUN
        };
        let ramp_e = if end_internal {
            i32::MIN
        } else {
            end_ground - (last - i) as i32 / TUNNEL_RAMP_RUN
        };
        let y = desired.max(ramp_s.max(ramp_e)).min(terrain_ys[i]);
        road_y.push(y);
    }
    // Clamp under terrain, then cap the slope by only ever digging deeper.
    for i in 0..n {
        road_y[i] = road_y[i].min(terrain_ys[i]);
    }
    for i in 1..n {
        road_y[i] = road_y[i].min(road_y[i - 1] + TUNNEL_RAMP_STEP);
    }
    for i in (0..last).rev() {
        road_y[i] = road_y[i].min(road_y[i + 1] + TUNNEL_RAMP_STEP);
    }

    let default_palette: &'static [Block] = match highway_type.as_str() {
        "footway" | "pedestrian" | "service" | "steps" => &[GRAY_CONCRETE],
        "path" => &[DIRT_PATH],
        _ => DEFAULT_ROAD_MIX,
    };
    let palette = get_blocks_for_surface_way(way, default_palette);

    for i in 0..n {
        let (bx, bz) = pts[i];
        let ry = road_y[i];
        let ty = terrain_ys[i];
        if ry - 1 <= crate::world_editor::MIN_Y {
            continue;
        }
        let covered = ty >= ry + TUNNEL_COVER_DROP;
        let ceil_y = ry + TUNNEL_CEIL_OFFSET;
        let top = if covered { ceil_y } else { ty };

        // Square footprint like the subway; the road row is a placeholder, laid in phase 2.
        for dx in -wall_off..=wall_off {
            for dz in -wall_off..=wall_off {
                let is_side_wall = dx.abs() == wall_off || dz.abs() == wall_off;
                for y in (ry - 1)..=top {
                    let block = if is_side_wall || (covered && y == ceil_y) {
                        tunnel_shell_block(bx + dx, y, bz + dz)
                    } else {
                        STONE_BRICKS // foundation, road row, and interior placeholder
                    };
                    editor.set_block_absolute(block, bx + dx, y, bz + dz, None, None);
                }
            }
        }
        tunnel_cells.push(HighwayTunnelCell {
            x: bx,
            z: bz,
            road_y: ry,
            half_width,
            covered,
            terrain_y: ty,
            palette,
            light: covered && i.is_multiple_of(TUNNEL_LIGHT_INTERVAL),
        });
    }
}

// Phase 2: carve the interior, then lay the road last so no carve can eat it.
pub fn carve_highway_tunnel_interior(editor: &mut WorldEditor, tunnel_cells: &[HighwayTunnelCell]) {
    const COVERED_WL: &[Block] = &[
        STONE_BRICKS,
        CRACKED_STONE_BRICKS,
        MOSSY_STONE_BRICKS,
        STONE,
        WATER,
    ];
    // Open-cut cells also clear surface-road asphalt that spilled over the mouth trench.
    const OPEN_WL: &[Block] = &[
        STONE_BRICKS,
        CRACKED_STONE_BRICKS,
        MOSSY_STONE_BRICKS,
        STONE,
        WATER,
        GRAY_CONCRETE_POWDER,
        CYAN_TERRACOTTA,
        GRAY_CONCRETE,
        BLACK_CONCRETE,
        LIGHT_GRAY_CONCRETE,
        WHITE_CONCRETE,
    ];
    // Whitelist for laying the floor over carved air, placeholder, or fill stone.
    const ROAD_WL: &[Block] = &[
        AIR,
        STONE,
        STONE_BRICKS,
        CRACKED_STONE_BRICKS,
        MOSSY_STONE_BRICKS,
        WATER,
    ];

    for cell in tunnel_cells {
        if cell.road_y - 1 <= crate::world_editor::MIN_Y {
            continue;
        }
        let ceil_y = cell.road_y + TUNNEL_CEIL_OFFSET;
        let top = if cell.covered {
            ceil_y - 1
        } else {
            cell.terrain_y
        };
        let wl = if cell.covered { COVERED_WL } else { OPEN_WL };
        let hw = cell.half_width;
        for dx in -hw..=hw {
            for dz in -hw..=hw {
                for y in (cell.road_y + 1)..=top {
                    editor.set_block_absolute(AIR, cell.x + dx, y, cell.z + dz, Some(wl), None);
                }
            }
        }
        if cell.light {
            editor.set_block_absolute(SEA_LANTERN, cell.x, ceil_y - 1, cell.z, None, None);
        }
    }

    for cell in tunnel_cells {
        if cell.road_y - 1 <= crate::world_editor::MIN_Y {
            continue;
        }
        let hw = cell.half_width;
        for dx in -hw..=hw {
            for dz in -hw..=hw {
                let surf = semirandom_surface(cell.x + dx, cell.z + dz, cell.palette);
                editor.set_block_absolute(
                    surf,
                    cell.x + dx,
                    cell.road_y,
                    cell.z + dz,
                    Some(ROAD_WL),
                    None,
                );
            }
        }
    }
}

// Tunnel-bore footprint, to keep the water depth-carve and vegetation off it.
pub fn collect_tunnel_footprint(
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    scale: f64,
) -> CoordinateBitmap {
    if !elements
        .iter()
        .any(|e| matches!(e, ProcessedElement::Way(w) if renders_as_highway_tunnel(w)))
    {
        return CoordinateBitmap::new_empty();
    }
    let mut bitmap = CoordinateBitmap::new(xzbbox);
    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };
        if !renders_as_highway_tunnel(way) {
            continue;
        }
        let Some(highway_type) = way.tags.get("highway") else {
            continue;
        };
        let wall = highway_block_range(highway_type, &way.tags, scale) + 1;
        for w in way.nodes.windows(2) {
            for (bx, _, bz) in &bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z) {
                for dx in -wall..=wall {
                    for dz in -wall..=wall {
                        bitmap.set(bx + dx, bz + dz);
                    }
                }
            }
        }
    }
    bitmap
}

/// Internal function that generates highways with connectivity context for elevation handling
/// A single street lamp: smooth-stone base, stone-brick-wall post, a lit redstone
/// lamp behind an iron-trapdoor hood.
fn place_street_lamp(editor: &mut WorldEditor, x: i32, z: i32, base: i32) {
    editor.set_block_absolute(SMOOTH_STONE, x, base + 1, z, None, None);
    for dy in 2..=4 {
        editor.set_block_absolute(STONE_BRICK_WALL, x, base + dy, z, None, None);
    }
    editor.set_block_with_properties_absolute(
        BlockWithProperties::new(REDSTONE_LAMP, Some(fastnbt::nbt!({ "lit": "true" }))),
        x,
        base + 5,
        z,
        None,
        None,
    );
    editor.set_block_absolute(IRON_TRAPDOOR, x, base + 6, z, None, None);
}

const WAY_LAMP_INTERVAL: usize = 25;

/// Periodic street lamps alongside a `lit=yes` way, alternating sides and
/// skipping road/water cells.
fn place_way_lamps(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    block_range: i32,
    road_mask: &RoadMaskBitmap,
) {
    let offset = block_range + 2;
    let mut tds: usize = 0;
    let mut side = 1i32;
    for w in way.nodes.windows(2) {
        let (dx, dz) = (w[1].x - w[0].x, w[1].z - w[0].z);
        let len = dx.abs().max(dz.abs());
        if len == 0 {
            continue;
        }
        let mag = (dx as f64).hypot(dz as f64);
        let (px, pz) = (
            (-dz as f64 / mag).round() as i32,
            (dx as f64 / mag).round() as i32,
        );
        for (bx, _, bz) in bresenham_line(w[0].x, 0, w[0].z, w[1].x, 0, w[1].z) {
            if tds > 0 && tds.is_multiple_of(WAY_LAMP_INTERVAL) {
                for s in [side, -side] {
                    let (lx, lz) = (bx + px * offset * s, bz + pz * offset * s);
                    if !road_mask.contains(lx, lz) && !editor.is_lc_water(lx, lz) {
                        let base = editor.get_absolute_y(lx, 0, lz);
                        place_street_lamp(editor, lx, lz, base);
                        side = -side;
                        break;
                    }
                }
            }
            tds += 1;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_highways_internal(
    editor: &mut WorldEditor,
    element: &ProcessedElement,
    args: &Args,
    highway_connectivity: &HighwayConnectivityMap,
    flood_fill_cache: &FloodFillCache,
    road_mask: &RoadMaskBitmap,
    bridge_structures: &BridgeStructureMap,
    bridge_surface: &BridgeSurfaceMap,
) {
    // Shared `indoor=yes` / layer parsing for the whole function. Indoor
    // highways must never produce elevated geometry (they sit inside
    // buildings), and features like street lamps on an explicit
    // `layer=*` should ride up with the bridge/overpass they belong to.
    let is_indoor = element.tags().get("indoor").is_some_and(|v| v == "yes");
    let layer_value_raw = element
        .tags()
        .get("layer")
        .and_then(|layer| layer.parse::<i32>().ok())
        .unwrap_or(0);
    // Negative layers map to ground level: undergrounds are out of
    // scope and their markers shouldn't sink below terrain.
    let layer_value_effective = if is_indoor || layer_value_raw < 0 {
        0
    } else {
        layer_value_raw
    };
    const LAYER_HEIGHT_STEP: i32 = 6;
    let layer_boost = layer_value_effective * LAYER_HEIGHT_STEP;

    if let Some(highway_type) = element.tags().get("highway") {
        // Phase: --road-detail gate. Drop pedestrian-grade highways +
        // crossings entirely when running in `compact` (low-scale) or
        // `none` mode to prevent block-resolution collapse on roads.
        // `max` mode (default) never skips.
        if road_detail_skip(highway_type, element.tags(), &args.road_detail) {
            return;
        }
        if highway_type == "street_lamp" {
            if let ProcessedElement::Node(first_node) = element {
                let x: i32 = first_node.x;
                let z: i32 = first_node.z;
                let base = node_feature_base_y(editor, bridge_surface, x, z, layer_boost, 0);
                place_street_lamp(editor, x, z, base);
            }
        } else if highway_type == "crossing" {
            // Handle traffic signals for crossings
            if let Some(crossing_type) = element.tags().get("crossing") {
                if crossing_type == "traffic_signals" {
                    if let ProcessedElement::Node(node) = element {
                        let x = node.x;
                        let z = node.z;
                        let head_base =
                            node_feature_base_y(editor, bridge_surface, x, z, layer_boost, 0);

                        // Try to build a hanging signal if it's on a road
                        let anchor = road_mask
                            .contains(x, z)
                            .then(|| get_nearest_non_road_block(x, z, 4, road_mask))
                            .flatten();

                        match anchor {
                            Some((ax, az)) => {
                                let pole_base = node_feature_base_y(
                                    editor,
                                    bridge_surface,
                                    ax,
                                    az,
                                    layer_boost,
                                    4,
                                );
                                editor.set_block_absolute(
                                    COBBLESTONE_WALL,
                                    ax,
                                    pole_base + 1,
                                    az,
                                    None,
                                    None,
                                );
                                editor.set_block_absolute(
                                    IRON_BARS,
                                    ax,
                                    pole_base + 2,
                                    az,
                                    None,
                                    None,
                                );
                                editor.set_block_absolute(
                                    IRON_BARS,
                                    ax,
                                    pole_base + 3,
                                    az,
                                    None,
                                    None,
                                );
                                editor.set_block_absolute(
                                    IRON_BARS,
                                    ax,
                                    pole_base + 4,
                                    az,
                                    None,
                                    None,
                                );
                                editor.set_block_absolute(
                                    IRON_BARS,
                                    ax,
                                    pole_base + 5,
                                    az,
                                    None,
                                    None,
                                );

                                let bar_y_a = head_base + 6;
                                for (lx, _, lz) in bresenham_line(x, bar_y_a, z, ax, bar_y_a, az) {
                                    let bar_base = node_feature_base_y(
                                        editor,
                                        bridge_surface,
                                        lx,
                                        lz,
                                        layer_boost,
                                        4,
                                    );
                                    editor.set_block_absolute(
                                        IRON_BARS,
                                        lx,
                                        bar_base + 6,
                                        lz,
                                        None,
                                        None,
                                    );
                                }
                            }
                            None => {
                                editor.set_block_absolute(
                                    COBBLESTONE_WALL,
                                    x,
                                    head_base + 1,
                                    z,
                                    None,
                                    None,
                                );
                                editor.set_block_absolute(
                                    IRON_BARS,
                                    x,
                                    head_base + 2,
                                    z,
                                    None,
                                    None,
                                );
                                editor.set_block_absolute(
                                    IRON_BARS,
                                    x,
                                    head_base + 3,
                                    z,
                                    None,
                                    None,
                                );
                            }
                        }

                        editor.set_block_absolute(BLACK_WOOL, x, head_base + 4, z, None, None);
                        editor.set_block_absolute(BLACK_WOOL, x, head_base + 5, z, None, None);

                        const BANNER_PATTERNS: &[(&str, &str)] = &[
                            ("red", "minecraft:triangle_top"),
                            ("lime", "minecraft:triangle_bottom"),
                            ("yellow", "minecraft:circle"),
                            ("black", "minecraft:curly_border"),
                            ("black", "minecraft:border"),
                        ];

                        let banner_y = head_base + 5;
                        let banner_offsets: [(i32, i32, &str); 4] = [
                            (0, -1, "north"),
                            (0, 1, "south"),
                            (-1, 0, "west"),
                            (1, 0, "east"),
                        ];
                        for (dx, dz, facing) in &banner_offsets {
                            editor.place_wall_banner(
                                LIGHT_GRAY_WALL_BANNER,
                                x + dx,
                                banner_y,
                                z + dz,
                                facing,
                                "light_gray",
                                BANNER_PATTERNS,
                            );
                        }
                    }
                }
            }
        } else if highway_type == "bus_stop" {
            if let ProcessedElement::Node(node) = element {
                let x = node.x;
                let z = node.z;
                let base = node_feature_base_y(editor, bridge_surface, x, z, layer_boost, 0);
                for dy in 1..=3 {
                    editor.set_block_absolute(COBBLESTONE_WALL, x, base + dy, z, None, None);
                }

                editor.set_block_absolute(WHITE_WOOL, x, base + 4, z, None, None);
                let neighbor_base =
                    node_feature_base_y(editor, bridge_surface, x + 1, z, layer_boost, 1);
                editor.set_block_absolute(WHITE_WOOL, x + 1, neighbor_base + 4, z, None, None);
            }
        } else if element
            .tags()
            .get("area")
            .is_some_and(|v: &String| v == "yes")
        {
            let ProcessedElement::Way(way) = element else {
                return;
            };

            // Handle areas like pedestrian plazas. Unified surface handling
            // via the shared surfaces module.
            let surface_block: Block = get_blocks_for_surface_way(way, &[STONE])[0];

            // Fill the area using flood fill cache
            let filled_area = flood_fill_cache.get_or_compute(way, args.timeout.as_ref());

            for &(x, z) in filled_area.iter() {
                editor.set_block(surface_block, x, 0, z, None, None);
            }
        } else {
            let mut previous_node: Option<(i32, i32)> = None;
            // Default surface mix. Overridden below based on highway_type or
            // an explicit surface=* tag via `get_blocks_for_surface`.
            let mut block_types: &[Block] = DEFAULT_ROAD_MIX;
            let mut block_range: i32 = 2;
            // default_lanes == 2 for highway types that historically had a
            // center stripe; flipped to `lanes > 1` check below after we
            // resolve the lanes tag. Keeps the same visual default.
            let mut default_lanes: i32 = 1;
            let scale_factor = args.scale;

            // Reuse the function-level layer resolution (already normalised
            // to 0 for indoor/negative).
            let layer_value = layer_value_effective;

            // Skip if 'level' is negative in the tags (indoor mapping)
            if let Some(level) = element.tags().get("level") {
                if level.parse::<i32>().unwrap_or(0) < 0 {
                    return;
                }
            }

            // Determine block type and range based on highway type
            match highway_type.as_str() {
                "footway" | "pedestrian" => {
                    block_types = &[GRAY_CONCRETE];
                    block_range = 1;
                }
                "path" => {
                    block_types = &[DIRT_PATH];
                    block_range = 1;
                }
                "motorway" | "primary" | "trunk" => {
                    block_range = 5;
                    default_lanes = 2;
                }
                "secondary" => {
                    block_range = 4;
                    default_lanes = 2;
                }
                "tertiary" => {
                    default_lanes = 2;
                }
                "track" => {
                    block_range = 1;
                }
                "service" => {
                    block_types = &[GRAY_CONCRETE];
                    block_range = 2;
                }
                "secondary_link" | "tertiary_link" => {
                    //Exit ramps, sliproads
                    block_range = 1;
                }
                "escape" => {
                    // Sand trap for vehicles on mountainous roads
                    block_types = &[SAND];
                    block_range = 1;
                }
                "steps" => {
                    //TODO: Add correct stairs respecting height, step_count, etc.
                    block_types = &[GRAY_CONCRETE];
                    block_range = 1;
                }

                _ => {
                    if let Some(lanes) = element.tags().get("lanes") {
                        if lanes == "2" {
                            block_range = 3;
                            default_lanes = 2;
                        } else if lanes != "1" {
                            block_range = 4;
                            default_lanes = 2;
                        }
                    }
                }
            }

            let ProcessedElement::Way(way) = element else {
                return;
            };

            let bridge_member = bridge_structures.lookup_member(way.id);
            let bridge_ramp = bridge_structures.lookup_ramp(way.id);
            // Redundant side deck under a wider module bridge: render nothing.
            if bridge_member.is_some_and(|m| m.covered_by_wider) {
                return;
            }
            let is_bridge_member = bridge_member.is_some();
            let is_bridge_ramp = bridge_ramp.is_some();
            // Low-detail / downscaled worlds (<= 0.3): force the plain flat Beam deck so the arch
            // spandrel curve and tall truss/suspension/cable decorations never draw (paired with
            // the flat clearance=0 deck from BridgeStructureMap::build).
            let bridge_style = if args.scale <= 0.3 {
                BridgeStyle::Beam
            } else {
                bridge_member.map(|m| m.style).unwrap_or(BridgeStyle::Beam)
            };
            let bridge_start_is_boundary = bridge_member
                .map(|m| m.start_is_group_boundary)
                .unwrap_or(true);
            let bridge_end_is_boundary = bridge_member
                .map(|m| m.end_is_group_boundary)
                .unwrap_or(true);
            let bridge_foundation_block = bridge_style.foundation_block();
            let bridge_rail_block_choice = bridge_style.rail_block();

            // Optional surface override via the OSM `surface=*` tag. Applies to
            // all road types; for single-block surfaces like concrete or sand
            // the mix degenerates to that one block, so `semirandom_surface`
            // always returns the same value.
            if let Some(blocks) = element
                .tags()
                .get("surface")
                .and_then(|s| get_blocks_for_surface(s))
            {
                block_types = blocks;
            }

            // Pedestrian walkways tagged with a paved surface render as
            // smooth stone, overriding the `surface=*` palette. Real-world
            // sidewalks in concrete or paving stones read as uniformly grey
            // from a distance, not as an asphalt speckle, so this gives
            // them a distinct look from the roads they run alongside.
            if is_pedestrian_way(element)
                && matches!(
                    element.tags().get("surface").map(|s| s.as_str()),
                    Some("concrete" | "paving_stones" | "sett")
                )
            {
                block_types = &[SMOOTH_STONE];
            }

            // Optional explicit width via `width=*` (meters ≈ blocks).
            // Clamped to the terrain-flattening helper's sample-buffer cap.
            if let Some(w) = element
                .tags()
                .get("width")
                .and_then(|w| w.parse::<f32>().ok())
            {
                block_range = ((w / 2.0).round() as i32).clamp(1, MAX_BLOCK_RANGE as i32);
            }

            // Resolve lane-marking count. `lane_markings=no` disables them,
            // `lanes=*` overrides the default for this highway type.
            // Multi-lane inner dividers are drawn for lanes >= 2 (one line
            // between every pair of adjacent lanes).
            //
            // Clamped to a realistic upper bound: the world's widest real
            // roads have ~12 lanes, but an `i32` parse will accept
            // arbitrary OSM values. Without the cap, a stray `lanes=999999`
            // tag (typo or vandalism) would send the inner divider loop
            // into millions of iterations per bresenham point.
            const MAX_LANES: i32 = 16;
            let mut lanes = element
                .tags()
                .get("lanes")
                .and_then(|l| l.parse::<i32>().ok())
                .unwrap_or(default_lanes)
                .clamp(0, MAX_LANES);
            if element.tags().get("lane_markings").map(|s| s.as_str()) == Some("no") {
                lanes = 1;
            }

            if scale_factor < 1.0 {
                block_range = ((block_range as f64) * scale_factor).floor() as i32;
            }

            // Untagged road/path crossing water at small scale: a 1-block causeway (or a raised
            // embankment) straight across a lake/river looks worse than the water, so drown it —
            // skip the road on water cells and let the water carve flow through continuously. Kept
            // in sync with collect_road_surface_coords (same `scale <= 0.5` gate + same water test)
            // so road_mask agrees and no dry air gap is left. Tagged bridges are unaffected.
            let drown_over_water = !is_bridge_member && !is_bridge_ramp && scale_factor <= 0.5;

            // Street lamps along explicitly-lit ways. This is below the
            // road_detail_skip early-return, so compact/none ways never reach
            // here and way-lamps respect --road-detail for free.
            if way.tags.get("lit").map(String::as_str) == Some("yes")
                && !is_bridge_member
                && !is_bridge_ramp
                && !is_indoor
                && layer_value_effective == 0
                && highway_type.as_str() != "steps"
            {
                place_way_lamps(editor, way, block_range, road_mask);
            }

            // Elevation based on layer (already normalised; `LAYER_HEIGHT_STEP`
            // is defined at the top of this function).
            let base_elevation = layer_boost;

            // Check if we need slopes at start and end
            // This is used for overpasses that need ramps to ground-level roads
            let needs_start_slope =
                should_add_slope_at_node(&way.nodes[0], layer_value, highway_connectivity);
            let needs_end_slope = should_add_slope_at_node(
                &way.nodes[way.nodes.len() - 1],
                layer_value,
                highway_connectivity,
            );

            let total_way_length = calculate_way_length(way);

            // Unique bresenham points; sum of max per segment + 1 (no shared-endpoint double count).
            let total_bresenham_length: usize = way
                .nodes
                .windows(2)
                .map(|pair| {
                    let dx = (pair[1].x - pair[0].x).unsigned_abs() as usize;
                    let dz = (pair[1].z - pair[0].z).unsigned_abs() as usize;
                    dx.max(dz)
                })
                .sum::<usize>()
                + 1;
            let bridge_internal_ramp_length: usize = {
                let raw = (total_bresenham_length as f32 * 0.35).clamp(15.0, 50.0) as usize;
                let cap = (total_bresenham_length / 2).max(1);
                raw.clamp(1, cap)
            };

            // Plain beam bridges get a swept segment-schematic deck instead;
            // only the structure's widest member carries it.
            let bridge_module = bridge_member
                .and_then(|m| m.module_idx)
                .and_then(crate::element_processing::bridge_modules::module_at);
            let bridge_structure_moduled = bridge_member
                .map(|m| m.structure_has_module)
                .unwrap_or(false);

            let is_short_isolated_elevated = !is_bridge_member
                && !is_bridge_ramp
                && needs_start_slope
                && needs_end_slope
                && layer_value > 0
                && total_way_length <= 35;

            let (effective_elevation, effective_start_slope, effective_end_slope) =
                if is_bridge_member || is_bridge_ramp || is_short_isolated_elevated {
                    (0, false, false)
                } else {
                    (base_elevation, needs_start_slope, needs_end_slope)
                };

            let slope_length = (total_way_length as f32 * 0.35).clamp(15.0, 50.0) as usize;

            // Check if this is a marked zebra crossing (only depends on tags, compute once)
            let is_zebra_crossing = highway_type == "footway"
                && element.tags().get("footway").map(|s| s.as_str()) == Some("crossing")
                && !matches!(
                    element.tags().get("crossing").map(|s| s.as_str()),
                    Some("no" | "unmarked")
                )
                && element.tags().get("crossing:markings").map(|s| s.as_str()) != Some("no");

            // Longitudinal grade profile, indexed by `tds`. Built here because
            // this is the last point where the way's full node list, the final
            // `block_range` and the exact `total_bresenham_length` are all in
            // hand and nothing cell-local has been touched yet. `None` with
            // the flag off, so everything below stays on the legacy path.
            let road_profile: Option<Vec<f64>> = compute_road_profile(
                editor,
                way,
                highway_type.as_str(),
                args,
                highway_connectivity,
                block_range,
                is_bridge_member || is_bridge_ramp,
                total_bresenham_length,
            );

            // Iterate over nodes to create the highway
            let mut segment_index = 0;
            let total_segments = way.nodes.len() - 1;
            // Cumulative bresenham distance across all segments; drives bridge ramp interp.
            let mut cumulative_distance_from_start: usize = 0;
            // Previous bridge cell Y for steep-step gap fill.
            let mut previous_bridge_y: Option<i32> = None;
            // Centerline samples captured for above-deck decoration after the segment loop.
            let mut bridge_path: Vec<BridgePathSample> = Vec::new();
            // Previous rail cell per side; used to orthogonally connect diagonal steps.
            let mut previous_rail_left: Option<(i32, i32)> = None;
            let mut previous_rail_right: Option<(i32, i32)> = None;

            for node in &way.nodes {
                if let Some(prev) = previous_node {
                    let (x1, z1) = prev;
                    let x2: i32 = node.x;
                    let z2: i32 = node.z;

                    // Generate the line of coordinates between the two nodes
                    let bresenham_points: Vec<(i32, i32, i32)> =
                        bresenham_line(x1, 0, z1, x2, 0, z2);

                    // Calculate elevation for this segment
                    let segment_length = bresenham_points.len();

                    // Travel direction for this segment. The perpendicular
                    // median sampling runs along the *other* axis, so that
                    // lateral cross-sections end up level while the road's
                    // Y still varies along length as the terrain climbs /
                    // descends.
                    let dir_horizontal = (x2 - x1).abs() >= (z2 - z1).abs();

                    // Whether wide-road Y-flattening applies to this
                    // segment. Bridges and 1-cell paths keep their legacy
                    // per-call behaviour; everything else gets the
                    // perpendicular median via
                    // `perpendicular_median_ground_y`.
                    // Grading extends this gate to `block_range == 0`: at
                    // reduced scale the range floors to 0, and such roads get
                    // no flattening and register no ground override at all
                    // today. With a profile they take a graded Y and register
                    // their 1-wide column like every other road.
                    let flatten_width = !is_bridge_member
                        && !is_bridge_ramp
                        && (block_range >= 1 || (road_profile.is_some() && block_range >= 0));
                    // Whether the road cross-section also registers an
                    // effective-ground override is decided per bresenham
                    // point below — `offset` varies inside a segment (slope
                    // ramps at layer transitions), and elevated sections
                    // (offset > 0) must NOT register, otherwise
                    // `ground_generation` fills terrain all the way up to
                    // the deck and bridges become giant embankments.

                    // Variables to manage dashed line pattern.
                    //
                    // Upstream: dash = gap = (5 * scale).ceil(). At
                    // scale 0.5 this collapses to 3/3, which on a wide
                    // boulevard with multiple parallel dividers reads
                    // as a checker pattern instead of clean dashes.
                    // Compact mode floors both to >=4 so dashes stay
                    // legible at low scale while max mode preserves
                    // upstream behaviour byte-for-byte.
                    let mut stripe_length: i32 = 0;
                    let base_dash: i32 = (5.0 * scale_factor).ceil() as i32;
                    let (dash_length, gap_length): (i32, i32) = if args.road_detail == "compact" {
                        (base_dash.max(4), base_dash.max(4))
                    } else {
                        (base_dash, base_dash)
                    };

                    // Segment-constants for multi-lane divider placement.
                    // Computed once here instead of at every bresenham point:
                    // `seg_len` needs a sqrt and all the perpendicular-unit-
                    // vector math is identical across the whole segment.
                    // `None` means there are no inner dividers to draw (either
                    // a single-lane road or a degenerate zero-length segment).
                    //
                    // Lane dividers (centre dashed stripe) render fine
                    // even at coarse block resolution because they run
                    //
                    // Compact mode (scale < 0.7): cap divider count to 1
                    // (single centre stripe) regardless of OSM `lanes`.
                    // The effective road width compresses to ~3-4 blocks
                    // at low scale; fitting `lanes - 1` parallel dashed
                    // stripes inside that band collapses into a white
                    // checker. Single centre stripe stays readable.
                    //
                    // Clean mode (scale ≥ 0.7): cap divider count to {1, 2}.
                    // - `lanes ≤ 4` → 1 centre stripe (was 1-3)
                    // - `lanes ≥ 5` → 2 stripes 1 block apart down centre
                    //                  (was 4+; reads as divided highway,
                    //                   like a real-world solid double
                    //                   centerline)
                    // - parking_aisle (`service=parking_aisle`) → 0 stripes
                    //                  (parking lots become clean asphalt)
                    //
                    // Max mode: upstream `lanes - 1` divider count.
                    let is_parking_aisle = matches!(
                        element.tags().get("service").map(|s| s.as_str()),
                        Some("parking_aisle")
                    );
                    // Pedestrian-grade highway types (footway / path / sidewalk
                    // tagged as highway / cycleway / etc.) get zero lane
                    // dividers in clean mode — they're walkways, not roads.
                    // Without this gate, footways tagged as `highway=*` would
                    // render with a centre stripe down the cobblestone, which
                    // looks wrong in-game.
                    let is_pedestrian_grade = matches!(
                        highway_type.as_str(),
                        "footway"
                            | "path"
                            | "cycleway"
                            | "steps"
                            | "corridor"
                            | "pedestrian"
                            | "platform"
                            | "bus_stop"
                            | "track"
                    );
                    let effective_lanes: i32 = match args.road_detail.as_str() {
                        "compact" => lanes.min(2),
                        "clean" => {
                            if is_parking_aisle
                                || highway_type.as_str() == "service"
                                || is_pedestrian_grade
                            {
                                1 // service / pedestrian ways: zero dividers
                            } else if lanes >= 5 {
                                3 // 2 dividers
                            } else {
                                2 // 1 divider (centre)
                            }
                        }
                        _ => lanes, // "max" — upstream
                    };
                    // Twin-centre-stripe geometry for clean mode on wide roads.
                    // When two stripes both ride within 1 block of centre,
                    // they read as a "divided road" marker rather than as
                    // two evenly-spread lane dividers. Apply only in clean
                    // mode for `lanes ≥ 5`.
                    let twin_centre_stripe = args.road_detail == "clean"
                        && lanes >= 5
                        && !is_parking_aisle
                        && !is_pedestrian_grade
                        && highway_type.as_str() != "service";
                    // Width gate: roads narrower than 4 blocks total don't
                    // get a centre stripe in clean / compact mode. A 3-block
                    // road has only 1 cell on either side of centre, so a
                    // dashed centre stripe leaves no asphalt visible
                    // adjacent to it — looks like a single-block-wide
                    // dashed path, not a road. Small turn lanes / service
                    // streets / one-way alleys all hit this cap.
                    // Max mode preserves upstream behaviour (no width gate).
                    let road_width_total = 2 * block_range + 1;
                    let width_gate_pass = match args.road_detail.as_str() {
                        "max" => true,
                        _ => road_width_total >= 4,
                    };
                    let lane_divider_geom = if effective_lanes >= 2 && width_gate_pass {
                        let dx_seg = (x2 - x1) as f32;
                        let dz_seg = (z2 - z1) as f32;
                        let seg_len = (dx_seg * dx_seg + dz_seg * dz_seg).sqrt();
                        if seg_len > 0.0 {
                            let road_width_blocks = road_width_total as f32;
                            Some((
                                -dz_seg / seg_len,                          // perp_x
                                dx_seg / seg_len,                           // perp_z
                                road_width_blocks / effective_lanes as f32, // lane_width
                                road_width_blocks / 2.0,                    // half_width
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // Unit perpendicular for this segment, used by bridge rail placement.
                    let bridge_rail_perp: Option<(f32, f32)> = if is_bridge_member || is_bridge_ramp
                    {
                        let dx_seg = (x2 - x1) as f32;
                        let dz_seg = (z2 - z1) as f32;
                        let seg_len = (dx_seg * dx_seg + dz_seg * dz_seg).sqrt();
                        if seg_len > 0.0 {
                            Some((-dz_seg / seg_len, dx_seg / seg_len))
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // Bridges/ramps drive their Y from cumulative tds, so skip the duplicate
                    // shared endpoint on later segments. Non-bridge slope offsets keep the
                    // legacy calculate_point_elevation indexing, which expects every point.
                    let skip_first = if (is_bridge_member || is_bridge_ramp) && segment_index > 0 {
                        1
                    } else {
                        0
                    };
                    for (point_index, (x, _, z)) in
                        bresenham_points.iter().enumerate().skip(skip_first)
                    {
                        let tds = cumulative_distance_from_start + point_index;
                        let bridge_y_here = bridge_member
                            .map(|info| {
                                info.y_at(tds, total_bresenham_length, bridge_internal_ramp_length)
                            })
                            .or_else(|| {
                                bridge_ramp.map(|info| info.y_at(tds, total_bresenham_length))
                            });

                        let offset = if is_bridge_member || is_bridge_ramp {
                            0
                        } else {
                            calculate_point_elevation(
                                segment_index,
                                point_index,
                                segment_length,
                                total_segments,
                                effective_elevation,
                                effective_start_slope,
                                effective_end_slope,
                                slope_length,
                            )
                        };

                        let register_ground_override = flatten_width && offset == 0;

                        let use_absolute_y = is_bridge_member || is_bridge_ramp || flatten_width;

                        // Precompute per-axial-offset perpendicular medians
                        // once for this centerline. Every cell in the stamp
                        // that shares an axial offset picks up the same
                        // value — without this cache, we'd recompute the
                        // full 3-tap median (which itself touches ~15
                        // ground samples) for every `(dx, dz)` cell, making
                        // wide-road rendering O(width²) per centerline.
                        let mut row_medians = [0i32; 2 * MAX_BLOCK_RANGE + 1];
                        if let Some(profile) = road_profile.as_deref() {
                            // Graded ways take one Y for the entire
                            // cross-section from `tds` alone, so no terrain
                            // sampling happens here at all. Being a pure
                            // function of `tds` is also what structurally
                            // removes the two secondary step sources: the
                            // diagonal-travel stamp overlap and the
                            // per-segment `dir_horizontal` axis flip can no
                            // longer hand one column two different Ys.
                            debug_assert!(
                                tds < profile.len(),
                                "profile must span every placement tds"
                            );
                            let graded = profile[tds.min(profile.len() - 1)].round() as i32;
                            row_medians[..2 * block_range as usize + 1].fill(graded);
                        } else if flatten_width {
                            precompute_row_medians(
                                editor,
                                *x,
                                *z,
                                block_range,
                                dir_horizontal,
                                &mut row_medians,
                            );
                        }

                        // Only Arch actually reads this; other styles re-sample inside place_pillar.
                        let centerline_ground_y =
                            if is_bridge_member && matches!(bridge_style, BridgeStyle::Arch) {
                                editor.get_ground_level(*x, *z)
                            } else {
                                0
                            };

                        if is_bridge_member {
                            if let (Some(by), Some(perp)) = (bridge_y_here, bridge_rail_perp) {
                                bridge_path.push((*x, by, *z, perp));
                            }
                        }

                        // Backfill steep ramp steps where deck+foundation alone leaves an air band.
                        if let Some(by) = bridge_y_here {
                            if let Some(prev_y) = previous_bridge_y {
                                let (fill_lo, fill_hi) = if by >= prev_y + 3 {
                                    (prev_y + 1, by - 2)
                                } else if by <= prev_y - 3 {
                                    (by + 1, prev_y - 2)
                                } else {
                                    (0, -1)
                                };
                                if fill_lo <= fill_hi {
                                    for fill_y in fill_lo..=fill_hi {
                                        for fdx in -block_range..=block_range {
                                            for fdz in -block_range..=block_range {
                                                editor.set_block_absolute(
                                                    STONE_BRICKS,
                                                    *x + fdx,
                                                    fill_y,
                                                    *z + fdz,
                                                    None,
                                                    Some(ROAD_PROTECTED_SURFACES),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            previous_bridge_y = Some(by);
                        }

                        // Draw the road surface for the entire width
                        for dx in -block_range..=block_range {
                            for dz in -block_range..=block_range {
                                let set_x: i32 = x + dx;
                                let set_z: i32 = z + dz;

                                // Drown untagged small-scale crossings over water (see
                                // drown_over_water): skip the surface AND the ground override so
                                // the water carve renders continuous water here instead of a
                                // 1-block causeway line.
                                if drown_over_water && editor.is_lc_water(set_x, set_z) {
                                    continue;
                                }

                                // Per-cell Y. For wide roads this is the
                                // perpendicular median at the cell's own
                                // along-length coord — so all cells at the
                                // same along-length coord share one Y
                                // (flat cross-section) and register the
                                // same effective-ground override.
                                let cell_y = if let Some(by) = bridge_y_here {
                                    by
                                } else if flatten_width {
                                    let axial = if dir_horizontal { dx } else { dz };
                                    row_medians[(axial + block_range) as usize] + offset
                                } else {
                                    offset
                                };
                                if register_ground_override {
                                    editor.register_road_surface_y(set_x, set_z, cell_y);
                                }

                                // Zebra crossing logic. Background uses the
                                // default asphalt mix (not the footway's own
                                // surface), matching main's pre-rebase
                                // behaviour — a zebra crossing is painted on
                                // the underlying road, so it reads more
                                // naturally against the road mix than the
                                // footway's single grey.
                                if is_zebra_crossing {
                                    // 2-block-wide stripe / 2-block-wide gap.
                                    // The previous 1/1 pattern collapsed into
                                    // a solid white bar from anything but
                                    // ground level, defeating the "this is a
                                    // crossing" visual cue. 2/2 reads as
                                    // distinct white bars at any view height.
                                    let on_stripe = if dir_horizontal {
                                        (set_x.rem_euclid(4)) < 2
                                    } else {
                                        (set_z.rem_euclid(4)) < 2
                                    };
                                    if on_stripe {
                                        // White bar. Whitelist the mix we
                                        // place for the non-bar cells so the
                                        // bar only replaces zebra background.
                                        if use_absolute_y {
                                            editor.set_block_absolute(
                                                WHITE_CONCRETE,
                                                set_x,
                                                cell_y,
                                                set_z,
                                                Some(DEFAULT_ROAD_MIX),
                                                None,
                                            );
                                        } else {
                                            editor.set_block(
                                                WHITE_CONCRETE,
                                                set_x,
                                                cell_y,
                                                set_z,
                                                Some(DEFAULT_ROAD_MIX),
                                                None,
                                            );
                                        }
                                    } else {
                                        // Non-bar cell: asphalt mix.
                                        // No whitelist — the zebra's footway
                                        // way may render before the
                                        // intersecting road has painted
                                        // asphalt at this cell, so requiring
                                        // existing asphalt would make the
                                        // crossing invisible. Edge-bleed onto
                                        // adjacent cobble is constrained by
                                        // the footway way's own bbox; well-
                                        // tagged crossings stay narrow.
                                        let bg = semirandom_surface(set_x, set_z, DEFAULT_ROAD_MIX);
                                        if use_absolute_y {
                                            editor.set_block_absolute(
                                                bg, set_x, cell_y, set_z, None, None,
                                            );
                                        } else {
                                            editor.set_block(bg, set_x, cell_y, set_z, None, None);
                                        }
                                    }
                                } else {
                                    // Unified surface selection. For single-block
                                    // surfaces (concrete, sand, dirt_path...),
                                    // `block_types` is a 1-element slice so
                                    // every cell picks the same block; for
                                    // multi-block mixes (default road, asphalt)
                                    // the hash scatters the blocks randomly.
                                    // Blacklist is the narrow asphalt-mix set
                                    // defined in ROAD_PROTECTED_SURFACES — see
                                    // its doc comment for the overlap-handling
                                    // rationale.
                                    let effective_block =
                                        semirandom_surface(set_x, set_z, block_types);
                                    if use_absolute_y {
                                        editor.set_block_absolute(
                                            effective_block,
                                            set_x,
                                            cell_y,
                                            set_z,
                                            None,
                                            Some(ROAD_PROTECTED_SURFACES),
                                        );
                                    } else {
                                        editor.set_block(
                                            effective_block,
                                            set_x,
                                            cell_y,
                                            set_z,
                                            None,
                                            Some(ROAD_PROTECTED_SURFACES),
                                        );
                                    }
                                }

                                // Add stone brick foundation and support pillars only for
                                // genuinely elevated decks — bridges and explicit overpasses.
                                // (Regular wide roads now flow through `use_absolute_y == true`
                                // too, but they aren't floating decks; they get embankments
                                // from the registered ground-surface override instead.)
                                let is_elevated_deck = (is_bridge_member
                                    && !bridge_structure_moduled)
                                    || is_bridge_ramp
                                    || effective_elevation > 0;
                                if is_elevated_deck && cell_y > 0 {
                                    // Foundation: stone bricks for everything except wooden boardwalks.
                                    let foundation = if is_bridge_member {
                                        bridge_foundation_block
                                    } else {
                                        STONE_BRICKS
                                    };
                                    if use_absolute_y {
                                        editor.set_block_absolute(
                                            foundation,
                                            set_x,
                                            cell_y - 1,
                                            set_z,
                                            None,
                                            None,
                                        );
                                    } else {
                                        editor.set_block(
                                            foundation,
                                            set_x,
                                            cell_y - 1,
                                            set_z,
                                            None,
                                            None,
                                        );
                                    }

                                    if is_bridge_member {
                                        let interval = bridge_style.pillar_interval();
                                        let is_center = dx == 0 && dz == 0;
                                        // Beam keeps the legacy (x+z) rule; other styles use
                                        // path-index so spacing stays consistent on diagonals.
                                        let is_pillar = is_center
                                            && interval > 0
                                            && match bridge_style {
                                                BridgeStyle::Beam => {
                                                    (set_x + set_z).rem_euclid(interval as i32) == 0
                                                }
                                                _ => tds.is_multiple_of(interval),
                                            };
                                        place_bridge_support_below_deck(
                                            editor,
                                            bridge_style,
                                            set_x,
                                            cell_y,
                                            set_z,
                                            centerline_ground_y,
                                            tds,
                                            total_bresenham_length,
                                            use_absolute_y,
                                            is_center,
                                            is_pillar,
                                        );
                                    } else if use_absolute_y {
                                        add_highway_support_pillar_absolute(
                                            editor,
                                            set_x,
                                            cell_y,
                                            set_z,
                                            dx,
                                            dz,
                                            block_range,
                                        );
                                    } else {
                                        add_highway_support_pillar(
                                            editor,
                                            set_x,
                                            cell_y,
                                            set_z,
                                            dx,
                                            dz,
                                            block_range,
                                        );
                                    }
                                }
                            }
                        }

                        // Side railings; stair_fill_cells keeps the rail 4-connected on diagonals.
                        if let (Some(by), Some((perp_x, perp_z))) =
                            (bridge_y_here, bridge_rail_perp)
                        {
                            // L1-projected stamp extent + 1, so the rail never lands on the deck.
                            let rail_dist =
                                block_range as f32 * (perp_x.abs() + perp_z.abs()) + 1.0;
                            for (sign, prev_state) in [
                                (1.0_f32, &mut previous_rail_left),
                                (-1.0_f32, &mut previous_rail_right),
                            ] {
                                let cx = *x as f32 + perp_x * rail_dist * sign;
                                let cz = *z as f32 + perp_z * rail_dist * sign;
                                let rail_cell = (cx.round() as i32, cz.round() as i32);
                                let cells_to_fill: Vec<(i32, i32)> = match *prev_state {
                                    Some(prev) => stair_fill_cells(prev, rail_cell),
                                    None => vec![rail_cell],
                                };
                                // Boardwalks and module decks bring their own railings.
                                let skip_side_railing = is_bridge_member
                                    && (!bridge_style.has_side_railing()
                                        || bridge_structure_moduled);
                                if !skip_side_railing {
                                    for (rx, rz) in cells_to_fill {
                                        if bridge_surface.contains(rx, rz) {
                                            continue;
                                        }
                                        let rail_block = if is_bridge_member {
                                            bridge_rail_block_choice
                                        } else {
                                            LIGHT_GRAY_CONCRETE
                                        };
                                        editor.set_block_absolute(
                                            rail_block,
                                            rx,
                                            by,
                                            rz,
                                            None,
                                            Some(ROAD_PROTECTED_SURFACES),
                                        );
                                        let rail_foundation = if is_bridge_member {
                                            bridge_style.rail_foundation_block()
                                        } else {
                                            STONE_BRICKS
                                        };
                                        if by > 0 {
                                            editor.set_block_absolute(
                                                rail_foundation,
                                                rx,
                                                by - 1,
                                                rz,
                                                None,
                                                None,
                                            );
                                        }
                                        let parapet = if is_bridge_member {
                                            bridge_style.parapet_block()
                                        } else {
                                            Some(BRICK_WALL)
                                        };
                                        if let Some(p) = parapet {
                                            editor.set_block_absolute(
                                                p,
                                                rx,
                                                by + 1,
                                                rz,
                                                None,
                                                None,
                                            );
                                        }
                                    }
                                }
                                *prev_state = Some(rail_cell);
                            }
                        }

                        // Draw inner-lane dividers as dashed white lines.
                        // For `lanes == 2` this reproduces the previous
                        // single-centerline stripe; higher `lanes` values
                        // produce `lanes - 1` evenly-spaced dividers across
                        // the road width. Each divider is offset
                        // perpendicular to the segment travel direction and
                        // rides at the same terrain-aware Y as the adjacent
                        // road cell (reuses `row_medians` so the per-cell
                        // flat cross-section is preserved).
                        if let Some((perp_x, perp_z, lane_width, half_width)) = lane_divider_geom {
                            if stripe_length < dash_length {
                                for l in 1..effective_lanes {
                                    // Signed perpendicular offset of this
                                    // divider from the centerline.
                                    //
                                    // Clean-mode twin-centre stripe: place
                                    // the two stripes ±0.5 block from centre
                                    // (1-block visual gap). Reads as
                                    // "divided highway" without occupying
                                    // multiple lane widths.
                                    let perp_dist = if twin_centre_stripe {
                                        if l == 1 {
                                            -0.5
                                        } else {
                                            0.5
                                        }
                                    } else {
                                        l as f32 * lane_width - half_width
                                    };
                                    let stripe_x = (*x as f32 + perp_x * perp_dist).round() as i32;
                                    let stripe_z = (*z as f32 + perp_z * perp_dist).round() as i32;

                                    // Y follows the perpendicular median
                                    // at this divider's axial position in
                                    // the cross-section (same rule as the
                                    // road cells). Clamp because the
                                    // rounded (stripe_x, stripe_z) could
                                    // land 1 cell outside the stamp on
                                    // diagonals.
                                    let stripe_y = if let Some(by) = bridge_y_here {
                                        by
                                    } else if flatten_width {
                                        let axial = if dir_horizontal {
                                            stripe_x - *x
                                        } else {
                                            stripe_z - *z
                                        };
                                        let idx = (axial + block_range).clamp(0, 2 * block_range)
                                            as usize;
                                        row_medians[idx] + offset
                                    } else {
                                        offset
                                    };

                                    // Whitelist on the actual road
                                    // surface so dividers appear on
                                    // non-default `surface=*` roads too
                                    // (hardcoding the default mix caused
                                    // markings to vanish on e.g.
                                    // concrete/asphalt-tagged highways).
                                    if use_absolute_y {
                                        editor.set_block_absolute(
                                            WHITE_CONCRETE,
                                            stripe_x,
                                            stripe_y,
                                            stripe_z,
                                            Some(block_types),
                                            None,
                                        );
                                    } else {
                                        editor.set_block(
                                            WHITE_CONCRETE,
                                            stripe_x,
                                            stripe_y,
                                            stripe_z,
                                            Some(block_types),
                                            None,
                                        );
                                    }
                                }
                            }

                            // Advance dash state once per centerline cell so
                            // the on/off pattern still reads as dashes, not
                            // solid lines (the original bug in early PR
                            // iterations).
                            stripe_length += 1;
                            if stripe_length >= dash_length + gap_length {
                                stripe_length = 0;
                            }
                        }
                    }

                    segment_index += 1;
                    cumulative_distance_from_start += segment_length - 1;
                }
                previous_node = Some((node.x, node.z));
            }

            if is_bridge_member {
                if let Some(module) = bridge_module {
                    crate::element_processing::bridge_modules::sweep_module(
                        editor,
                        &bridge_path,
                        module,
                    );
                } else if !bridge_structure_moduled {
                    decorate_bridge_above_deck(
                        editor,
                        bridge_style,
                        &bridge_path,
                        block_range,
                        bridge_start_is_boundary,
                        bridge_end_is_boundary,
                    );
                }
            }
        }
    }
}

/// Helper function to determine if a slope should be added at a specific node
fn should_add_slope_at_node(
    node: &crate::osm_parser::ProcessedNode,
    current_layer: i32,
    highway_connectivity: &HighwayConnectivityMap,
) -> bool {
    let node_coord = (node.x, node.z);

    // If we don't have connectivity information, always add slopes for non-zero layers
    if highway_connectivity.endpoints_is_empty() {
        return current_layer != 0;
    }

    // Check if there are other highways at different layers connected to this node
    if let Some(connected_layers) = highway_connectivity.endpoint_layers(&node_coord) {
        // Count how many ways are at the same layer as current way
        let same_layer_count = connected_layers
            .iter()
            .filter(|&&layer| layer == current_layer)
            .count();

        // If this is the only way at this layer connecting to this node, we need a slope
        // (unless we're at ground level and connecting to ground level ways)
        if same_layer_count <= 1 {
            return current_layer != 0;
        }

        // If there are multiple ways at the same layer, don't add slope
        false
    } else {
        // No other highways connected, add slope if not at ground level
        current_layer != 0
    }
}

/// Helper function to calculate the total length of a way in blocks
fn calculate_way_length(way: &ProcessedWay) -> usize {
    let mut total_length = 0;
    let mut previous_node: Option<&crate::osm_parser::ProcessedNode> = None;

    for node in &way.nodes {
        if let Some(prev) = previous_node {
            let dx = (node.x - prev.x).abs();
            let dz = (node.z - prev.z).abs();
            total_length += ((dx * dx + dz * dz) as f32).sqrt() as usize;
        }
        previous_node = Some(node);
    }

    total_length
}

/// Calculate the Y elevation for a specific point along the highway
#[allow(clippy::too_many_arguments)]
fn calculate_point_elevation(
    segment_index: usize,
    point_index: usize,
    segment_length: usize,
    total_segments: usize,
    base_elevation: i32,
    needs_start_slope: bool,
    needs_end_slope: bool,
    slope_length: usize,
) -> i32 {
    // If no slopes needed, return base elevation
    if !needs_start_slope && !needs_end_slope {
        return base_elevation;
    }

    // Calculate total distance from start
    let total_distance_from_start = segment_index * segment_length + point_index;
    let total_way_length = total_segments * segment_length;

    // Ensure we have reasonable values
    if total_way_length == 0 || slope_length == 0 {
        return base_elevation;
    }

    // Start slope calculation - gradual rise from ground level
    if needs_start_slope && total_distance_from_start <= slope_length {
        let slope_progress = total_distance_from_start as f32 / slope_length as f32;
        let elevation_offset = (base_elevation as f32 * slope_progress) as i32;
        return elevation_offset;
    }

    // End slope calculation - gradual descent to ground level
    if needs_end_slope
        && total_distance_from_start >= (total_way_length.saturating_sub(slope_length))
    {
        let distance_from_end = total_way_length - total_distance_from_start;
        let slope_progress = distance_from_end as f32 / slope_length as f32;
        let elevation_offset = (base_elevation as f32 * slope_progress) as i32;
        return elevation_offset;
    }

    // Middle section at full elevation
    base_elevation
}

/// Add support pillars for elevated highways
fn add_highway_support_pillar(
    editor: &mut WorldEditor,
    x: i32,
    highway_y: i32,
    z: i32,
    dx: i32,
    dz: i32,
    _block_range: i32, // Keep for future use
) {
    // Only add pillars at specific intervals and positions
    if dx == 0 && dz == 0 && (x + z) % 8 == 0 {
        // Add pillar from ground to highway level
        for y in 1..highway_y {
            editor.set_block(STONE_BRICKS, x, y, z, None, None);
        }

        // Add pillar base
        for base_dx in -1..=1 {
            for base_dz in -1..=1 {
                editor.set_block(STONE_BRICKS, x + base_dx, 0, z + base_dz, None, None);
            }
        }
    }
}

/// Add support pillars for bridges using absolute Y coordinates
/// Pillars extend from ground level up to the bridge deck
fn add_highway_support_pillar_absolute(
    editor: &mut WorldEditor,
    x: i32,
    bridge_deck_y: i32,
    z: i32,
    dx: i32,
    dz: i32,
    _block_range: i32, // Keep for future use
) {
    // Only add pillars at specific intervals and positions
    if dx == 0 && dz == 0 && (x + z) % 8 == 0 {
        // Get the actual ground level at this position
        let ground_y = editor.get_ground_level(x, z);

        // Add pillar from ground up to bridge deck
        // Only if the bridge is actually above the ground
        if bridge_deck_y > ground_y {
            for y in (ground_y + 1)..bridge_deck_y {
                editor.set_block_absolute(STONE_BRICKS, x, y, z, None, None);
            }

            // Add pillar base at ground level
            for base_dx in -1..=1 {
                for base_dz in -1..=1 {
                    editor.set_block_absolute(
                        STONE_BRICKS,
                        x + base_dx,
                        ground_y,
                        z + base_dz,
                        None,
                        None,
                    );
                }
            }
        }
    }
}

/// Generates a siding using stone brick slabs
pub fn generate_siding(
    editor: &mut WorldEditor,
    element: &ProcessedWay,
    bridge_surface: &BridgeSurfaceMap,
) {
    let mut previous_node: Option<XZPoint> = None;
    let siding_block: Block = STONE_BRICK_SLAB;

    for node in &element.nodes {
        let current_node = node.xz();

        if let Some(prev_node) = previous_node {
            let bresenham_points: Vec<(i32, i32, i32)> = bresenham_line(
                prev_node.x,
                0,
                prev_node.z,
                current_node.x,
                0,
                current_node.z,
            );

            for (bx, _, bz) in bresenham_points {
                if let Some(deck_y) = bridge_surface.deck_y_at(bx, bz) {
                    if !editor.check_for_block_absolute(
                        bx,
                        deck_y,
                        bz,
                        Some(ROAD_PROTECTED_SURFACES),
                        None,
                    ) {
                        editor.set_block_absolute(siding_block, bx, deck_y + 1, bz, None, None);
                    }
                } else if !editor.check_for_block(bx, 0, bz, Some(ROAD_PROTECTED_SURFACES)) {
                    editor.set_block(siding_block, bx, 1, bz, None, None);
                }
            }
        }

        previous_node = Some(current_node);
    }
}

/// A centerline point with its segment's unit travel direction (`ux`, `uz`) and cumulative
/// distance `s` (blocks) from the way start, used for dash phase.
struct AerowayCenterPoint {
    x: i32,
    z: i32,
    ux: f32,
    uz: f32,
    s: f32,
}

/// Runway centerline dash: stripe-on / stripe-off lengths in meters (scaled by `--scale`).
const RUNWAY_DASH_ON_M: f32 = 10.0;
const RUNWAY_DASH_OFF_M: f32 = 6.0;
/// How far inside the runway edge (blocks) the solid white edge stripes sit.
const RUNWAY_EDGE_INSET: f32 = 1.0;
/// Half-width (metres) used when an aeroway has no `width=*` tag (~24 m strip).
const AEROWAY_DEFAULT_HALF_M: f64 = 12.0;
/// Clamp (metres) for `width=*`-derived half-widths — guards against absurd tags.
const AEROWAY_MIN_HALF_M: f64 = 6.0;
const AEROWAY_MAX_HALF_M: f64 = 40.0;

/// True where a runway centerline dash is painted, given distance `s` (blocks) from the way start.
fn runway_centerline_dash_on(s: f32, scale: f64) -> bool {
    let on = (RUNWAY_DASH_ON_M * scale as f32).max(1.0);
    let off = (RUNWAY_DASH_OFF_M * scale as f32).max(1.0);
    (s % (on + off)) < on
}

/// Parses an OSM `width=*` value in metres (tolerates a trailing "m").
fn parse_aeroway_width_m(tags: &HashMap<String, String>) -> Option<f64> {
    let raw = tags.get("width")?;
    let s = raw.trim().trim_end_matches('m').trim();
    s.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)
}

/// Renders an aeroway as a concrete strip with markings: runways get asphalt-gray with a dashed
/// white centerline + white edge stripes, taxiways a lighter surface with a yellow centerline.
/// No threshold "piano keys" — OSM splits runways into segments, so a per-way renderer can't tell
/// a real end from an internal split.
/// Default helipad radius (metres) for node helipads without geometry.
pub(crate) const HELIPAD_NODE_RADIUS_M: f64 = 8.0;
/// Ring diameter as a fraction of the pad's equivalent-area radius.
const HELIPAD_RING_FRACTION: f64 = 0.85;

/// Helipad surface: light-gray pad, white ring + "H", sometimes a parked helicopter.
fn paint_helipad(
    editor: &mut WorldEditor,
    cells: &[(i32, i32)],
    cx: i32,
    cz: i32,
    building_footprints: &CoordinateBitmap,
) {
    if cells.is_empty() {
        return;
    }
    // Rooftop pad cells are skipped per-cell below (building_footprints.contains), which is
    // tile-invariant. The old aggregate `covered*2 > cells.len()` whole-pad skip was NOT: a pad
    // straddling a Meld cell boundary counts different coverage in each process, so one painted
    // and the other did not. Dropped for seam-safety; a mostly-rooftop pad simply paints only
    // its (few) non-building fringe cells.
    let r = ((cells.len() as f64) / std::f64::consts::PI).sqrt();
    let ring_r = (r * HELIPAD_RING_FRACTION).max(2.5);
    let bar_half_h = ((r * 0.45) as i32).clamp(2, 6);
    let bar_half_w = ((r * 0.30) as i32).clamp(1, 4);

    for &(x, z) in cells {
        if building_footprints.contains(x, z) {
            continue;
        }
        editor.set_block(LIGHT_GRAY_CONCRETE, x, 0, z, None, None);
    }

    let over_base = [LIGHT_GRAY_CONCRETE];
    for &(x, z) in cells {
        if building_footprints.contains(x, z) {
            continue;
        }
        let (dx, dz) = (x - cx, z - cz);
        let dist = ((dx * dx + dz * dz) as f64).sqrt();
        let on_ring = dist >= ring_r - 1.2 && dist < ring_r;
        let on_h = (dx.abs() == bar_half_w && dz.abs() <= bar_half_h)
            || (dz == 0 && dx.abs() <= bar_half_w);
        if on_ring || on_h {
            editor.set_block(WHITE_CONCRETE, x, 0, z, Some(&over_base), None);
        }
    }

    // The parked-helicopter prop is intentionally NOT placed here: it is a schematic centred on
    // the pad centroid, and a pad straddling a Meld cell boundary would read a cell-local ground
    // Y (get_absolute_y off the current cell's ground buffer) in each process, vertically shearing
    // or truncating the model at the seam. Seam-safe placement needs a region-ownership guard the
    // pad renderer does not have, so the bundled helicopter stays reserved (see structures/mod.rs).
}

/// Renders an `aeroway=helipad` way as a filled pad with markings.
fn generate_helipad_way(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    args: &Args,
    building_footprints: &CoordinateBitmap,
) {
    let outline: Vec<(i32, i32)> = way.nodes.iter().map(|n| (n.x, n.z)).collect();
    let cells = crate::floodfill::flood_fill_area(&outline, None);
    if cells.is_empty() {
        // Open or degenerate geometry: fall back to a disc at the first node.
        if let Some(n) = way.nodes.first() {
            paint_helipad_disc(editor, n.x, n.z, args, building_footprints);
        }
        return;
    }
    let (mut sx, mut sz) = (0i64, 0i64);
    for &(x, z) in &cells {
        sx += x as i64;
        sz += z as i64;
    }
    let cx = (sx / cells.len() as i64) as i32;
    let cz = (sz / cells.len() as i64) as i32;
    // Concave pads can put the mean outside the polygon; snap to the nearest cell.
    let (cx, cz) = if cells.contains(&(cx, cz)) {
        (cx, cz)
    } else {
        *cells
            .iter()
            .min_by_key(|&&(x, z)| {
                let (dx, dz) = ((x - cx) as i64, (z - cz) as i64);
                dx * dx + dz * dz
            })
            .unwrap()
    };
    paint_helipad(editor, &cells, cx, cz, building_footprints);
}

/// Renders an `aeroway=helipad` node as a default-size disc pad.
pub fn generate_helipad_node(
    editor: &mut WorldEditor,
    node: &crate::osm_parser::ProcessedNode,
    args: &Args,
    building_footprints: &CoordinateBitmap,
) {
    paint_helipad_disc(editor, node.x, node.z, args, building_footprints);
}

fn paint_helipad_disc(
    editor: &mut WorldEditor,
    cx: i32,
    cz: i32,
    args: &Args,
    building_footprints: &CoordinateBitmap,
) {
    let radius = ((HELIPAD_NODE_RADIUS_M * args.scale).round() as i32).max(4);
    let mut cells = Vec::new();
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            if dx * dx + dz * dz <= radius * radius {
                cells.push((cx + dx, cz + dz));
            }
        }
    }
    paint_helipad(editor, &cells, cx, cz, building_footprints);
}

pub fn generate_aeroway(
    editor: &mut WorldEditor,
    way: &ProcessedWay,
    args: &Args,
    building_footprints: &CoordinateBitmap,
) {
    let aeroway = way.tags.get("aeroway").map(String::as_str);
    let is_runway = aeroway == Some("runway");
    let is_taxiway = aeroway == Some("taxiway");

    // A closed helipad area is a small disc pad (ring + "H"), not a linear strip.
    if aeroway == Some("helipad") {
        generate_helipad_way(editor, way, args, building_footprints);
        return;
    }

    let base_block = if is_runway {
        GRAY_CONCRETE
    } else {
        LIGHT_GRAY_CONCRETE
    };

    // Half-width from the OSM `width=*` tag (metres, clamped to sane sizes); default when absent.
    let half_m = parse_aeroway_width_m(&way.tags)
        .map(|w| (w * 0.5).clamp(AEROWAY_MIN_HALF_M, AEROWAY_MAX_HALF_M))
        .unwrap_or(AEROWAY_DEFAULT_HALF_M);
    let half_width: i32 = (half_m * args.scale).round().max(1.0) as i32;

    // Build the centerline once: bresenham per segment, consecutive duplicates dropped, with a
    // running distance so dash phase and markings stay consistent across segments and regions.
    let mut points: Vec<AerowayCenterPoint> = Vec::new();
    let mut s_accum = 0.0_f32;
    let mut last: Option<(i32, i32)> = None;
    for pair in way.nodes.windows(2) {
        let (x1, z1) = (pair[0].x, pair[0].z);
        let (x2, z2) = (pair[1].x, pair[1].z);
        let len = (((x2 - x1) as f32).hypot((z2 - z1) as f32)).max(1e-6);
        let (ux, uz) = ((x2 - x1) as f32 / len, (z2 - z1) as f32 / len);
        for (x, _, z) in bresenham_line(x1, 0, z1, x2, 0, z2) {
            if last == Some((x, z)) {
                continue;
            }
            if let Some((lx, lz)) = last {
                s_accum += ((x - lx) as f32).hypot((z - lz) as f32);
            }
            points.push(AerowayCenterPoint {
                x,
                z,
                ux,
                uz,
                s: s_accum,
            });
            last = Some((x, z));
        }
    }

    // Pass 1: full surface, before markings. A runway's base may overwrite taxiway surface so it
    // wins crossings regardless of element order; a taxiway's base only fills empty cells (`None`),
    // so it never paints over a runway.
    let runway_overwrites = [LIGHT_GRAY_CONCRETE, YELLOW_CONCRETE];
    let base_over: Option<&[Block]> = is_runway.then_some(&runway_overwrites[..]);
    for cp in &points {
        for dx in -half_width..=half_width {
            for dz in -half_width..=half_width {
                editor.set_block(base_block, cp.x + dx, 0, cp.z + dz, base_over, None);
            }
        }
    }

    // Pass 2: markings. `set_block` only overwrites a whitelisted block, so markings must list
    // the base surface they replace — else pass 1 has claimed every cell and they're dropped.
    let base_overwrite = [base_block];
    let over_base = Some(&base_overwrite[..]);
    for cp in &points {
        // Perpendicular unit vector across the strip.
        let (px, pz) = (-cp.uz, cp.ux);
        if is_runway {
            if runway_centerline_dash_on(cp.s, args.scale) {
                editor.set_block(WHITE_CONCRETE, cp.x, 0, cp.z, over_base, None);
            }
            let off = (half_width as f32 - RUNWAY_EDGE_INSET).max(0.0);
            for sign in [1.0_f32, -1.0] {
                let ex = (cp.x as f32 + sign * px * off).round() as i32;
                let ez = (cp.z as f32 + sign * pz * off).round() as i32;
                editor.set_block(WHITE_CONCRETE, ex, 0, ez, over_base, None);
            }
        } else if is_taxiway {
            editor.set_block(YELLOW_CONCRETE, cp.x, 0, cp.z, over_base, None);
        }
    }
}

/// Returns the half-width (block_range) for a highway type.
///
/// This extracts the same logic used inside `generate_highways_internal` so
/// that pre-scan passes (e.g. building-passage collection) can determine road
/// width without generating any blocks.
pub(crate) fn highway_block_range(
    highway_type: &str,
    tags: &HashMap<String, String>,
    scale: f64,
) -> i32 {
    let mut block_range: i32 = match highway_type {
        "footway" | "pedestrian" => 1,
        "path" => 1,
        "motorway" | "primary" | "trunk" => 5,
        "secondary" => 4,
        "tertiary" => 2,
        "track" => 1,
        "service" => 2,
        "secondary_link" | "tertiary_link" => 1,
        "escape" => 1,
        "steps" => 1,
        _ => {
            if let Some(lanes) = tags.get("lanes") {
                if lanes == "2" {
                    3
                } else if lanes != "1" {
                    4
                } else {
                    2
                }
            } else {
                2
            }
        }
    };

    if scale < 1.0 {
        block_range = ((block_range as f64) * scale).floor() as i32;
    }

    block_range
}

/// Collect all (x, z) coordinates that are covered by any rendered road or path
/// surface. The returned bitmap has 1 for every block that the highway renderer
/// places as a road/path surface and 0 everywhere else.
///
/// Geometry is computed identically to `generate_highways_internal`:
/// - Bresenham line between each consecutive pair of OSM nodes
/// - Expanded by `block_range` in both axes (same value as the renderer uses)
/// - `area=yes` ways, indoor ways, negative-level ways, and pure node types
///   (street_lamp, crossing, bus_stop) are excluded, matching the renderer's
///   early-return guards.
///
/// This lets `get_nearest_road_block` in `amenities.rs` or other processors do a single O(1) bitmap lookup
/// instead of live `get_ground_level` + `check_for_block_absolute` world scans.
pub fn collect_road_surface_coords(
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    scale: f64,
    ground: &crate::ground::Ground,
) -> CoordinateBitmap {
    let mut bitmap = CoordinateBitmap::new(xzbbox);
    let (off_x, off_z) = (xzbbox.min_x(), xzbbox.min_z());

    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };

        let Some(highway_type) = way.tags.get("highway") else {
            continue;
        };

        // Exclude non-surface node-only highway types
        match highway_type.as_str() {
            "street_lamp" | "crossing" | "bus_stop" => continue,
            _ => {}
        }

        // Exclude area highways (pedestrian plazas etc.) — flood-filled separately
        if way.tags.get("area").is_some_and(|v| v == "yes") {
            continue;
        }

        // Exclude indoor ways (same guard as generate_highways_internal)
        if way.tags.get("indoor").is_some_and(|v| v == "yes") {
            continue;
        }

        // Exclude negative-level ways (indoor mapping)
        if way
            .tags
            .get("level")
            .and_then(|l| l.parse::<i32>().ok())
            .is_some_and(|l| l < 0)
        {
            continue;
        }

        // Tunnels render a covered shell, not a surface road, so exclude them.
        if renders_as_highway_tunnel(way) {
            continue;
        }

        // Use the same block_range the renderer uses for this highway type
        let block_range = highway_block_range(highway_type, &way.tags, scale);

        // Match the renderer's drown_over_water: an untagged crossing at small scale is skipped on
        // water cells, so keep those cells OUT of the road mask too — otherwise the water carve
        // (which avoids road_mask) would leave a dry air gap where the road was drowned.
        let untagged = way.tags.get("bridge").is_none_or(|b| b == "no");
        let drown_over_water = untagged && scale <= 0.5;

        for i in 1..way.nodes.len() {
            let prev = way.nodes[i - 1].xz();
            let cur = way.nodes[i].xz();

            let points = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);

            for (bx, _, bz) in &points {
                for dx in -block_range..=block_range {
                    for dz in -block_range..=block_range {
                        if drown_over_water
                            && ground.cover_class(XZPoint::new(bx + dx - off_x, bz + dz - off_z))
                                == crate::land_cover::LC_WATER
                        {
                            continue;
                        }
                        bitmap.set(bx + dx, bz + dz);
                    }
                }
            }
        }
    }

    bitmap
}

/// Collect all (x, z) coordinates covered by highways tagged
/// `tunnel=building_passage`.  The returned bitmap can be passed into building
/// generation to cut ground-level openings through walls and floors.
pub fn collect_building_passage_coords(
    elements: &[ProcessedElement],
    xzbbox: &XZBBox,
    scale: f64,
) -> CoordinateBitmap {
    // Quick scan: skip bitmap allocation entirely when there are no passage ways.
    let has_any = elements.iter().any(|e| {
        if let ProcessedElement::Way(w) = e {
            w.tags.get("tunnel").map(|v| v.as_str()) == Some("building_passage")
                && w.tags.contains_key("highway")
        } else {
            false
        }
    });
    if !has_any {
        return CoordinateBitmap::new_empty();
    }

    let mut bitmap = CoordinateBitmap::new(xzbbox);

    for element in elements {
        let ProcessedElement::Way(way) = element else {
            continue;
        };

        // Must be tunnel=building_passage
        if way.tags.get("tunnel").map(|v| v.as_str()) != Some("building_passage") {
            continue;
        }

        // Must have a highway tag so we know the road width
        let Some(highway_type) = way.tags.get("highway") else {
            continue;
        };

        let block_range = highway_block_range(highway_type, &way.tags, scale);

        for i in 1..way.nodes.len() {
            let prev = way.nodes[i - 1].xz();
            let cur = way.nodes[i].xz();

            let points = bresenham_line(prev.x, 0, prev.z, cur.x, 0, cur.z);

            for (bx, _, bz) in &points {
                for dx in -block_range..=block_range {
                    for dz in -block_range..=block_range {
                        bitmap.set(bx + dx, bz + dz);
                    }
                }
            }
        }
    }

    bitmap
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runway_dash_alternates_on_and_off() {
        // At scale 1: 10 blocks on, 6 off, repeating every 16.
        assert!(runway_centerline_dash_on(0.0, 1.0));
        assert!(runway_centerline_dash_on(9.0, 1.0));
        assert!(!runway_centerline_dash_on(10.0, 1.0));
        assert!(!runway_centerline_dash_on(15.0, 1.0));
        assert!(
            runway_centerline_dash_on(16.0, 1.0),
            "pattern repeats at 16"
        );
        assert!(
            runway_centerline_dash_on(160.0, 1.0),
            "phase stays consistent far along"
        );
    }

    #[test]
    fn runway_dash_scales_with_world_scale() {
        // At scale 2: 20 blocks on, 12 off.
        assert!(runway_centerline_dash_on(19.0, 2.0));
        assert!(!runway_centerline_dash_on(20.0, 2.0));
    }

    // --- Rendering regression tests: markings must actually overwrite the base surface. ---

    use crate::coordinate_system::cartesian::XZBBox;
    use crate::coordinate_system::geographic::LLBBox;
    use crate::osm_parser::ProcessedNode;
    use crate::world_editor::WorldEditor;
    use clap::Parser as _;
    use std::collections::HashMap as StdMap;
    use std::path::PathBuf;

    /// Builds an in-memory editor (never saved) over a 400×100 area at ground Y=0.
    fn test_editor(xzbbox: &XZBBox) -> WorldEditor<'_> {
        let llbbox = LLBBox::new(54.6, 9.9, 54.61, 9.91).unwrap();
        WorldEditor::new(PathBuf::from("/dev/null/unused"), xzbbox, llbbox)
    }

    fn straight_aeroway(kind: &str) -> ProcessedWay {
        let mut tags = StdMap::new();
        tags.insert("aeroway".to_string(), kind.to_string());
        ProcessedWay {
            id: 1,
            nodes: vec![
                ProcessedNode {
                    id: 1,
                    tags: StdMap::new(),
                    x: 10,
                    z: 50,
                },
                ProcessedNode {
                    id: 2,
                    tags: StdMap::new(),
                    x: 390,
                    z: 50,
                },
            ],
            tags,
            unclipped_bounds: None,
            unclipped_polygon_area: None,
        }
    }

    #[test]
    fn runway_paints_white_centerline_and_edges_over_gray() {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let xzbbox = XZBBox::rect_from_xz_lengths(400.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);

        generate_aeroway(
            &mut editor,
            &straight_aeroway("runway"),
            &args,
            &CoordinateBitmap::new_empty(),
        );

        // Centerline at the way start (s=0, dash on) is white; a dash-gap cell stays gray.
        assert!(
            editor.check_for_block(10, 0, 50, Some(&[WHITE_CONCRETE])),
            "centerline dash"
        );
        assert!(
            editor.check_for_block(22, 0, 50, Some(&[GRAY_CONCRETE])),
            "dash gap stays asphalt"
        );
        // Solid white edge stripe one block inside the 12-wide half (z = 50 ± 11).
        assert!(
            editor.check_for_block(10, 0, 39, Some(&[WHITE_CONCRETE])),
            "left edge stripe"
        );
        assert!(
            editor.check_for_block(10, 0, 61, Some(&[WHITE_CONCRETE])),
            "right edge stripe"
        );
        // Plain surface between centerline and edge is asphalt gray.
        assert!(
            editor.check_for_block(10, 0, 45, Some(&[GRAY_CONCRETE])),
            "asphalt base"
        );
    }

    #[test]
    fn taxiway_paints_yellow_centerline_over_light_gray() {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let xzbbox = XZBBox::rect_from_xz_lengths(400.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);

        generate_aeroway(
            &mut editor,
            &straight_aeroway("taxiway"),
            &args,
            &CoordinateBitmap::new_empty(),
        );

        assert!(
            editor.check_for_block(10, 0, 50, Some(&[YELLOW_CONCRETE])),
            "yellow centerline"
        );
        assert!(
            editor.check_for_block(10, 0, 45, Some(&[LIGHT_GRAY_CONCRETE])),
            "light-gray base"
        );
        // Taxiways get no white edge stripes.
        assert!(
            !editor.check_for_block(10, 0, 39, Some(&[WHITE_CONCRETE])),
            "no edge stripe"
        );
    }

    #[test]
    fn runway_width_tag_widens_the_strip() {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let xzbbox = XZBBox::rect_from_xz_lengths(400.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);

        let mut way = straight_aeroway("runway");
        way.tags.insert("width".to_string(), "60".to_string());
        generate_aeroway(&mut editor, &way, &args, &CoordinateBitmap::new_empty());

        // 60 m wide ⇒ half-width 30: asphalt reaches z=70 and the edge stripe sits at z=50+29.
        assert!(
            editor.check_for_block(10, 0, 70, Some(&[GRAY_CONCRETE])),
            "widened asphalt"
        );
        assert!(
            editor.check_for_block(10, 0, 79, Some(&[WHITE_CONCRETE])),
            "edge stripe at width/2-1"
        );
    }

    #[test]
    fn runway_overrides_a_crossing_taxiway_regardless_of_order() {
        let args = Args::parse_from(["arnis", "--bbox", "1,2,3,4"].iter());
        let xzbbox = XZBBox::rect_from_xz_lengths(400.0, 100.0).unwrap();
        let mut editor = test_editor(&xzbbox);

        // Process the taxiway FIRST — the order that used to leak taxiway surface onto the runway.
        let mut tags = StdMap::new();
        tags.insert("aeroway".to_string(), "taxiway".to_string());
        let taxiway = ProcessedWay {
            id: 2,
            nodes: vec![
                ProcessedNode {
                    id: 3,
                    tags: StdMap::new(),
                    x: 200,
                    z: 10,
                },
                ProcessedNode {
                    id: 4,
                    tags: StdMap::new(),
                    x: 200,
                    z: 90,
                },
            ],
            tags,
            unclipped_bounds: None,
            unclipped_polygon_area: None,
        };
        generate_aeroway(&mut editor, &taxiway, &args, &CoordinateBitmap::new_empty());
        generate_aeroway(
            &mut editor,
            &straight_aeroway("runway"),
            &args,
            &CoordinateBitmap::new_empty(),
        );

        // The crossing cell belongs to the runway, not the taxiway.
        assert!(
            editor.check_for_block(200, 0, 50, Some(&[GRAY_CONCRETE])),
            "runway wins crossing"
        );
        assert!(!editor.check_for_block(200, 0, 50, Some(&[YELLOW_CONCRETE])));
        assert!(!editor.check_for_block(200, 0, 50, Some(&[LIGHT_GRAY_CONCRETE])));
        // Away from the runway the taxiway is untouched.
        assert!(
            editor.check_for_block(200, 0, 20, Some(&[YELLOW_CONCRETE])),
            "taxiway intact off-runway"
        );
    }

    // ---- G5: connectivity struct keeps endpoint semantics, adds junctions ----

    fn highway_way(id: u64, coords: &[(i32, i32)]) -> ProcessedElement {
        let mut tags = StdMap::new();
        tags.insert("highway".to_string(), "residential".to_string());
        ProcessedElement::Way(ProcessedWay {
            id,
            nodes: coords
                .iter()
                .enumerate()
                .map(|(i, &(x, z))| ProcessedNode {
                    id: id * 1000 + i as u64,
                    tags: StdMap::new(),
                    x,
                    z,
                })
                .collect(),
            tags,
            unclipped_bounds: None,
            unclipped_polygon_area: None,
        })
    }

    #[test]
    fn connectivity_endpoints_unchanged_and_midway_junctions_detected() {
        // A: (0,0)-(50,0)-(100,0). B: T-junction ending on A's MID node.
        // C: shares only A's endpoint (100,0).
        let elements = vec![
            highway_way(1, &[(0, 0), (50, 0), (100, 0)]),
            highway_way(2, &[(50, 0), (50, 60)]),
            highway_way(3, &[(100, 0), (160, 0)]),
        ];
        let map = build_highway_connectivity_map(&elements);

        // Endpoint view must NOT contain A's mid node: only B's endpoint
        // registers at (50,0). Anything else changes legacy slope decisions.
        assert_eq!(map.endpoint_layers(&(50, 0)).map_or(0, |v| v.len()), 1);
        assert_eq!(map.endpoint_layers(&(0, 0)).map_or(0, |v| v.len()), 1);
        assert_eq!(map.endpoint_layers(&(100, 0)).map_or(0, |v| v.len()), 2);
        assert!(map.endpoint_layers(&(50, 60)).is_some());

        // Junction view: mid-way T-junction pins now exist.
        assert!(map.is_junction((50, 0)), "A-mid + B-end = junction");
        assert!(map.is_junction((100, 0)), "shared endpoint = junction");
        assert!(!map.is_junction((0, 0)), "dead end is not a junction");
        assert!(!map.is_junction((160, 0)), "dead end is not a junction");
        assert!(
            !map.is_junction((75, 0)),
            "a bresenham point that is not a node never pins"
        );
    }

    #[test]
    fn connectivity_closed_way_closure_is_a_junction() {
        let elements = vec![highway_way(9, &[(0, 0), (30, 0), (30, 30), (0, 0)])];
        let map = build_highway_connectivity_map(&elements);
        assert!(map.is_junction((0, 0)), "closure coordinate occurs twice");
        assert!(!map.is_junction((30, 0)));
    }

    // ---- G3: station list is byte-exact against the placement loop ----

    fn nodes_at(coords: &[(i32, i32)]) -> Vec<ProcessedNode> {
        coords
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| ProcessedNode {
                id: i as u64,
                tags: StdMap::new(),
                x,
                z,
            })
            .collect()
    }

    /// Independent re-derivation of the placement loop's `tds` indexing,
    /// transcribed from `generate_highways_internal`: `skip_first == 0` on
    /// every non-bridge way, `tds = cumulative_distance_from_start +
    /// point_index`, `cumulative_distance_from_start += segment_length - 1`
    /// after each segment, later writes at a repeated `tds` winning. Returns
    /// the map plus the way's `total_bresenham_length` computed by the
    /// separate formula the real code uses for it.
    fn placement_tds_map(nodes: &[ProcessedNode]) -> (BTreeMap<usize, (i32, i32)>, usize) {
        let mut seen: BTreeMap<usize, (i32, i32)> = BTreeMap::new();
        let mut cumulative_distance_from_start: usize = 0;
        let mut previous_node: Option<(i32, i32)> = None;
        for node in nodes {
            if let Some((x1, z1)) = previous_node {
                let points = bresenham_line(x1, 0, z1, node.x, 0, node.z);
                let segment_length = points.len();
                for (point_index, (x, _, z)) in points.iter().enumerate() {
                    seen.insert(cumulative_distance_from_start + point_index, (*x, *z));
                }
                cumulative_distance_from_start += segment_length - 1;
            }
            previous_node = Some((node.x, node.z));
        }
        let total = nodes
            .windows(2)
            .map(|p| {
                let dx = (p[1].x - p[0].x).unsigned_abs() as usize;
                let dz = (p[1].z - p[0].z).unsigned_abs() as usize;
                dx.max(dz)
            })
            .sum::<usize>()
            + 1;
        (seen, total)
    }

    #[test]
    fn grade_stations_reproduce_placement_loop_tds() {
        // Axis-aligned, pure diagonal, a bend past 45 degrees (the axis-flip
        // case), a zero-length segment, a closed loop, and a way long enough
        // to span several anchor windows.
        let shapes: Vec<Vec<(i32, i32)>> = vec![
            vec![(0, 0), (40, 0)],
            vec![(0, 0), (40, 40)],
            vec![(10, 10), (60, 14), (64, 70), (5, 90)],
            vec![(0, 0), (25, 3), (25, 3), (25, 60)],
            vec![(0, 0), (30, 0), (30, 30), (0, 30), (0, 0)],
            vec![(-120, -40), (-3, 17), (200, 9), (205, 400)],
            vec![(7, 7), (7, 7)],
        ];
        for shape in shapes {
            let nodes = nodes_at(&shape);
            let (expected, total) = placement_tds_map(&nodes);
            let stations = road_grade_stations(&nodes);

            assert_eq!(
                stations.len(),
                total,
                "station_count must equal total_bresenham_length for {shape:?}"
            );
            assert_eq!(
                expected.len(),
                total,
                "placement tds range must be dense 0..total for {shape:?}"
            );
            for (i, station) in stations.iter().enumerate() {
                assert_eq!(
                    expected[&i],
                    (station.x, station.z),
                    "station {i} diverges from placement tds for {shape:?}"
                );
            }
        }
    }

    #[test]
    fn grade_stations_overwrite_shared_endpoints() {
        // Two segments meeting at (20, 0). The placement loop emits that node
        // twice at the SAME tds (skip_first == 0 and the `- 1` accumulation),
        // so the builder must overwrite rather than append.
        let nodes = nodes_at(&[(0, 0), (20, 0), (20, 15)]);
        let stations = road_grade_stations(&nodes);
        assert_eq!(stations.len(), 20 + 15 + 1, "no double-counted endpoint");
        assert_eq!((stations[20].x, stations[20].z), (20, 0));
        // The later segment's sampling axis wins at the shared station.
        assert!(!stations[20].dir_horizontal, "second segment is z-dominant");
        assert!(stations[19].dir_horizontal, "first segment is x-dominant");

        // A zero-length segment advances tds by nothing and simply rewrites
        // the station in place.
        let degenerate = nodes_at(&[(0, 0), (5, 0), (5, 0), (9, 0)]);
        assert_eq!(road_grade_stations(&degenerate).len(), 10);
    }

    #[test]
    fn grade_node_stations_land_on_their_own_station() {
        let nodes = nodes_at(&[(10, 10), (60, 14), (64, 70), (5, 90)]);
        let stations = road_grade_stations(&nodes);
        let node_tds = road_grade_node_stations(&nodes);
        assert_eq!(node_tds.len(), nodes.len());
        assert_eq!(node_tds[0], 0);
        assert_eq!(*node_tds.last().unwrap(), stations.len() - 1);
        for (node, &tds) in nodes.iter().zip(node_tds.iter()) {
            assert_eq!(
                (stations[tds].x, stations[tds].z),
                (node.x, node.z),
                "junction pin at tds {tds} must sit on its own node"
            );
        }
    }

    // ---- G9: profile properties ----

    const G: f64 = 1.0 / 6.0;
    const EPS: f64 = 1e-9;

    /// Gently rising terrain with sub-block ripple: deterministic, and shallow
    /// enough that no anchor pair is infeasible, so `g` is the cap everywhere.
    fn gentle_base(n: usize) -> Vec<f64> {
        (0..n)
            .map(|i| 60.0 + i as f64 * 0.05 + ((i * 7 % 13) as f64) * 0.11)
            .collect()
    }

    fn max_abs_delta(p: &[f64]) -> f64 {
        p.windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f64::max)
    }

    #[test]
    fn grade_bound_holds_everywhere() {
        let base = gentle_base(200);
        let profile = grade_profile(&base, &BTreeMap::new(), G);
        assert_eq!(profile.len(), base.len());
        assert!(
            max_abs_delta(&profile) <= G + EPS,
            "profile exceeded the grade cap: {}",
            max_abs_delta(&profile)
        );
        // The raw terrain it was built from does exceed it — otherwise this
        // test would pass on a no-op.
        assert!(max_abs_delta(&base) > G);
    }

    #[test]
    fn grade_holds_pins_and_anchors_exactly() {
        // Flat terrain with one mid-way junction pin 2 blocks above it: the
        // case a forward-first clamp destroys (it biases toward the way's
        // start and walks the pin away).
        let base = vec![64.0; 129];
        let mut pins = BTreeMap::new();
        pins.insert(30, 66.0);
        let profile = grade_profile(&base, &pins, G);

        assert_eq!(profile[30], 66.0, "mid-way pin must be exact");
        for anchor in [0usize, 64, 128] {
            assert_eq!(profile[anchor], 64.0, "anchor {anchor} must be exact");
        }
        assert!(max_abs_delta(&profile) <= G + EPS);
        // Beyond the anchor that bounds the pin's window, the profile is the
        // terrain again — pin influence is window-local, not way-global.
        assert_eq!(profile[100], 64.0);
    }

    #[test]
    fn grade_relaxes_infeasible_pin_pair() {
        // Two pins 10 blocks apart demanding 10 blocks of climb: 1.0 per block
        // against a cap of 1/6. The interval between them relaxes to its own
        // linear grade; every other interval keeps `g`.
        let base = vec![60.0; 41];
        let mut pins = BTreeMap::new();
        pins.insert(10, 60.0);
        pins.insert(20, 70.0);
        let profile = grade_profile(&base, &pins, G);

        assert_eq!(profile[0], 60.0, "anchor still exact");
        assert_eq!(profile[10], 60.0);
        assert_eq!(profile[20], 70.0);
        for i in 10..20 {
            assert!(
                (profile[i + 1] - profile[i]).abs() <= 1.0 + EPS,
                "relaxed interval must not exceed its own pin-to-pin grade"
            );
        }
        for i in (0..10).chain(20..40) {
            assert!(
                (profile[i + 1] - profile[i]).abs() <= G + EPS,
                "relaxation must stay inside the infeasible interval (edge {i})"
            );
        }
    }

    #[test]
    fn grade_solver_is_direction_invariant() {
        // Way direction is arbitrary in OSM, so the solver must be a mirror of
        // itself. Asserted bit-exactly, not within epsilon: the two sweeps
        // accumulate the same edge costs in the same order under reversal.
        let base = gentle_base(101);
        let constraints = vec![(0usize, base[0]), (37, 62.5), (100, base[100])];
        let forward = grade_solve_window(&base, &constraints, G);

        let reversed_base: Vec<f64> = base.iter().rev().copied().collect();
        let reversed_constraints: Vec<(usize, f64)> = constraints
            .iter()
            .rev()
            .map(|&(i, c)| (100 - i, c))
            .collect();
        let reversed = grade_solve_window(&reversed_base, &reversed_constraints, G);

        for i in 0..101 {
            assert_eq!(forward[i], reversed[100 - i], "mirror mismatch at {i}");
        }
    }

    #[test]
    fn grade_profile_is_direction_invariant_when_anchors_mirror() {
        // 129 stations puts anchors at {0, 64, 128}, a set that is symmetric
        // under reversal. Anchors are measured from the way's FIRST node by
        // design (that is what makes them identical in every tile and cell),
        // so at other lengths reversal relocates them; the solver symmetry
        // asserted above is the direction-free part.
        let base = gentle_base(129);
        let mut pins = BTreeMap::new();
        pins.insert(40, 63.25);
        let forward = grade_profile(&base, &pins, G);

        let reversed_base: Vec<f64> = base.iter().rev().copied().collect();
        let mut reversed_pins = BTreeMap::new();
        reversed_pins.insert(128 - 40, 63.25);
        let reversed = grade_profile(&reversed_base, &reversed_pins, G);

        for i in 0..129 {
            assert_eq!(forward[i], reversed[128 - i], "mirror mismatch at {i}");
        }
    }

    #[test]
    fn grade_window_locality_bounds_dem_influence() {
        // The tile-invariance property: a terrain sample cannot reach past the
        // anchors that bracket it, so an edge-clamped DEM read near a Meld
        // cell border cannot propagate into the cell.
        let mut base = gentle_base(200);
        let before = grade_profile(&base, &BTreeMap::new(), G);

        base[150] += 5.0; // inside window [128, 192], far from every other one
        let after = grade_profile(&base, &BTreeMap::new(), G);

        assert_ne!(before[150], after[150], "the perturbed window must move");
        for i in 0..=128 {
            assert_eq!(before[i], after[i], "window before the anchor moved at {i}");
        }
        for i in 192..200 {
            assert_eq!(before[i], after[i], "window after the anchor moved at {i}");
        }
    }

    #[test]
    fn grade_class_table_and_scale() {
        assert_eq!(road_grade_denominator("motorway"), Some(12));
        assert_eq!(road_grade_denominator("motorway_link"), Some(12));
        assert_eq!(road_grade_denominator("tertiary"), Some(8));
        assert_eq!(road_grade_denominator("service"), Some(6));
        assert_eq!(road_grade_denominator("path"), Some(4));
        assert_eq!(
            road_grade_denominator("living_street"),
            Some(6),
            "unlisted classes take the residential tier, never ungraded"
        );
        assert_eq!(
            road_grade_denominator("steps"),
            None,
            "stairs are supposed to step"
        );

        assert!(road_grade_step("steps", 1.0).is_none());
        assert_eq!(road_grade_step("residential", 1.0), Some(1.0 / 6.0));
        assert_eq!(road_grade_step("motorway", 0.5), Some(1.0 / 6.0));
        assert_eq!(
            road_grade_step("footway", 0.3),
            Some(0.5),
            "N_eff floors at 2 so tiny scales do not step every block"
        );
    }
}
