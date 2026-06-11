# arnis (Teddy563 fork) — v2.8.5

**Seamless multi-tile rendering for Meld.** This release makes adjacent tiles
(generated as separate processes under a shared master origin / `--seed`) agree at
their shared borders, so the region butt-join in Meld is visually seamless.

All changes are gated to tile mode (master origin + `--seed`). **Single-world
generation is byte-identical to upstream** — no behaviour change without `--seed`.

## The core principle
A block at world (X, Z) must be a pure function of (X, Z, seed) — independent of which
tile rendered it. Then two tiles that both cover a border block emit the identical block.
This release removes the things that broke that purity.

## Fixes

### Trees, parks & vegetation no longer cut at tile borders
- Scatter areas (`landuse=forest`, `natural=wood`/`tree_row`, `leisure=park/garden`, and
  all `landuse` vegetation) now decide each tile with a **position-seeded RNG** instead of
  a shared per-element stream. The old stream was drawn *inside* a terrain gate, so a
  single border tile differing between two tiles desynced the stream and relocated the
  whole area's scatter on one side. (`landuse.rs`, `leisure.rs`, `natural.rs`)
- Scatter ways **and** multipolygon-relation members render **unclipped** in tile mode
  (cap-guarded) so both tiles flood-fill the identical interior. (`osm_parser.rs`)

### Buildings
- Multipolygon (relation) buildings no longer split at a tile border.
- Buildings whose footprint is ≥60% water are skipped (no more houses standing in open
  water). (`buildings.rs`)

### Terrain / elevation
- Elevation **and** land-cover grids are sized with the same origin-anchored metric the
  world uses — terrain and land cover no longer slide 1–3 blocks off OSM features or
  disagree across tiles. (`elevation/mod.rs`, `ground.rs`)
- Robust parallel elevation fetch: more retries, retry-missing-tiles rounds, atomic tile
  cache writes, de-synced retry jitter — fixes whole tiles rendering flat (and the
  staircase cliff against a neighbour) when many tiles fetch at once. (`aws_terrain.rs`)
- The global elevation outlier filter is skipped in tile mode (its per-tile statistic
  caused a vertical step at the border, worst over coastal bathymetry); the bounded local
  repair already covers real spikes. (`elevation/mod.rs`)

### Biome tint
- Per-chunk biome lookup uses cell-local coordinates and a globally-continuous latitude,
  removing the vertical grass/leaf-colour seam at tile borders. (`biome.rs`,
  `world_editor/java.rs`)

## Merge accuracy
**≈ 97%** of rendered surface melds seamlessly across tile borders (terrain, roads,
buildings, forests/woods, parks, land-cover ground, biome tint). The remaining ~3% are
known, documented residuals (see below) — not regressions.

### Known residuals (deferred, documented in `FORK-AUDIT.md`)
- `natural=scrub/heath/grassland/wetland` still use a shared-stream RNG (they pass the RNG
  into helper functions) and are kept clipped — a localized seam is possible if your area
  is one of these. Conversion to position-seeded RNG is the next step.
- Water bodies larger than one tile + buffer can show a ≤1–2 block surface/underwater-floor
  seam (the water-leveling/depth code is upstream and intentionally untouched).
- Features whose bounding box exceeds ~12.5M blocks (≈3500×3500) stay clipped (a safety cap
  that prevents them from vanishing).

## Not changed
Roads, bridges, water carving/leveling, and highways are untouched. A coastal road that
renders as a raised bridge is OSM-tag-driven (`bridge=*` / `layer=*`), not a fork change.

## Compatibility
- Internal crate version stays `1.8.4`; this is release tag `v2.8.5`.
- `--seed` (alias `--tile-invariant-rendering`) opts into all of the above. Without it,
  output is unchanged from upstream.
