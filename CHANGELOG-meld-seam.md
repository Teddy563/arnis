# Arnis fork — Meld seam-fix batch (CHANGELOG)

Status: **UNCOMMITTED** on top of git HEAD `0dc5678`. Local-only; exe shipped to
`light-meld/arnis.exe`. **Not pushed.** This batch makes the Arnis fork render every
block as a pure function of its global position + seed, so adjacent Meld cells produce
identical blocks at their shared border and the region butt-join is seamless.

Scope vs HEAD: **12 files, +383 / −65** (pre-cleanup), now +~360 / −78 after the DRY
cleanup. All changes are gated on master-origin / `--seed`; **single-world output is
byte-identical** to upstream.

---

## 1. Coordinate alignment — terrain/land-cover no longer drift off OSM
**Files:** `elevation/mod.rs`, `ground.rs`, `coordinate_system/transformation.rs` (test)

- `compute_grid_dims(bbox, scale, master_origin_lat, master_origin_lng)` — gained the
  origin args. Under master-origin it now sizes the elevation **and** land-cover grid
  with the *same* origin-anchored metres-per-degree the world layout uses
  (`111320`, `mpd_lon` at origin lat), instead of haversine `geo_distance` at the
  bbox's average latitude. The old ~0.1 % stretch slid terrain/land-cover 1–3 blocks
  off buildings/roads mid-cell and made tiles disagree at the seam.
- Threaded `master_origin_lat/lng` through `Ground::new_enabled` and
  `generate_ground_data`.
- Added regression test `test_grid_dims_match_world_extent_under_master_origin`
  (asserts grid extent == xzbbox block extent).

## 2. Biome seam — grass/leaf tint no longer steps at the border
**Files:** `biome.rs`, `world_editor/java.rs`

- `build_chunk_biome_nbt` gained `origin_x/origin_z` and now subtracts them before the
  `cover_class`/`water_distance` lookup (those index a **cell-local** grid). It was the
  one caller feeding absolute coords, so each tile clamped to the grid's edge column and
  picked a different biome → hard vertical tint seam.
- New `biome_lat_for_chunk`: in tile mode each chunk's biome latitude is reconstructed
  from its absolute world-Z on this cell's global lat line, so two cells agree on a
  shared chunk's latitude (no temperature-biome step).

## 3. Trees meld across the border
**Files:** `osm_parser.rs`, `element_processing/landuse.rs`, `element_processing/natural.rs`,
`ground_generation.rs`, `floodfill.rs`

- `landuse=forest` / `natural=wood,tree_row` **ways render UNCLIPPED** in tile mode
  (`osm_parser.rs`). The tree spawn walks the flood-filled polygon **interior**, and a
  per-cell-clipped polygon flood-fills differently (50k algo branch / multi-seed /
  timeout all key off the clipped bbox) → a tile gets a tree in one cell but not its
  neighbour. Same polygon → same interior → same trees.
- Tree/flower spawn RNG switched to a **position-seeded** `coord_rng(x, z, id)` in tile
  mode (`landuse.rs`, `natural.rs`); the shared RNG stream was consumed in per-cell
  flood order, so the border decided differently.
- Added `tile_invariant_enabled()` helper (`ground_generation.rs`) = `--seed` set.
- `MAX_FLOOD_FILL_AREA` made `pub` so the cap guard can be shared.

## 4. Buildings no longer cut in half at the border
**Files:** `osm_parser.rs` (single ways), `element_processing/buildings.rs` (relations)

- Building / `building:part` ways and merged relation rings render **UNCLIPPED** in
  tile mode. `clip_way_to_bbox` sealed each tile's half with a synthetic wall →
  half a building per tile. `set_block` already clamps writes to the cell bbox, so the
  clip was redundant for safety.
- **Cap-guarded:** an oversized footprint (bbox ≥ ½ the flood-fill cap) keeps the clip,
  because an over-cap flood fill returns EMPTY → the building would *vanish*.

## 5. Buildings no longer spawn submerged in water
**File:** `element_processing/buildings.rs`

- A building whose footprint is **≥ 60 % `LC_WATER`** is skipped. Floor Y was chosen as
  max-ground-over-footprint with no water check, so the later water carve flooded the
  house. Deterministic (grid-indexed) → every straddling tile agrees. Piers/stilts over
  a minority of water are preserved.

## 6. Elevation fetch reliability — no more whole-flat regions
**File:** `elevation/providers/aws_terrain.rs`

- Retries **3 → 6**, base backoff **500 → 600 ms**, plus deterministic per-tile
  **jitter** to de-sync the ~64 parallel S3 requests Meld launches.
- **Retry-missing-tiles loop:** up to 4 rounds with `800ms·2ⁿ` backoff re-fetch only the
  still-missing tiles before sampling — a cell never proceeds on a half-fetched grid.
- **Atomic cache write:** stage to a per-PID temp file then `rename`, so parallel cells
  sharing one cache dir can't read a half-written PNG → garbage decode → NaN → flat
  region.

## 7. Coastal step — building+road+water+tree-ground no longer step at the seam (this session)
**File:** `elevation/mod.rs`

- `filter_elevation_outliers` (a **global IQR over each cell's own grid**) is **skipped
  under master-origin**. Two cells computed different reject bands → a border cell was
  NaN'd-then-interpolated in one cell but kept in the other → a vertical step in ground,
  road, water shore and building base (worst over Black-Sea bathymetry; the ground under
  trees stepped too, which read as "trees cut"). The bounded 5×5 MAD repair already
  removes local spikes seam-safely; water bathymetry is flattened by
  `level_water_surfaces`. Single-world keeps the filter → byte-identical.
- Full analysis: `light-meld/docs-temp/seam-merge-rootcause.md`.

## 8. Cleanup (this session) — DRY the cap guard
**Files:** `osm_parser.rs`, `element_processing/buildings.rs`

- Extracted the un-clip cap-guard predicate (`overlaps && area < MAX_FLOOD_FILL_AREA/2`)
  into one shared `osm_parser::tile_unclip_within_cap(bounds, xzbbox)`. It was duplicated
  in the way path and the relation-ring path; a future edit to one copy could have let
  trees and buildings un-clip under different rules. Behaviour-identical.

## 9. Forest meld at the border — relation (multipolygon) forests (this session)
**File:** `osm_parser.rs` (relation pass)

- `landuse=forest` / `natural=wood,tree_row` **multipolygon RELATION** members now render
  **UNCLIPPED** in tile mode (cap-guarded per member), the same as standalone ways (§3).
  The way un-clip never ran for relation members, so a forest tagged as a relation stayed
  clipped per-cell → its flood-fill interior differed across the seam → trees didn't meld
  (the "−X region" report). `generate_*_from_relation` flood-fills each outer member
  **individually**, so the per-member bbox is the exact flood-fill polygon — the cap guard
  (`tile_unclip_within_cap`) is applied per member. Single-world keeps the clip →
  byte-identical.
- Adversarially verified: an earlier hypothesis (flood-fill wall-clock `--timeout 600`
  truncation / grid-sampled sparsity) was **refuted** — forest ways already un-clip and
  cache by id, and the 600 s budget never fires on a sub-12.5M forest. The real cause was
  relation members staying clipped.

## 10. Forest/park seam — generalize un-clip to ALL scatter areas (this session)
**File:** `osm_parser.rs`

- The §3/§9 un-clip whitelist (`landuse=forest`, `natural=wood/tree_row`) was too
  narrow. **Every** flood-fill area that scatters per-position content seeds its RNG
  with `element_rng(element.id)` and then **consumes it in flood-fill iteration order**
  (`for (x,z) in filled_area`). A per-cell clip changes that order/membership, so the
  same id-seeded stream lands the scatter on different tiles each side of the seam.
  `leisure=park/garden/nature_reserve`, `natural=scrub/heath/grassland/wetland`,
  `landuse=cemetery/meadow/grass/…` were all still clipped → still seamed (the residual
  −X/+Z park the user kept seeing).
- Replaced the narrow whitelist (way path **and** relation path) with one shared
  predicate `is_scatter_area_tags(tags)` = buildings ∪ all `leisure` ∪ all non-water
  non-point `natural` ∪ vegetation/recreation `landuse` ∪ `amenity=grave_yard`,
  explicitly **excluding water** (own ring-clip path) and **pure-ground landuse**
  (residential/industrial/farmland paving — uniform fill, no seam, often huge → keep
  clipped to bound flood-fill cost). Un-clip makes `flood_fill_area` return the identical
  Vec in both cells → identical `element_rng` consumption → identical scatter → meld.
  No per-site RNG surgery needed (un-clip alone fixes the id-seeded sites). Cap-guarded;
  single-world byte-identical. This also **simplifies** §3/§9 into one predicate.

## 11. Scatter seam — the REAL cause: conditional-RNG desync (this session)
**Files:** `element_processing/leisure.rs`, `element_processing/landuse.rs`, `osm_parser.rs`

- §3/§9/§10 chased *membership* (un-clip). The actual cause is an **RNG desync**:
  `generate_leisure`/`generate_landuse`/`generate_natural` seed `element_rng(element.id)`
  once and draw from it **inside terrain gates** (`if check_for_block(GRASS_BLOCK)`)
  while iterating the flood fill. When a tile's ground differs between two cells (a path
  vs grass at a border), one cell draws and the other doesn't → the shared stream
  **desyncs** → every later tile scatters differently → the whole area mismatches. The
  forest arm was immune because it already used a **per-tile `coord_rng(x,z,id)`** (fresh,
  independent — a gate flip affects only that tile).
- §10's broad un-clip made `filled_area` the full polygon in both cells, so the stream
  traversed more tiles → more desync → the seam spread to **every** axis (the regression
  the user saw).
- Fix: reseed a **per-tile `coord_rng`** at the top of the fill loop in `leisure` and
  `landuse` (folding the old per-arm forest coord_rng into it). No shared stream → no
  cascade. Single-world keeps the id stream (`tile_inv` gate) → byte-identical.
- `natural` scrub/heath/grassland/wetland still use the stream (and pass `&mut rng` to
  helpers), so converting them is a separate careful pass — for now they're **removed
  from the un-clip set** (only `wood`/`tree_row` un-clip) so un-clip can't amplify their
  desync. They keep the pre-existing localized behavior until converted.

---

## Verification
`cargo fmt --check` 0 · `cargo clippy --all-targets --all-features -- -D warnings` 0 ·
`cargo build --release` 0 · `cargo test` **187 passed / 1 ignored**.

## Known residuals (documented, not in this batch)
- Forest **way OR relation member** with unclipped bbox ≥ 12.5M (~3500 blocks/side) still
  clips per-cell (the cap guard prevents a vanish); rare. Would need a cell-independent
  point-in-polygon spawn against the unclipped ring instead of flood-fill membership.
- Large-lake **surface clamp** (`clamp_by_adjacent_land`) + underwater **depth tier**
  (`water_depth` `component_max`) can step on bodies bigger than cell+buffer — both live
  near upstream water code that regressed when last touched; deferred to an isolated pass.
- `fill_nan_values` divergence on a genuinely missing tile — mitigated by §6.
