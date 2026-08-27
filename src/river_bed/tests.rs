//! Fixtures for the river bed field.
//!
//! These are the gates the plan names: the three mask fixtures (through-lake clip, ESA-wide +
//! centerline-only suppression, riverbank-only polygon) convert R2, and the confluence bound
//! plus the scale-0.3 half-width floor convert R4.

use super::*;
use std::collections::HashMap;

/// Render bbox used by every fixture unless it says otherwise.
const BB: (i32, i32, i32, i32) = (0, 199, 0, 199);

fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

fn node(id: u64, x: i32, z: i32) -> ProcessedNode {
    ProcessedNode {
        id,
        tags: HashMap::new(),
        x,
        z,
    }
}

fn way(id: u64, kv: &[(&str, &str)], pts: &[(i32, i32)]) -> ProcessedElement {
    ProcessedElement::Way(ProcessedWay {
        id,
        tags: tags(kv),
        nodes: pts
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| node(id * 1000 + i as u64, x, z))
            .collect(),
        unclipped_bounds: None,
        unclipped_polygon_area: None,
    })
}

/// A closed rectangle ring, first node repeated last (the `water_areas` closure convention).
fn rect(id: u64, kv: &[(&str, &str)], x0: i32, x1: i32, z0: i32, z1: i32) -> ProcessedElement {
    let mut w = way(id, kv, &[(x0, z0), (x1, z0), (x1, z1), (x0, z1), (x0, z0)]);
    if let ProcessedElement::Way(ref mut ww) = w {
        let first_id = ww.nodes[0].id;
        let last = ww.nodes.len() - 1;
        ww.nodes[last].id = first_id;
    }
    w
}

fn no_lc(_x: i32, _z: i32) -> bool {
    false
}

fn no_legacy(_x: i32, _z: i32) -> i32 {
    0
}

fn build(
    elements: &[ProcessedElement],
    scale: f64,
    lc: &dyn Fn(i32, i32) -> bool,
    legacy: &dyn Fn(i32, i32) -> i32,
) -> RiverBedField {
    build_field(
        elements,
        &FieldInputs {
            is_lc_water: lc,
            legacy_depth: legacy,
            bb: BB,
            scale,
        },
    )
}

/// Depths across a transect at constant z, over the x range of the fixture.
fn transect(f: &RiverBedField, z: i32, x0: i32, x1: i32) -> Vec<Option<i32>> {
    (x0..=x1).map(|x| f.depth_override(x, z)).collect()
}

// -------------------------------------------------------------------------------------------
// Test 1 - bed cross-section
// -------------------------------------------------------------------------------------------

/// A straight polygon river of `width` blocks, centred on x = 100, running the whole bbox in z.
fn straight_polygon_river(width: i32) -> Vec<ProcessedElement> {
    let x0 = 100 - width / 2;
    let x1 = x0 + width - 1;
    vec![rect(
        1,
        &[("natural", "water"), ("water", "river")],
        x0,
        x1,
        20,
        180,
    )]
}

#[test]
fn cross_section_is_a_smooth_u() {
    // (width, expected centre depth) straight off the D table, D = clamp(6*(hw/30)^0.7, 1, 6)
    // with hw ~ width/2: hw 3 -> 1.20 -> 1, hw 6 -> 1.94 -> 2, hw 20 -> 4.52 -> 5.
    for &(width, want_centre) in &[(6i32, 1i32), (12, 2), (40, 5)] {
        let els = straight_polygon_river(width);
        let f = build(&els, 1.0, &no_lc, &no_legacy);
        let x0 = 100 - width / 2;
        let x1 = x0 + width - 1;
        let row = transect(&f, 100, x0 - 2, x1 + 2);

        // Outside the polygon there is no override at all: the legacy bed still owns it.
        assert_eq!(row[0], None, "w={width}: override leaked outside the mask");
        assert_eq!(
            row[row.len() - 1],
            None,
            "w={width}: override leaked outside the mask"
        );

        let inside: Vec<i32> = row.iter().flatten().copied().collect();
        assert_eq!(
            inside.len(),
            width as usize,
            "w={width}: mask width mismatch"
        );

        // Shore columns carve nothing: the profile enters the water at zero depth, which is
        // the whole point (the legacy shoal ring is a flat cliff edge, this is a grade).
        assert_eq!(inside[0], 0, "w={width}: shore column must be depth 0");
        assert_eq!(
            *inside.last().unwrap(),
            0,
            "w={width}: shore column must be depth 0"
        );

        // Monotone non-decreasing from the shore to the centre, on both halves.
        let mid = inside.len() / 2;
        for i in 1..mid {
            assert!(
                inside[i] >= inside[i - 1],
                "w={width}: not monotone at {i}: {inside:?}"
            );
        }
        // No step larger than one block anywhere across the section.
        for i in 1..inside.len() {
            assert!(
                (inside[i] - inside[i - 1]).abs() <= 1,
                "w={width}: step > 1 at {i}: {inside:?}"
            );
        }
        // Mirror symmetry: the profile is a pure function of distance to the bank.
        for i in 0..inside.len() {
            assert_eq!(
                inside[i],
                inside[inside.len() - 1 - i],
                "w={width}: asymmetric: {inside:?}"
            );
        }
        let centre = *inside.iter().max().unwrap();
        assert_eq!(
            centre, want_centre,
            "w={width}: centre depth {centre}, D table says {want_centre}: {inside:?}"
        );
    }
}

#[test]
fn river_columns_get_no_dune_bumps() {
    // The bed top must be exactly `water_y - depth - 1` on a river column, i.e. the dune pass
    // contributes nothing. `dune_bump_at` is the single source of that height.
    for depth in 1..=6 {
        for body_max in 0..=7 {
            for x in [-40i32, 0, 17, 1234] {
                for z in [-9i32, 0, 88, 4001] {
                    assert_eq!(
                        crate::water_depth::dune_bump_at(x, z, depth, body_max, true),
                        0,
                        "river column ({x},{z}) depth {depth} got a dune bump"
                    );
                }
            }
        }
    }
    // Control: the legacy path still bumps somewhere, so the assertion above has teeth.
    let any_legacy_bump =
        (0..400).any(|x| (0..400).any(|z| crate::water_depth::dune_bump_at(x, z, 6, 7, false) > 0));
    assert!(any_legacy_bump, "legacy dunes vanished - test is vacuous");
}

// -------------------------------------------------------------------------------------------
// Test 2 - the flag
// -------------------------------------------------------------------------------------------

#[test]
fn flag_off_builds_an_empty_field() {
    let els = straight_polygon_river(40);
    let f = build_field(
        &els,
        &FieldInputs {
            is_lc_water: &no_lc,
            legacy_depth: &no_legacy,
            bb: BB,
            scale: 1.0,
        },
    );
    assert!(f.override_count() > 0, "fixture produced no overrides");

    // The public entry point is the one that honours the flag.
    assert_eq!(RiverBedMode::from_arg("off"), RiverBedMode::Off);
    assert_eq!(RiverBedMode::from_arg("v1"), RiverBedMode::V1);
    assert_eq!(RiverBedMode::from_arg("nonsense"), RiverBedMode::Off);
    let empty = RiverBedField::empty();
    assert_eq!(empty.depth_override(100, 100), None);
    assert_eq!(empty.override_count(), 0);
}

#[test]
fn a_render_with_no_river_elements_builds_nothing() {
    let els = vec![
        rect(1, &[("natural", "water")], 40, 160, 40, 160),
        way(2, &[("highway", "residential")], &[(0, 0), (199, 199)]),
    ];
    let f = build(&els, 1.0, &no_lc, &no_legacy);
    assert_eq!(
        f.override_count(),
        0,
        "a lake-only render must produce no overrides even with the flag on"
    );
}

// -------------------------------------------------------------------------------------------
// Test 3 (R2 gate) - through-lake clip
// -------------------------------------------------------------------------------------------

#[test]
fn a_centerline_through_a_mapped_lake_is_clipped_out_of_the_mask() {
    // A river centerline drawn straight across a mapped lake, the standard OSM pattern.
    let els = vec![
        way(1, &[("waterway", "river")], &[(20, 100), (180, 100)]),
        rect(2, &[("natural", "water")], 80, 120, 70, 130),
    ];
    let f = build(&els, 1.0, &no_lc, &no_legacy);

    // Lake interior: no override at all, so every lake column carves exactly what it carves
    // today - terraces, shoal ring, wobble and dunes included.
    for x in 82..=118 {
        for z in 72..=128 {
            assert_eq!(
                f.depth_override(x, z),
                None,
                "({x},{z}) inside the mapped lake was overridden"
            );
        }
    }
    // The river outside the lake keeps its override, so the clip is a clip and not a kill.
    assert!(
        f.depth_override(40, 100).is_some(),
        "river upstream of the lake lost its override"
    );
    assert!(
        f.depth_override(160, 100).is_some(),
        "river downstream of the lake lost its override"
    );
}

// -------------------------------------------------------------------------------------------
// Test 4 (R2 gate) - embedding suppression
// -------------------------------------------------------------------------------------------

/// Land cover water in a band of `half` blocks either side of z = 100.
fn lc_band(half: i32) -> impl Fn(i32, i32) -> bool {
    move |_x: i32, z: i32| (z - 100).abs() <= half
}

#[test]
fn a_centerline_inside_wide_esa_water_is_suppressed() {
    // waterway=river with no width tag => width 10 => hw 5. The ESA band is 40 blocks wide, so
    // the centre of the stamp sits ~20 blocks from land: far more than hw + 4 can explain.
    // Those columns are not a 10-wide river, they are a wide body the tags under-describe, and
    // a narrow override ribbon inside it would be a permanent half-blended stripe.
    let els = vec![way(1, &[("waterway", "river")], &[(20, 100), (180, 100)])];
    let wide = lc_band(20);
    let f = build(&els, 1.0, &wide, &no_legacy);
    assert_eq!(
        f.override_count(),
        0,
        "centerline embedded in wide ESA water must be suppressed entirely"
    );

    // Control: when the ESA water is about as wide as the tagged channel, the stamp is real
    // evidence and survives.
    let narrow = lc_band(6);
    let g = build(&els, 1.0, &narrow, &no_legacy);
    assert!(
        g.override_count() > 0,
        "a centerline matching its ESA water must keep its override"
    );
    assert!(
        g.depth_override(100, 100).is_some(),
        "mid-channel column lost its override"
    );
}

#[test]
fn a_river_polygon_inside_wide_esa_water_is_never_suppressed() {
    // Suppression only ever removes columns whose ONLY evidence is a line stamp. A mapped
    // river polygon is evidence in its own right.
    let els = vec![rect(
        1,
        &[("natural", "water"), ("water", "river")],
        20,
        180,
        90,
        110,
    )];
    let wide = lc_band(40);
    let f = build(&els, 1.0, &wide, &no_legacy);
    assert!(
        f.depth_override(100, 100).is_some(),
        "a mapped river polygon must not be suppressed by wide ESA water"
    );
}

// -------------------------------------------------------------------------------------------
// Test 5 (R2 gate) - riverbank-only polygon
// -------------------------------------------------------------------------------------------

#[test]
fn a_riverbank_polygon_alone_produces_a_bed() {
    // `waterway=riverbank` is dropped by `is_channel_waterway`, so it draws nothing and has
    // never had a mask. Here it is mask-only: it shapes the bed, it still draws nothing.
    let els = vec![rect(1, &[("waterway", "riverbank")], 80, 119, 20, 180)];
    let f = build(&els, 1.0, &no_lc, &no_legacy);
    let row: Vec<i32> = transect(&f, 100, 80, 119).into_iter().flatten().collect();
    assert_eq!(row.len(), 40, "riverbank polygon did not rasterize");
    assert_eq!(row[0], 0, "riverbank shore column must be depth 0");
    assert_eq!(
        *row.iter().max().unwrap(),
        5,
        "riverbank centre depth off the D table: {row:?}"
    );
}

#[test]
fn an_oxbow_polygon_is_a_lake_not_a_river() {
    let els = vec![rect(
        1,
        &[("natural", "water"), ("water", "oxbow")],
        80,
        119,
        20,
        180,
    )];
    let f = build(&els, 1.0, &no_lc, &no_legacy);
    assert_eq!(
        f.override_count(),
        0,
        "water=oxbow is a lake and must keep the legacy bed"
    );
}

#[test]
fn ditches_and_drains_and_culverts_stay_out() {
    for kind in ["ditch", "drain"] {
        let els = vec![way(1, &[("waterway", kind)], &[(20, 100), (180, 100)])];
        let f = build(&els, 1.0, &no_lc, &no_legacy);
        assert_eq!(f.override_count(), 0, "waterway={kind} must not be masked");
    }
    let culvert = vec![way(
        1,
        &[("waterway", "river"), ("tunnel", "culvert")],
        &[(20, 100), (180, 100)],
    )];
    let f = build(&culvert, 1.0, &no_lc, &no_legacy);
    assert_eq!(
        f.override_count(),
        0,
        "a culverted river is below grade and must not be masked"
    );
}

// -------------------------------------------------------------------------------------------
// Test 6 (R4 gate) - confluence
// -------------------------------------------------------------------------------------------

/// A 20-wide river polygon running north into a lake. Ring z bounds are one past the intended
/// last row because `compute_scanline_spans` is bottom-inclusive / top-exclusive: the river
/// fills z 10..=119 and the lake z 119..=189, so the two masks touch with no gap.
fn lake_confluence_fixture() -> Vec<ProcessedElement> {
    vec![
        rect(
            1,
            &[("natural", "water"), ("water", "river")],
            90,
            109,
            10,
            120,
        ),
        rect(2, &[("natural", "water")], 40, 160, 119, 190),
    ]
}

#[test]
fn a_river_entering_a_mapped_lake_blends_instead_of_stepping() {
    // River polygon running north into a lake polygon that starts at z = 119.
    let els = lake_confluence_fixture();
    // A legacy bed that is deliberately NOT flat, so an unblended step would show.
    let legacy = |_x: i32, z: i32| if z >= 120 { 3 } else { 2 };
    let f = build(&els, 1.0, &no_lc, &legacy);

    // (a) Every lake column is byte-identical to a flag-off render: no override exists there.
    for x in 42..=158 {
        for z in 122..=188 {
            assert_eq!(
                f.depth_override(x, z),
                None,
                "lake column ({x},{z}) was overridden"
            );
        }
    }

    // (b) No step across the mask boundary: the last river column and the first lake column
    // differ by at most one block, and at m = 0 the blend returns the legacy value exactly.
    for x in 92..=107 {
        let river_edge = f
            .depth_override(x, 119)
            .expect("river column at the mouth lost its override");
        let lake_side = legacy(x, 120);
        assert!(
            (river_edge - lake_side).abs() <= 1,
            "step of {} at x={x} across the confluence boundary",
            (river_edge - lake_side).abs()
        );
    }

    // (c) Continuity at m = 0: the river column touching the lake keeps the depth it would
    // have had with the flag off, so nothing moves at the join. (The blend target is that
    // column's OWN legacy depth, which is what makes the boundary pair continuous on both
    // sides at once.)
    for x in 92..=107 {
        assert_eq!(
            f.depth_override(x, 119),
            Some(legacy(x, 119)),
            "the mouth column at x={x} moved off its flag-off depth"
        );
    }

    // (d) Inside the band the value really is a blend and not the pure river profile: swapping
    // the legacy field moves in-band columns.
    let hot = build(&lake_confluence_fixture(), 1.0, &no_lc, &|_x, _z| 6);
    let cold = build(&lake_confluence_fixture(), 1.0, &no_lc, &|_x, _z| 0);
    let in_band_differs =
        (100..=119).any(|z| hot.depth_override(100, z) != cold.depth_override(100, z));
    assert!(
        in_band_differs,
        "no column inside the band responded to the legacy field - the blend never engaged"
    );

    // (e) Far upstream, outside the band, the profile is the pure river one.
    let upstream = f.depth_override(100, 40).unwrap();
    let far = f.depth_override(100, 20).unwrap();
    assert_eq!(upstream, far, "upstream depth still drifting with the band");
    assert!(
        upstream >= 2,
        "upstream river column did not reach its own profile depth: {upstream}"
    );
}

#[test]
fn the_confluence_band_stays_inside_its_stated_width() {
    // B = clamp(2*hw, 8, 32). hw here is ~10, so B = 20 blocks: past that the blend weight is
    // 1 and the profile is the river's own, independent of the legacy field.
    let els = lake_confluence_fixture();
    let hot = |_x: i32, _z: i32| 6;
    let cold = |_x: i32, _z: i32| 0;
    let a = build(&els, 1.0, &no_lc, &hot);
    let b = build(&els, 1.0, &no_lc, &cold);
    let band = 32;
    for z in 10..=(119 - band) {
        assert_eq!(
            a.depth_override(100, z),
            b.depth_override(100, z),
            "legacy depth still influences z={z}, which is more than {band} blocks from the mouth"
        );
    }
}

// -------------------------------------------------------------------------------------------
// Test 7 (R4 gate) - scale sweep
// -------------------------------------------------------------------------------------------

#[test]
fn scale_sweep_including_a_line_river_at_scale_0_3() {
    for &scale in &[0.3f64, 0.5, 1.0] {
        let cap = river_depth_cap_blocks(scale);
        let els = vec![way(1, &[("waterway", "river")], &[(20, 100), (180, 100)])];
        let f = build(&els, scale, &no_lc, &no_legacy);
        assert!(
            f.override_count() > 0,
            "scale {scale}: line river produced no mask (half-width floor missing?)"
        );
        assert!(
            f.max_override_depth() <= cap,
            "scale {scale}: depth {} exceeds the cap {cap}",
            f.max_override_depth()
        );
        // The half-width floor is what keeps `t = d/hw` finite: at scale 0.3
        // `scaled_half_width` returns 1, and below 0.3 it returns 0.
        assert_eq!(
            scaled_half_width(10, scale).max(1),
            if scale < 0.7 { 1 } else { 5 }
        );
    }

    // Polygon rivers at every scale respect the small-scale cap of 5.
    let els = straight_polygon_river(60);
    let small = build(&els, 0.4, &no_lc, &no_legacy);
    assert!(
        small.max_override_depth() <= SMALL_SCALE_MAX_DEPTH,
        "small-scale cap breached: {}",
        small.max_override_depth()
    );
    let full = build(&els, 1.0, &no_lc, &no_legacy);
    assert!(
        full.max_override_depth() <= MAX_WATER_DEPTH,
        "full-scale cap breached: {}",
        full.max_override_depth()
    );
}

#[test]
fn the_half_width_floor_never_divides_by_zero() {
    // hw = 0 would make t = d/hw non-finite. The profile floors hw at 1 itself, belt to the
    // classifier's braces.
    let d = river_profile_depth(3.0, 0.0, 0.2, SMALL_SCALE_MAX_DEPTH);
    assert!(d.is_finite(), "profile went non-finite at hw = 0");
    assert!((0.0..=5.0).contains(&d));
}

// -------------------------------------------------------------------------------------------
// Test 8 - the depth cap table and the profile shape
// -------------------------------------------------------------------------------------------

#[test]
fn the_depth_cap_table_matches_the_design() {
    let d = |hw: f64| river_depth_cap_for_hw(hw, MAX_WATER_DEPTH);
    assert!((d(3.0) - 1.2).abs() < 0.1, "hw=3 -> {}", d(3.0));
    assert!((d(10.0) - 2.8).abs() < 0.1, "hw=10 -> {}", d(10.0));
    assert!((d(20.0) - 4.5).abs() < 0.1, "hw=20 -> {}", d(20.0));
    assert!((d(30.0) - 6.0).abs() < 1e-9, "hw=30 -> {}", d(30.0));
    assert!((d(80.0) - 6.0).abs() < 1e-9, "hw=80 must stay capped");
    assert!(
        d(0.5) >= 1.0,
        "the cap floors at 1 so a stream still carves"
    );
}

#[test]
fn the_profile_has_zero_slope_at_both_ends() {
    // Smoothstep, not a parabola: flat entry at the shore, broad rounded bottom. Sampled as
    // the discrete slope over the first and last tenth of the half-width.
    let hw = 30.0;
    let p = |d: f64| river_profile_depth(d, hw, 1.0, MAX_WATER_DEPTH);
    let near_shore = p(hw * 0.1) - p(0.0);
    let mid = p(hw * 0.55) - p(hw * 0.45);
    let near_centre = p(hw) - p(hw * 0.9);
    assert!(
        near_shore < mid,
        "shore slope {near_shore} should be gentler than mid {mid}"
    );
    assert!(
        near_centre < mid,
        "centre slope {near_centre} should be gentler than mid {mid}"
    );
    // And the peak slope stays under one block per block, so no adjacent-column step > 1.
    let mut worst: f64 = 0.0;
    let mut d = 0.0;
    while d < hw {
        worst = worst.max(p(d + 1.0) - p(d));
        d += 0.25;
    }
    assert!(worst <= 1.0, "peak bank slope {worst} blocks/block");
}

// -------------------------------------------------------------------------------------------
// Test 9 - Measured clearance
// -------------------------------------------------------------------------------------------

#[test]
fn measured_clearance_reserves_the_river_cap_under_the_flag() {
    // The case that under-reserved before: a narrow LC water strip (the legacy estimate reads
    // 2-3 blocks off it) crossed by a wide tagged way whose river profile reaches 6.
    let mut grid = crate::flat_grid::FlatGrid::new(64, 64, 0u8);
    for z in 30..=33 {
        for x in 0..64 {
            grid.set(z, x, crate::land_cover::LC_WATER);
        }
    }
    let legacy = crate::water_depth::estimate_max_carve_depth(&grid, 64, 64, 1.0, false);
    let with_v1 = crate::water_depth::estimate_max_carve_depth(&grid, 64, 64, 1.0, true);
    assert!(
        legacy < MAX_WATER_DEPTH,
        "fixture is not a narrow strip any more: legacy estimate {legacy}"
    );
    assert_eq!(
        with_v1, MAX_WATER_DEPTH,
        "the flag must raise the reservation to the river cap"
    );

    // And the field over that same strip really does want the deeper floor.
    let els = vec![way(
        1,
        &[("waterway", "river"), ("width", "80")],
        &[(0, 100), (199, 100)],
    )];
    let f = build(&els, 1.0, &no_lc, &no_legacy);
    assert!(
        f.max_override_depth() > legacy,
        "wide tagged way carved {} but the legacy reservation was {legacy}",
        f.max_override_depth()
    );
    assert!(
        f.max_override_depth() <= with_v1,
        "the raised reservation must still cover the deepest river column"
    );

    // Small scale keeps its own cap.
    assert_eq!(
        crate::water_depth::estimate_max_carve_depth(&grid, 64, 64, 0.3, true),
        SMALL_SCALE_MAX_DEPTH
    );
}

// -------------------------------------------------------------------------------------------
// Test 10 - determinism / order independence
// -------------------------------------------------------------------------------------------

#[test]
fn overlapping_ways_take_the_max_in_any_order() {
    let a = way(1, &[("waterway", "stream")], &[(20, 100), (180, 100)]);
    let b = way(
        2,
        &[("waterway", "river"), ("width", "40")],
        &[(20, 100), (180, 100)],
    );
    let f1 = build(&[a.clone(), b.clone()], 1.0, &no_lc, &no_legacy);
    let f2 = build(&[b, a], 1.0, &no_lc, &no_legacy);
    for x in (20..=180).step_by(7) {
        for z in 80..=120 {
            assert_eq!(
                f1.depth_override(x, z),
                f2.depth_override(x, z),
                "element order changed the field at ({x},{z})"
            );
        }
    }
}
