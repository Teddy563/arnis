# Shoreline + building-realism port (2026-08-24)

Hand-port of two upstream louis-e/arnis feature families into the Meld fork,
planned by a three-way analysis (shoreline surface / building-overhaul slicing /
seam-gate constraints) and landed as five commits. The build-realism monolith
4196a220 stays rejected as a whole; these are the slices its wave-3 triage said
"stay candidates".

## What landed

| Commit | Upstream source | What |
|---|---|---|
| 726b8b41 | PR #1269 slice (9d8823b2) | underground buildings excluded from the surface footprint bitmap |
| 726206bf | bb1fb63c | per-type building height inference when OSM has no height data |
| f6839c32 | 4bef7ad2 | ArchEra classification; fork-native palette/frame consumer |
| 35460f15 | d594d585 (+4a5ce957) | EsaPixelRaster/GridMapping/sample_grid + sub-pixel shoreline reconstruction |

## Meld-specific decisions (divergences from upstream)

1. **Shoreline pass is OFF in master-origin cells.** Ring simplification is
   global per ring: a straight coast deliberately collapses to ONE fitted
   segment spanning the raster, rings are closed along each cell's raster
   border, and the DP splits + orthogonal-LSQ line fits depend on every ring
   vertex. Two adjacent Meld cells therefore provably receive different rings
   for the same coast, and **no finite halo guarantees agreement** (worst case
   ~1.4 ESA px = ~13 m of shore disagreement at a cell border). The fork's
   `level_water_surfaces` would amplify a 1-cell mask difference into a
   water-HEIGHT seam. So: single-bbox renders behave exactly like upstream;
   `--master-origin-*` renders skip the pass; `ARNIS_SHORELINE_TILED=1` is the
   experimental opt-in (probabilistic seams, unproven). The scale gate
   (`MIN_CELLS_PER_PIXEL = 2.0` ≈ scale 0.216) additionally no-ops the pass for
   Meld's 1:10/1:20 planet renders.
2. **Height inference reads the pre-clip polygon area.** Upstream feeds the
   footprint cell count; in Meld that count is the CLIPPED per-tile view and
   would steer the office/generic area tables differently in each tile sharing
   a building. `unclipped_polygon_area` (populated under
   `--tile-invariant-rendering`) is identical in every tile.
3. **Era consumer is fork-native.** A 3-way apply of upstream's 304-line
   consumer was attempted and rejected: it re-inserts upstream's
   WindowFrameStyle/wall-tag machinery that the fork already owns in diverged
   form. Instead the era hooks the fork's own decision points:
   `get_wall_block_for_category` (70% adherence, general urban categories
   only) and `pick_window_frame` (80%/55%/15% by era, inside the fork's
   category gate so the balcony decorator never double-decorates). Untagged
   parts inherit the group era via the style hint packed in the shared group
   seed.

## Verification record

- 451 unit/integration tests green (7 upstream shoreline tests included;
  13 new fork tests across the three slices).
- Golden gate per slice: footprints moved nothing; height inference moved all
  five fixtures (same five moved in upstream's own commit), double-run
  identical; era moved exactly midtown + munich_altstadt (the two fixtures
  with heritage/era tags — same footprint as upstream's commit note).
- Live acceptance: Constanța Black Sea coast at 1:1, fork vs bare upstream
  v3.1.0 — water-mask IoU **0.9872**, shoreline boundary-cell count within 2%
  (1967 vs 2009). The fork now shores like main arnis.
- Golden fixtures run `--offline` and never fetch ESA: the golden gate is
  structurally blind to the raster/shoreline change (recorded here so nobody
  reads "goldens unchanged" as evidence about it).
- Harness gotcha fixed along the way: `scripts/golden_hash.sh` runs
  `target/release/arnis.exe` and does NOT rebuild — a stale binary silently
  passes everything. Build first; the log now carries one honest attribution.

## Still open (deliberate)

- `osm_land_override.rs` (trim ESA false-water where OSM has land): planned,
  separate decision, ~1.5-2 days.
- Still-water prescan (1279-H) + coarse-field budget: deferred with the
  level_water_surfaces rewrite.
- B2 slice wave (Köppen wall palettes, unified Oklab block palette, facade
  module-first, Luanti completion): scoped in the port plan, not started.
- A 3×3 seam-grid fixture for the golden harness would allow revisiting the
  tiled-mode shoreline gate with evidence instead of reasoning.
