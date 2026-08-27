//! River-only U-shaped bed (`--river-bed v1`).
//!
//! A per-render field that OVERRIDES the carve depth of columns inside a river mask with a
//! smoothstep "rounded parabola" profile, and blends back to the legacy depth over a band
//! wherever the river meets water it does not own.
//!
//! Three rules govern every line in this file:
//!
//! 1. **Flag off = byte-identical.** `RiverBedMode::Off` builds an empty field whose
//!    `depth_override` is `None` everywhere, so every caller falls back to
//!    `BigWaterField::depth_at` and nothing downstream can observe the feature.
//! 2. **Rivers only.** Lakes, oceans and ESA-only water keep the legacy bed - sqrt terraces,
//!    shoal ring, bank wobble and dunes - byte for byte, even with the flag on. A river
//!    centerline drawn through a mapped lake or past the coast is removed from the mask by
//!    explicit CLIPPING (subtract non-river water polygons) and embedding SUPPRESSION (a
//!    centerline sitting deep inside wide land-cover water with no river polygon behind it),
//!    never assumed away from tags.
//! 3. **Depth-only, never carve-creation.** The mask is intersected at APPLY time with "would
//!    carve today" (an `LC_WATER` column in the land-cover pass, or a column inside a water
//!    polygon's fill span). Line waterways still draw nothing of their own here: the
//!    historically failed line-waterway carve (`waterways.rs:1-8` - grooves, cut trees,
//!    floaters) stays dead.
//!
//! Determinism: no RNG and no wall-clock anywhere in this path. The legacy bank wobble and the
//! dune pass are REMOVED for river columns rather than re-seeded, so the profile is a pure
//! function of element geometry, absolute (master-anchored) block coordinates and `scale`.

use crate::bresenham::bresenham_line;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::element_processing::water_areas::{
    collect_all_ring_edges, collect_ring_edges, compute_scanline_spans, subtract_spans,
    union_spans, ScanlineEdge,
};
use crate::element_processing::waterways::{is_channel_waterway, is_subgrade, waterway_width};
use crate::ground::Ground;
use crate::land_cover::LC_WATER;
use crate::osm_parser::{ProcessedElement, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use crate::water_depth::{
    chamfer_3_4_dt, BigWaterField, MAX_WATER_DEPTH, SMALL_SCALE_MAX_DEPTH, SMALL_SCALE_THRESHOLD,
};

/// `waterway=*` values that carry a river profile. `ditch`/`drain` are deliberately absent:
/// a 2-3 block channel is below the useful resolution of the mask and today's flat ribbon is
/// fine for them.
const RIVER_WATERWAYS: &[&str] = &["river", "stream", "canal", "brook", "fairway", "flowline"];

/// `water=*` VALUES that make a polygon a river. `oxbow` is a lake and stays out.
const RIVER_WATER_VALUES: &[&str] = &["river", "canal", "stream"];

/// Halo, in blocks, added around the render bbox before rasterizing, so windowed maxima and
/// the confluence band see the same neighbourhood in adjacent Meld cells.
/// `64` = the widest scaled half-width a `width=*` tag can buy (`MAX_WATERWAY_WIDTH / 2`),
/// `32` = the widest confluence band.
const HALO_CAP: i32 = 96;

/// Widest confluence blend band, in blocks.
const BAND_MAX: f64 = 32.0;

/// Narrowest confluence blend band, in blocks.
const BAND_MIN: f64 = 8.0;

/// Slack over the local half-width before a centerline-only column counts as EMBEDDED in a
/// wider water body (and is therefore not evidence of a river of its own).
const EMBED_MARGIN: f64 = 4.0;

/// Chamfer-DT saturation. The transform is u8, so distance saturates at 255 units = 85 blocks:
/// that is the hard ceiling on `d_blocks`, and hence on the windowed-max polygon half-width.
/// Harmless - the depth cap saturates at `hw >= 30` - but it is a real ceiling, not an
/// asymptote, and any future retune of the `D` table has to stay under it.
const DT_MAX: u8 = u8::MAX;

/// Chamfer units per block (the 3-4 chamfer's straight step).
const DT_UNITS_PER_BLOCK: f64 = 3.0;

/// Upper bound on lattice cells. Past it the field is dropped and every column falls back to
/// the legacy bed, which is always a valid render.
const MAX_LATTICE_CELLS: usize = 64_000_000;

/// `--river-bed` value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RiverBedMode {
    /// Legacy bed everywhere. Byte-identical to a build without this feature.
    #[default]
    Off,
    /// U-shaped river bed, version 1. The version is part of the contract: any retune of the
    /// `q`/`D`/`B` tables ships as `v2`, never as a changed `v1`.
    V1,
}

impl RiverBedMode {
    /// Parse `--river-bed`. Anything unrecognised is `Off`; `args.rs` already restricts the
    /// accepted values, so this is the belt to that braces.
    pub fn from_arg(spec: &str) -> Self {
        match spec.trim() {
            "v1" => Self::V1,
            _ => Self::Off,
        }
    }

    pub fn enabled(self) -> bool {
        matches!(self, Self::V1)
    }
}

/// Deepest a river column can carve at this scale. Mirrors the legacy caps so the world datum
/// reservation and the `carve_water_column_with_flags` debug-assert keep holding.
pub fn river_depth_cap_blocks(scale: f64) -> i32 {
    if scale < SMALL_SCALE_THRESHOLD {
        SMALL_SCALE_MAX_DEPTH
    } else {
        MAX_WATER_DEPTH
    }
}

// ---------------------------------------------------------------------------------------------
// The field
// ---------------------------------------------------------------------------------------------

/// Baked per-block river bed depth over a halo-expanded lattice.
pub struct RiverBedField {
    min_x: i32,
    min_z: i32,
    w: usize,
    h: usize,
    /// Final (blended, rounded) override depth per lattice cell.
    depth: Vec<u8>,
    /// Set where an override exists. Empty field => never set => legacy everywhere.
    mask: BitGrid,
}

impl RiverBedField {
    pub fn empty() -> Self {
        Self {
            min_x: 0,
            min_z: 0,
            w: 0,
            h: 0,
            depth: Vec::new(),
            mask: BitGrid::new(0),
        }
    }

    #[inline]
    fn idx(&self, x: i32, z: i32) -> Option<usize> {
        if self.w == 0 {
            return None;
        }
        let lx = i64::from(x) - i64::from(self.min_x);
        let lz = i64::from(z) - i64::from(self.min_z);
        if lx < 0 || lz < 0 || lx as usize >= self.w || lz as usize >= self.h {
            return None;
        }
        Some(lz as usize * self.w + lx as usize)
    }

    /// The river bed depth at this column, or `None` where the legacy bed owns it.
    ///
    /// Callers must only consult this for columns that ALREADY carve (rule 3): the field
    /// carries no would-carve information of its own.
    #[inline]
    pub fn depth_override(&self, x: i32, z: i32) -> Option<i32> {
        let i = self.idx(x, z)?;
        if self.mask.get(i) {
            Some(i32::from(self.depth[i]))
        } else {
            None
        }
    }

    /// Deepest override in the field (0 when empty). Used by the clearance tests.
    #[cfg(test)]
    pub fn max_override_depth(&self) -> i32 {
        let mut m = 0u8;
        for (i, &d) in self.depth.iter().enumerate() {
            if self.mask.get(i) && d > m {
                m = d;
            }
        }
        i32::from(m)
    }

    /// Count of columns carrying an override. Test/diagnostic use.
    #[cfg(test)]
    pub fn override_count(&self) -> usize {
        (0..self.w * self.h).filter(|&i| self.mask.get(i)).count()
    }
}

// ---------------------------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------------------------

/// Build the field once per render, before tiling.
///
/// `bwf` is already baked at this point and supplies the blend target: inside a confluence band
/// the river depth lerps to the legacy depth, so the value at the mask boundary equals the
/// legacy one exactly and the lake bed on the far side is never written differently at all.
pub fn compute_river_bed_field(
    elements: &[ProcessedElement],
    ground: &Ground,
    bwf: &BigWaterField,
    xzbbox: &XZBBox,
    scale: f64,
    mode: RiverBedMode,
) -> RiverBedField {
    if !mode.enabled() {
        return RiverBedField::empty();
    }
    let off_x = xzbbox.min_x();
    let off_z = xzbbox.min_z();
    // `cover_class` clamps reads to the render bbox, so land cover sampled in the halo is the
    // border row repeated. See the R6 note: that bounds the embedding suppression and the
    // LC-seeded confluence band near a cell seam; it moves nothing in the cell interior.
    let is_lc_water =
        |x: i32, z: i32| ground.cover_class(XZPoint::new(x - off_x, z - off_z)) == LC_WATER;
    let legacy_depth = |x: i32, z: i32| bwf.depth_at(x, z);
    let inputs = FieldInputs {
        is_lc_water: &is_lc_water,
        legacy_depth: &legacy_depth,
        bb: (
            xzbbox.min_x(),
            xzbbox.max_x(),
            xzbbox.min_z(),
            xzbbox.max_z(),
        ),
        scale,
    };
    build_field(elements, &inputs)
}

/// Everything the build needs that is not element geometry. Behind closures so the fixtures can
/// drive the whole pipeline without a `Ground` or a baked `BigWaterField`.
struct FieldInputs<'a> {
    is_lc_water: &'a dyn Fn(i32, i32) -> bool,
    legacy_depth: &'a dyn Fn(i32, i32) -> i32,
    /// Render bbox: (min_x, max_x, min_z, max_z).
    bb: (i32, i32, i32, i32),
    scale: f64,
}

/// A classified water polygon, already reduced to rings.
struct PolyRings {
    outers: Vec<Vec<XZPoint>>,
    inners: Vec<Vec<XZPoint>>,
}

/// A classified line river: its centerline and the half-width its tags buy.
struct LineRiver<'a> {
    way: &'a ProcessedWay,
    /// Footprint half-width in blocks (the ribbon reaches `hw + 1` in Chebyshev, as the draw does).
    hw_stamp: i32,
    /// Profile half-width, floored at 1 so `t = d/hw` can never divide by zero
    /// (`scaled_half_width` returns 0 below scale 0.3 and <= 1 below 0.7).
    hw_profile: u8,
}

fn build_field(elements: &[ProcessedElement], inp: &FieldInputs) -> RiverBedField {
    let scale = inp.scale;
    let (bb_min_x, bb_max_x, bb_min_z, bb_max_z) = inp.bb;

    // ---- 1. Classification -------------------------------------------------------------
    let mut lines: Vec<LineRiver> = Vec::new();
    let mut river_polys: Vec<PolyRings> = Vec::new();
    let mut nonriver_polys: Vec<PolyRings> = Vec::new();
    classify(
        elements,
        scale,
        &mut lines,
        &mut river_polys,
        &mut nonriver_polys,
    );
    if lines.is_empty() && river_polys.is_empty() {
        return RiverBedField::empty();
    }

    // ---- 2. Lattice --------------------------------------------------------------------
    let halo = ((64.0 * scale).ceil() as i32 + 32).clamp(1, HALO_CAP);
    let Some(lat) = lattice_for(
        &lines,
        &river_polys,
        halo,
        (bb_min_x, bb_max_x, bb_min_z, bb_max_z),
    ) else {
        return RiverBedField::empty();
    };
    let n = lat.w * lat.h;

    // ---- 3. Rasterize the three masks ---------------------------------------------------
    let mut line_mask = BitGrid::new(n);
    let mut hw_line = vec![0u8; n];
    for lr in &lines {
        stamp_line(lr, &lat, &mut line_mask, &mut hw_line);
    }
    let mut poly_mask = BitGrid::new(n);
    for p in &river_polys {
        rasterize_poly(p, &lat, &mut poly_mask);
    }
    let mut nonriver_mask = BitGrid::new(n);
    for p in &nonriver_polys {
        rasterize_poly(p, &lat, &mut nonriver_mask);
    }

    // Land-cover water over the lattice, sampled once and reused by suppression and by the
    // confluence seeds.
    let mut lc_water = BitGrid::new(n);
    for j in 0..lat.h {
        let z = lat.min_z + j as i32;
        for i in 0..lat.w {
            if (inp.is_lc_water)(lat.min_x + i as i32, z) {
                lc_water.set(j * lat.w + i);
            }
        }
    }

    // ---- 4. Clip + embedding suppression, BEFORE any distance transform -----------------
    // Both removals have to land before the bed DT so the mask, `d`, `hw` and the blend all
    // see one consistent footprint.
    //
    // Clip: an OSM river centerline is routinely drawn straight through a mapped lake and out
    // past the coast. Those stamps would override a LAKE bed, which is the one thing this
    // feature may never do, so subtract every non-river water polygon from the line stamp.
    // Rings are element geometry, so the subtraction is the same in every cell.
    let mut removed_stamp = BitGrid::new(n);
    for i in 0..n {
        if line_mask.get(i) && nonriver_mask.get(i) {
            line_mask.clear(i);
            removed_stamp.set(i);
        }
    }

    // Embedding suppression: a column whose only evidence is a line stamp, sitting deeper
    // inside land-cover water than its own tagged half-width can explain, is not a river of
    // that width - it is a centerline drawn across a wide body (a big river the tags
    // under-describe, or an ESA-only lake with no polygon to subtract). Overriding a narrow
    // ribbon there would leave a permanent half-blended stripe, plausibly worse than today,
    // so those columns keep the legacy bed wholesale.
    if line_mask.any() {
        let mut land_dt = vec![0u8; n];
        for (i, slot) in land_dt.iter_mut().enumerate() {
            if lc_water.get(i) {
                *slot = DT_MAX;
            }
        }
        chamfer_3_4_dt(&mut land_dt, lat.w, lat.h);
        for i in 0..n {
            if !line_mask.get(i) || poly_mask.get(i) {
                continue;
            }
            let land_d = f64::from(land_dt[i]) / DT_UNITS_PER_BLOCK;
            if land_d > f64::from(hw_line[i]) + EMBED_MARGIN {
                line_mask.clear(i);
                removed_stamp.set(i);
            }
        }
    }

    // ---- 5. The mask ---------------------------------------------------------------------
    let mut mask = BitGrid::new(n);
    for i in 0..n {
        if line_mask.get(i) || poly_mask.get(i) {
            mask.set(i);
        }
    }
    if !mask.any() {
        return RiverBedField::empty();
    }

    // ---- 6. Distance from the bank -------------------------------------------------------
    // Seeds are non-mask cells INSIDE the render bbox only. Standalone ways reach this field
    // already clipped to that bbox (`osm_parser.rs:1065`), so a river crossing the border has
    // its centerline truncated there: seeding the halo would read that truncation as a bank
    // and pinch the bed shut to depth 0 along every cell seam. Restricting seeds to the region
    // where element geometry is complete makes the near-seam column measure its true lateral
    // bank instead, which is the same bank the neighbouring cell measures.
    let mut dt = vec![DT_MAX; n];
    for j in 0..lat.h {
        let z = lat.min_z + j as i32;
        let in_z = z >= bb_min_z && z <= bb_max_z;
        for i in 0..lat.w {
            let idx = j * lat.w + i;
            if mask.get(idx) {
                continue;
            }
            let x = lat.min_x + i as i32;
            if in_z && x >= bb_min_x && x <= bb_max_x {
                dt[idx] = 0;
            }
        }
    }
    chamfer_3_4_dt(&mut dt, lat.w, lat.h);

    // ---- 7. Local half-width -------------------------------------------------------------
    // Line rivers carry a tag-derived half-width (element-keyed and unclipped, the tile-safe
    // pattern). Polygon rivers have none, so theirs is a windowed max of the bank distance -
    // a windowed max of a tile-invariant field, with the halo covering the window reach.
    let mut d_in_mask = vec![0u8; n];
    for (i, slot) in d_in_mask.iter_mut().enumerate() {
        if mask.get(i) {
            *slot = (f64::from(dt[i]) / DT_UNITS_PER_BLOCK).round().min(255.0) as u8;
        }
    }
    let win = ((16.0 * scale).round() as i32).max(1) as usize;
    let poly_hw = if poly_mask.any() {
        windowed_max_2d(&d_in_mask, lat.w, lat.h, win)
    } else {
        Vec::new()
    };

    // ---- 8. Profile ----------------------------------------------------------------------
    let d_max = river_depth_cap_blocks(scale);
    let mut hw_field = vec![0f32; n];
    let mut depth_f = vec![0f32; n];
    for i in 0..n {
        if !mask.get(i) {
            continue;
        }
        let own_d = f64::from(d_in_mask[i]);
        let mut hw = f64::from(hw_line[i]);
        if poly_mask.get(i) {
            hw = hw.max(f64::from(poly_hw[i]).max(own_d));
        }
        let hw = hw.max(1.0);
        hw_field[i] = hw as f32;
        let d_blocks = f64::from(dt[i]) / DT_UNITS_PER_BLOCK;
        depth_f[i] = river_profile_depth(d_blocks, hw, scale, d_max) as f32;
    }
    drop(dt);
    drop(d_in_mask);
    drop(poly_hw);
    // One 3x3 tent over the profile: it rounds the chamfer's octagonal facets off the depth
    // contours, which is the "smoothed shoreline" the design anchors on.
    let depth_f = tent3(&depth_f, lat.w, lat.h);

    // ---- 9. Confluence blend --------------------------------------------------------------
    // Where the river runs into water it does not own - a mapped lake, the sea, an ESA-only
    // body, or its own stamp columns that the clip/suppression removed - the profile must not
    // step into the legacy bed, it must arrive at it. `m` is the distance to that foreign
    // water; at `m = 0` the blend returns the legacy depth exactly, so the boundary pair is
    // equal by construction and the far side is never written differently at all.
    let mut m_dt = vec![DT_MAX; n];
    for j in 0..lat.h {
        for i in 0..lat.w {
            let idx = j * lat.w + i;
            if mask.get(idx) {
                continue;
            }
            let is_water = nonriver_mask.get(idx) || removed_stamp.get(idx) || lc_water.get(idx);
            if !is_water {
                continue;
            }
            if neighbours_mask(&mask, &lat, i, j) {
                m_dt[idx] = 0;
            }
        }
    }
    let has_seed = m_dt.contains(&0);
    if has_seed {
        chamfer_3_4_dt(&mut m_dt, lat.w, lat.h);
    }

    // ---- 10. Bake --------------------------------------------------------------------------
    let mut depth = vec![0u8; n];
    let mut out_mask = BitGrid::new(n);
    for j in 0..lat.h {
        let z = lat.min_z + j as i32;
        for i in 0..lat.w {
            let idx = j * lat.w + i;
            if !mask.get(idx) {
                continue;
            }
            let x = lat.min_x + i as i32;
            let river_f = f64::from(depth_f[idx]);
            let final_f = if has_seed {
                let hw = f64::from(hw_field[idx]);
                let band = (2.0 * hw).clamp(BAND_MIN, BAND_MAX);
                let m = f64::from(m_dt[idx]) / DT_UNITS_PER_BLOCK;
                let u = (m / band).clamp(0.0, 1.0);
                let wgt = smoothstep(u);
                let legacy_f = f64::from((inp.legacy_depth)(x, z));
                legacy_f + (river_f - legacy_f) * wgt
            } else {
                river_f
            };
            depth[idx] = final_f.round().clamp(0.0, f64::from(d_max)) as u8;
            out_mask.set(idx);
        }
    }

    RiverBedField {
        min_x: lat.min_x,
        min_z: lat.min_z,
        w: lat.w,
        h: lat.h,
        depth,
        mask: out_mask,
    }
}

// ---------------------------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------------------------

#[inline]
fn smoothstep(u: f64) -> f64 {
    3.0 * u * u - 2.0 * u * u * u
}

/// Depth cap by local half-width. `hw = 3 -> ~1.2`, `10 -> ~2.8`, `20 -> ~4.5`, `>= 30 -> 6`.
pub fn river_depth_cap_for_hw(hw: f64, d_max: i32) -> f64 {
    (6.0 * (hw / 30.0).powf(0.7)).clamp(1.0, f64::from(d_max))
}

/// The bed profile, in blocks, at `d_blocks` from the bank of a channel of half-width `hw`.
///
/// Smoothstep, not a literal parabola: a parabola is STEEPEST at the shore, the opposite of
/// "banks curve gently". Smoothstep has zero slope at both ends - a soft entry at the shore
/// and a broad rounded bottom - which is the U the reference cross-sections show.
///
/// The peak analytic bank slope is `1.5 * D / hw` per block (<= 0.3 for `hw >= 30`, and
/// `D <= 1.3` for `hw <= 4`), so adjacent columns can never differ by more than one block.
pub fn river_profile_depth(d_blocks: f64, hw: f64, scale: f64, d_max: i32) -> f64 {
    let hw = hw.max(1.0);
    let t = (d_blocks / hw).clamp(0.0, 1.0);
    // Bank roundness grows with width and with map scale: a narrow stream keeps a tight curve
    // (q ~ 1), a 1:1 wide river gets long soft banks (q = 1.5).
    let q = 1.0 + 0.5 * (hw / 30.0).min(1.0) * scale.min(1.0);
    let tau = t.powf(q);
    river_depth_cap_for_hw(hw, d_max) * smoothstep(tau)
}

// ---------------------------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------------------------

/// Mirror of `waterways::scaled_half_width` (private there, and `waterways.rs` is not this
/// change's to edit). The mask has to collapse at small scales exactly like the ribbon that
/// draws it, or the override would claim blocks the river never fills.
fn scaled_half_width(width: i32, scale: f64) -> i32 {
    let hw = (width / 2).max(0);
    if scale < 0.3 {
        0
    } else if scale < 0.7 {
        hw.min(1)
    } else {
        hw
    }
}

fn tag<'a>(tags: &'a std::collections::HashMap<String, String>, k: &str) -> Option<&'a str> {
    tags.get(k).map(String::as_str)
}

fn is_river_water_value(tags: &std::collections::HashMap<String, String>) -> bool {
    tag(tags, "water").is_some_and(|v| RIVER_WATER_VALUES.contains(&v))
}

/// Any water polygon that is NOT a river: bare `natural=water`, `natural=bay`, `water=lake`,
/// `water=oxbow` (an oxbow is a lake), and so on.
fn is_nonriver_water_tags(tags: &std::collections::HashMap<String, String>) -> bool {
    if is_river_water_value(tags) {
        return false;
    }
    tags.contains_key("water") || matches!(tag(tags, "natural"), Some("water") | Some("bay"))
}

/// Closed by shared node id, or by endpoints within a block (the tolerance `water_areas` uses).
fn ring_is_closed(nodes: &[ProcessedNode]) -> bool {
    if nodes.len() < 4 {
        return false;
    }
    let first = &nodes[0];
    let last = nodes.last().unwrap();
    first.id == last.id || ((first.x - last.x).abs() <= 1 && (first.z - last.z).abs() <= 1)
}

fn to_ring(nodes: &[ProcessedNode]) -> Option<Vec<XZPoint>> {
    ring_is_closed(nodes).then(|| nodes.iter().map(ProcessedNode::xz).collect())
}

/// Assemble a relation's outer/inner rings. Water relations keep their member ways UNCLIPPED
/// (`osm_parser.rs:1119`), which is what makes a relation-derived river mask tile-safe.
fn relation_rings(rel: &crate::osm_parser::ProcessedRelation) -> Option<PolyRings> {
    let mut outers: Vec<Vec<ProcessedNode>> = Vec::new();
    let mut inners: Vec<Vec<ProcessedNode>> = Vec::new();
    for mem in &rel.members {
        match mem.role {
            ProcessedMemberRole::Outer => outers.push(mem.way.nodes.clone()),
            ProcessedMemberRole::Inner => inners.push(mem.way.nodes.clone()),
            ProcessedMemberRole::Part => {}
        }
    }
    crate::element_processing::merge_way_segments(&mut outers);
    crate::element_processing::merge_way_segments(&mut inners);
    let outers: Vec<Vec<XZPoint>> = outers.iter().filter_map(|r| to_ring(r)).collect();
    if outers.is_empty() {
        return None;
    }
    let inners: Vec<Vec<XZPoint>> = inners.iter().filter_map(|r| to_ring(r)).collect();
    Some(PolyRings { outers, inners })
}

/// Tag-scan every parsed element. The dispatch cannot be reused for this: it routes any way
/// carrying a `natural` key to `natural.rs`, so the commonest river polygon of all -
/// `natural=water` + `water=river` on a closed way - never reaches `water_areas.rs` at all.
fn classify<'a>(
    elements: &'a [ProcessedElement],
    scale: f64,
    lines: &mut Vec<LineRiver<'a>>,
    river_polys: &mut Vec<PolyRings>,
    nonriver_polys: &mut Vec<PolyRings>,
) {
    for el in elements {
        match el {
            ProcessedElement::Way(way) => {
                let waterway = tag(&way.tags, "waterway");
                // Polygon rivers drawn as ways: `natural=water` + a river `water` value, and
                // `waterway=riverbank` (which `is_channel_waterway` drops, so it renders via
                // ESA only and has never had a mask of its own). Mask-only: their carve stays
                // the land-cover pass.
                let is_river_poly = (tag(&way.tags, "natural") == Some("water")
                    && is_river_water_value(&way.tags))
                    || waterway == Some("riverbank");
                if is_river_poly {
                    if let Some(ring) = to_ring(&way.nodes) {
                        river_polys.push(PolyRings {
                            outers: vec![ring],
                            inners: Vec::new(),
                        });
                    }
                    continue;
                }
                if is_nonriver_water_tags(&way.tags) {
                    if let Some(ring) = to_ring(&way.nodes) {
                        nonriver_polys.push(PolyRings {
                            outers: vec![ring],
                            inners: Vec::new(),
                        });
                    }
                    continue;
                }
                let Some(wt) = waterway else { continue };
                if !RIVER_WATERWAYS.contains(&wt) {
                    continue;
                }
                // Same gates the draw uses, so the mask can never claim a cell the ribbon
                // refuses to fill (a weir crest, a culvert running under a bank).
                if !is_channel_waterway(wt) || is_subgrade(way) || way.nodes.len() < 2 {
                    continue;
                }
                let hw_stamp = scaled_half_width(waterway_width(way), scale);
                lines.push(LineRiver {
                    way,
                    hw_stamp,
                    hw_profile: hw_stamp.clamp(1, 255) as u8,
                });
            }
            ProcessedElement::Relation(rel) => {
                if is_river_water_value(&rel.tags) {
                    if let Some(p) = relation_rings(rel) {
                        river_polys.push(p);
                    }
                } else if is_nonriver_water_tags(&rel.tags) {
                    if let Some(p) = relation_rings(rel) {
                        nonriver_polys.push(p);
                    }
                }
            }
            ProcessedElement::Node(_) => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Lattice + rasterization
// ---------------------------------------------------------------------------------------------

struct Lat {
    min_x: i32,
    min_z: i32,
    w: usize,
    h: usize,
}

impl Lat {
    #[inline]
    fn idx(&self, x: i32, z: i32) -> Option<usize> {
        let lx = i64::from(x) - i64::from(self.min_x);
        let lz = i64::from(z) - i64::from(self.min_z);
        if lx < 0 || lz < 0 || lx as usize >= self.w || lz as usize >= self.h {
            return None;
        }
        Some(lz as usize * self.w + lx as usize)
    }
    #[inline]
    fn max_x(&self) -> i32 {
        self.min_x + self.w as i32 - 1
    }
    #[inline]
    fn max_z(&self) -> i32 {
        self.min_z + self.h as i32 - 1
    }
}

#[inline]
fn grow_aabb(aabb: &mut (i32, i32, i32, i32), x: i32, z: i32, pad: i32) {
    aabb.0 = aabb.0.min(x.saturating_sub(pad));
    aabb.1 = aabb.1.max(x.saturating_add(pad));
    aabb.2 = aabb.2.min(z.saturating_sub(pad));
    aabb.3 = aabb.3.max(z.saturating_add(pad));
}

/// The lattice spans the river geometry padded by the halo, intersected with the render bbox
/// padded by the same halo. Relation rings arrive unclipped and can run far outside the render,
/// so the intersection is what keeps the allocation proportional to the render.
fn lattice_for(
    lines: &[LineRiver],
    river_polys: &[PolyRings],
    halo: i32,
    bb: (i32, i32, i32, i32),
) -> Option<Lat> {
    let mut aabb = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for lr in lines {
        let pad = lr.hw_stamp + 1;
        for n in &lr.way.nodes {
            grow_aabb(&mut aabb, n.x, n.z, pad);
        }
    }
    for p in river_polys {
        for ring in &p.outers {
            for pt in ring {
                grow_aabb(&mut aabb, pt.x, pt.z, 0);
            }
        }
    }
    let (lo_x, hi_x, lo_z, hi_z) = aabb;
    if lo_x > hi_x {
        return None;
    }
    let (bb_min_x, bb_max_x, bb_min_z, bb_max_z) = bb;
    let min_x = lo_x.saturating_sub(halo).max(bb_min_x.saturating_sub(halo));
    let max_x = hi_x.saturating_add(halo).min(bb_max_x.saturating_add(halo));
    let min_z = lo_z.saturating_sub(halo).max(bb_min_z.saturating_sub(halo));
    let max_z = hi_z.saturating_add(halo).min(bb_max_z.saturating_add(halo));
    if min_x > max_x || min_z > max_z {
        return None;
    }
    let w = (i64::from(max_x) - i64::from(min_x) + 1) as usize;
    let h = (i64::from(max_z) - i64::from(min_z) + 1) as usize;
    match w.checked_mul(h) {
        Some(t) if t > 0 && t <= MAX_LATTICE_CELLS => Some(Lat { min_x, min_z, w, h }),
        _ => {
            eprintln!("Warning: river area too large for --river-bed; using the legacy bed");
            None
        }
    }
}

/// The line footprint of `compute_waterway_field` (`waterways.rs:214-255`), geometry only.
///
/// The `ground_level <= seg_water_y + BANK_TOLERANCE` gate is deliberately DROPPED: it reads
/// `Ground`, which is bbox-clamped, so keeping it would make the mask depend on where the cell
/// border happens to fall. The apply-time "would carve today" intersect bounds the mask instead,
/// and it bounds it with data every cell agrees on.
fn stamp_line(lr: &LineRiver, lat: &Lat, mask: &mut BitGrid, hw_line: &mut [u8]) {
    let r = lr.hw_stamp + 1;
    for pair in lr.way.nodes.windows(2) {
        let a = pair[0].xz();
        let b = pair[1].xz();
        for (bx, _, bz) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z) {
            for x in (bx - r)..=(bx + r) {
                for z in (bz - r)..=(bz + r) {
                    if (x - bx).abs().max((z - bz).abs()) > r {
                        continue;
                    }
                    if let Some(i) = lat.idx(x, z) {
                        mask.set(i);
                        // Overlapping ways take the max: order-independent, so the mask cannot
                        // depend on element iteration order.
                        hw_line[i] = hw_line[i].max(lr.hw_profile);
                    }
                }
            }
        }
    }
}

fn rasterize_poly(p: &PolyRings, lat: &Lat, out: &mut BitGrid) {
    let mut z0 = i32::MAX;
    let mut z1 = i32::MIN;
    for ring in &p.outers {
        for pt in ring {
            z0 = z0.min(pt.z);
            z1 = z1.max(pt.z);
        }
    }
    let z0 = z0.max(lat.min_z);
    let z1 = z1.min(lat.max_z());
    if z0 > z1 {
        return;
    }
    let outer_groups: Vec<Vec<ScanlineEdge>> =
        p.outers.iter().map(|r| collect_ring_edges(r)).collect();
    let inner_edges = collect_all_ring_edges(&p.inners);
    let (lo_x, hi_x) = (lat.min_x, lat.max_x());
    for z in z0..=z1 {
        let zf = f64::from(z);
        let mut outer_spans: Vec<(i32, i32)> = Vec::new();
        for g in &outer_groups {
            let s = compute_scanline_spans(g, zf, lo_x, hi_x);
            if !s.is_empty() {
                outer_spans = union_spans(&outer_spans, &s);
            }
        }
        if outer_spans.is_empty() {
            continue;
        }
        let fill = if inner_edges.is_empty() {
            outer_spans
        } else {
            let inner_spans = compute_scanline_spans(&inner_edges, zf, lo_x, hi_x);
            if inner_spans.is_empty() {
                outer_spans
            } else {
                subtract_spans(&outer_spans, &inner_spans)
            }
        };
        for (s, e) in fill {
            for x in s..=e {
                if let Some(i) = lat.idx(x, z) {
                    out.set(i);
                }
            }
        }
    }
}

#[inline]
fn neighbours_mask(mask: &BitGrid, lat: &Lat, i: usize, j: usize) -> bool {
    let (w, h) = (lat.w, lat.h);
    for dj in -1i32..=1 {
        let nj = j as i32 + dj;
        if nj < 0 || nj as usize >= h {
            continue;
        }
        for di in -1i32..=1 {
            let ni = i as i32 + di;
            if ni < 0 || ni as usize >= w || (di == 0 && dj == 0) {
                continue;
            }
            if mask.get(nj as usize * w + ni as usize) {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------------------------
// Small array helpers
// ---------------------------------------------------------------------------------------------

/// A dense bitset over the lattice.
struct BitGrid {
    bits: Vec<u64>,
}

impl BitGrid {
    fn new(n: usize) -> Self {
        Self {
            bits: vec![0u64; n.div_ceil(64)],
        }
    }
    #[inline]
    fn get(&self, i: usize) -> bool {
        (self.bits[i >> 6] >> (i & 63)) & 1 == 1
    }
    #[inline]
    fn set(&mut self, i: usize) {
        self.bits[i >> 6] |= 1u64 << (i & 63);
    }
    #[inline]
    fn clear(&mut self, i: usize) {
        self.bits[i >> 6] &= !(1u64 << (i & 63));
    }
    fn any(&self) -> bool {
        self.bits.iter().any(|&b| b != 0)
    }
}

/// van Herk / Gil-Werman sliding maximum: O(n) regardless of window size.
fn sliding_max_1d(src: &[u8], r: usize) -> Vec<u8> {
    let n = src.len();
    if r == 0 || n == 0 {
        return src.to_vec();
    }
    let k = 2 * r + 1;
    let ext_len = (n + 2 * r).div_ceil(k) * k;
    let mut ext = vec![0u8; ext_len];
    ext[r..r + n].copy_from_slice(src);
    let mut pre = vec![0u8; ext_len];
    let mut suf = vec![0u8; ext_len];
    for b in (0..ext_len).step_by(k) {
        let e = b + k;
        let mut m = 0u8;
        for i in b..e {
            m = m.max(ext[i]);
            pre[i] = m;
        }
        let mut m = 0u8;
        for i in (b..e).rev() {
            m = m.max(ext[i]);
            suf[i] = m;
        }
    }
    (0..n).map(|i| suf[i].max(pre[i + 2 * r])).collect()
}

/// Separable square-window maximum of radius `r`.
fn windowed_max_2d(src: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut tmp = vec![0u8; src.len()];
    for j in 0..h {
        let row = &src[j * w..(j + 1) * w];
        tmp[j * w..(j + 1) * w].copy_from_slice(&sliding_max_1d(row, r));
    }
    let mut out = vec![0u8; src.len()];
    let mut col = vec![0u8; h];
    for i in 0..w {
        for (j, slot) in col.iter_mut().enumerate() {
            *slot = tmp[j * w + i];
        }
        let m = sliding_max_1d(&col, r);
        for (j, &v) in m.iter().enumerate() {
            out[j * w + i] = v;
        }
    }
    out
}

/// One separable 3x3 tent (`[1,2,1]/4` twice), edges replicated.
fn tent3(src: &[f32], w: usize, h: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; src.len()];
    for j in 0..h {
        let base = j * w;
        for i in 0..w {
            let l = src[base + i.saturating_sub(1)];
            let c = src[base + i];
            let rr = src[base + (i + 1).min(w - 1)];
            tmp[base + i] = 0.25 * l + 0.5 * c + 0.25 * rr;
        }
    }
    let mut out = vec![0f32; src.len()];
    for j in 0..h {
        let up = j.saturating_sub(1) * w;
        let cur = j * w;
        let dn = (j + 1).min(h - 1) * w;
        for i in 0..w {
            out[cur + i] = 0.25 * tmp[up + i] + 0.5 * tmp[cur + i] + 0.25 * tmp[dn + i];
        }
    }
    out
}

#[cfg(test)]
mod tests;
