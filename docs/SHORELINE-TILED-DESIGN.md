# Making shoreline smoothing work in Meld merged worlds

Research, 2026-08-24. Three designs analysed in parallel against the shipped
3.1.6 code. The pass currently runs only in single-bbox renders and is gated
off when `--master-origin-*` is set, because ring simplification is global per
ring: each cell traces rings on its own crop, so two cells disagree about the
same coast.

## What the measurement showed

A throwaway experiment cropped one synthetic global coastline two ways, ran
`reconstruct_water_shoreline` on both crops, and counted disagreeing cells in
the shared strip:

| raster halo H | disagreeing cells (strip of 257 x 1257) |
|---|---|
| 0 px | 1949 |
| 16 px | 59 |
| 32 px | 882 |
| 64 px | 731 |

Non-monotonic, and divergence does not decay with distance from the crop edge
(300-375 disagreeing cells per 100-column band deep in the interior). A bigger
halo does not converge, which confirms the gate comment empirically: the DP
seed cascade, the whole-run least-squares refits and the per-crop area fallback
couple the whole ring. Any design that keeps per-cell global rings is out.

## The three routes

### A. World-anchored windows (recommended)

Partition the global ESA pixel grid into fixed 256 px windows, window id =
`pixel.div_euclid(256)`. This is the same world-lattice technique that took the
field-texture seam from 28.4% to 100.000% agreement (`road_bearings.rs`). Every
cell computes the same windows from the same global pixels, with an 8 px halo
and interior-only write-back, so rings are world-anchored instead of
crop-anchored.

The load-bearing part is not the window loop, it is the coordinate frame: the
pixel-to-grid mapping must be rebuilt from the master origin in global block
coordinates, then offset by the cell's integer block position. Per-cell bbox
floats differ in the last ulp and flip a `ceil()` at a seam column; the
experiment saw 260 such nearest-neighbour disagreements on a pathologically
aligned mapping. Same fix also stabilises the base sampling.

The area-change fallback must be evaluated over the full window core, never
clipped to the cell grid, or a core straddling a seam counts different regions
in each cell and the binary keep/revert decision diverges.

Cost: raster grows to window alignment (a 1:1 cell goes from about 12 KB to
140 KB), fetches still hit the shared COG cache. Effort 1.5 to 2 days.
Quality: full smoothing everywhere including across seams, with sub-pixel
window-border jogs every 2.5 km (0 to 1 blocks typical, worst case about 7
blocks on adversarially curved coasts, further softened by the existing class
boundary smoothing). Confidence 85%.

### B. Shoreline bake

Precompute the rings once per world-anchored 1 degree chunk, publish them as
immutable vector files in the land-cover cache, and have cells rasterize the
baked rings instead of tracing their own. A fourth Prepare data pack next to
elevation, OSM and Overture, with `--prewarm-shoreline` mirroring
`--prewarm-overture`.

Strongest determinism of the three: cells never recompute geometry, they read
frozen bytes, so even a different binary cannot make them disagree. Important
correction from the analysis: the artifact must be the simplified rings, not a
corrected pixel layer, or re-quantising to 10 m reintroduces the staircase the
pass exists to remove. Rings are also kilobytes per chunk instead of ~145 MB.

Cost: about 700 to 900 lines of Rust plus 330 lines of Meld plumbing, a bake
step users must run, roughly 65 MB and 5 to 15 s per chunk cold. Confidence
85%, but it is the heaviest option and it still needs B's global-block
rasterization to avoid the same float-tie seam A must fix.

### C. Halo plus no-edit inset

Keep the per-cell pass, add a 16 px raster halo, and skip shoreline edits
within `seam_buffer + 16` blocks of the grid edge. This leans on merge
geometry: Meld discards every region outside the canonical rectangle, so the
256 block overlap strip between two cells is kept from exactly one of them.
All strip divergence is therefore invisible except across the single seam line,
and the inset makes that line pixel-identical to the shipped baseline.

Cheapest by far, about 1 day, roughly 30 lines across both repos. The price is
a 32 block staircase corridor centred on every seam, where the coast keeps
exactly the stairs the feature removes. Confidence 80%.

## Recommendation

Go with A. It is the only route that delivers the actual goal, a smoothed coast
that continues across cell borders, at a cost close to C's and well under B's.
Its determinism is structural rather than incidental: identical world-anchored
inputs through provably deterministic code (no HashMap, no rayon, no RNG in the
module, scan-order-determined rings, no fast-math).

Sequence:

1. Master-frame `GridMapping` in global block coordinates. Ship and verify this
   first, on its own. It stabilises the base land-cover sampling for every
   tiled render whether or not the shoreline pass ever runs, and it is the
   precondition for A, B and C alike.
2. Windowed driver in `land_cover_shoreline.rs`, inner algorithms untouched,
   core-clipped rasterization, per-core area fallback.
3. Window-aligned raster extent in `EsaPixelRaster::covering`.
4. Flip the gate: tiled renders take the windowed path, single-bbox keeps the
   global-ring path unchanged, `ARNIS_SHORELINE_TILED=off` as the opt-out.

Verification gate, in order of strength:

- Two-cell equivalence unit test: one synthetic global raster, two overlapping
  cell extents differing by an integer block offset, assert byte-identical
  classes over the shared strip. This is the test that catches every
  bookkeeping mistake in steps 1 and 2, and it is the permanent regression
  guard the feature currently lacks.
- Window-kink bound test: straight coast at several angles crossing a window
  border, assert the reconstructed boundary jogs at most 1 px.
- Live 2x2 Meld render over a real coast, block-diff the shared strips, then
  look at the window lines at 1:1.

Keep C's inset in the back pocket. If the live 2x2 shows any residual seam that
step 1 did not explain, the inset contains it without reverting the feature.

## Status

Step 1 is done (commit 70b8cf21): the master frame ships, gx/gz return frame
coordinates, rounding happens in frame space and the cell offset is applied as
an exact integer afterwards. Four tests walk every shared pixel of two adjacent
cells. The single-bbox path is byte-identical, confirmed by the golden gate.
Steps 2 to 4, the windowed driver itself, are not started.

Two water defects found while doing it, both fixed in the same commit: the bed
profile flooring away half a block per column, and `compute_water_distance`
treating a cropped cell's edge as coastline (which carved one bank of a
seam-crossing channel shallower than the other).

MEASURED 2026-08-24, and it overturns the ranking below. A 4x4 grid of cells
was rendered over Navodari (Black Sea coast, Lake Siutghiol, the canal) with
the shipped 3.1.6 and with a candidate fix, using Meld's real geometry: shared
master origin, the elevation lock, a 128-block halo per cell, and columns
within 24 blocks of a rendered edge excluded because the merge discards them.
Water surface height was compared for every world column two neighbouring cells
both produced.

    shipped 3.1.6   4411 of 1069037 shared columns disagree (0.41%), worst 6
    candidate fix   4850 of 1070199 shared columns disagree (0.45%), worst 6

The candidate replaced `level_water_surfaces`' body-wide statistics with
bounded local ones, which is what this document previously called the biggest
remaining water defect. It is not. The four worst cell pairs are unchanged by
it - 1735 to 1732, 1344 to 1331, 785 to 786, 363 to 363, every one of them
100% of their shared columns at up to 6 blocks - so whatever drives the real
seam is upstream of surface levelling. All four involve the westernmost column
of cells, which is the thread to pull next. The candidate is parked on
`wip/water-surface-bounded`; it was not shipped, because it adds a code path
and measures slightly worse.

Two intermediate harness mistakes worth remembering, because both produced
confident wrong answers:

- Omitting the elevation lock let each cell normalise its own height range, so
  identical real heights mapped to different Y. That alone reported 637 and 353
  disagreements where the truth was 46 and 263.
- Comparing cells rendered edge-to-edge, with no halo, sampled exactly the
  columns where a bounded window is truncated. That made the bounded candidate
  look 5x worse than shipped when the real difference is a few percent.

Still open, no longer ranked until the real cause is identified:

1. Whatever produces the 6-block steps on the western cell pairs above.
2. Water that exists only in OSM is never surface-levelled: the levelling runs
   during the elevation fetch on the ESA-only mask, and OSM water is stamped
   afterwards, so an OSM-only river keeps raw DEM noise under it.
3. `comp_max` is a per-component maximum taken over the crop, so a body cut by
   a cell boundary can pick a different depth band on each side. Worth zero to
   one block.

The bed geometry does NOT need the window treatment: its distance transform
saturates at 85 blocks, inside Meld's 128-block halo, and it already treats its
array edge as open rather than as shore.

## Notes carried forward

- The scale gate stays: below about scale 0.216 the pass no-ops, so 1:10 and
  1:20 planet renders are unaffected either way. This work matters for 1:4 and
  finer projects.
- `level_water_surfaces` amplifies a one-cell mask difference into a water
  height seam, which is why the bar here is byte-identity and not "close
  enough".
- A transient COG range-request failure in one cell but not its neighbour still
  produces different window bytes. Same class as any cache-miss divergence the
  fork already lives with; the shared disk cache plus a prewarm makes it rare.
