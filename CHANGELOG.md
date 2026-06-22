# Changelog - Meld fork

All releases of the Meld fork of louis-e/arnis. Follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format.

Starting with this release the fork tracks the upstream Arnis version number (2.9.0); earlier entries used an internal 1.8.x sequence.

## [2.9.2] - 2026-06-22

Two themes: load your own tree models, and shape the terrain. This release adds a pack-agnostic schematic tree system and two terrain knobs on top of the 2.9.1 generator. It ships no tree art; it places whatever pack directory you point it at.

### Added

- **`--tree-pack <DIR>` (schematic tree packs).** Points Arnis at a directory of Sponge `.schem` tree models plus a `region.json` manifest and places those models instead of the built-in procedural trees. The engine picks a realm by location, a community by terrain, then a weighted species, with a vanilla-tree blend mixed in, and stamps the models inside Arnis's own tree-spawn pass so density and clearings stay natural. Mutually optional: without the flag, the built-in procedural trees are used exactly as before. Usage: `arnis --output-dir ./world --bbox <bbox> --tree-pack ./packs/eur`.
- **`--tree-sizes <list>` (size by scale).** Comma list of `small,medium,big,tall,giant` choosing which height tiers may place. A disabled tier falls back to a smaller one rather than leaving a gap, and large tiers are gated off on small scales so a 1:N city does not sprout 40-block giants. Usage: `arnis ... --tree-pack ./packs/eur --tree-sizes small,medium,big`.
- **`--vertical-exaggeration <FACTOR>`.** Multiplies terrain height only, not the map footprint, so relief that flattens at small scales comes back; it auto-compresses to the build height. Default 1.0. Usage: `arnis ... --terrain --vertical-exaggeration 2.0`.
- **`--snow-mode off|realistic|peaks|manual` plus `--snow-percent <P>` / `--snow-y <Y>`.** Controls snow placement: off, the real latitude snow line, the top N percent of world height (`--snow-percent`), or above a fixed Y (`--snow-y`). Usage: `arnis ... --terrain --snow-mode peaks --snow-percent 15`.

### Fixed

- **Tree anchoring (float fix).** Trees anchor on slopes and at water edges instead of floating, never root in water, may overhang the bank with canopy, and never clip into buildings.
- **Small-scale water.** Scale-aware river and pool depth (a linear bowl below scale 0.5), line-river width scale-capped so 1:10 streams stay 3 blocks or under, and a more varied riverbed. Trees and leaves are swept off the water surface so the waterline stays clean.

### Changed / Engine

- **`--no-buildings` skips street furniture.** Also drops street-furniture nodes (lampposts, signs, traffic signals), not just building footprints.

## [2.9.1] - 2026-06-18

A maintenance release on top of 2.9.0. It lets a Meld orchestrator feed each cell its own slice of a shared OSM tile cache directly (no per-cell merge step), removes a water rendering artifact that could flood a triangle of a cell, and makes the Overture building fetch skip work it does not need.

### Added

- **`--osm-tile-dir <DIR>` plus `--osm-tile-z <Z>` (tile mode).** Reads OSM for `--bbox` straight from a directory of stable web mercator grid tiles (`osm_g1_z{Z}_{X}_{Y}.json`, default Z 11) instead of one merged `--file`. Arnis computes the tiles that overlap `--bbox`, loads each, and de-duplicates elements by (type, id), so an orchestrator can fill the directory once and hand the same directory to every cell with zero per-cell assembly. Mutually exclusive with `--file`; a missing tile is skipped. Usage: `arnis --output-dir ./world --bbox <cell-bbox> --osm-tile-dir ./cache/osm --osm-tile-z 11`.
- **`--prewarm-overture`.** Fetches and caches the Overture building partitions and STAC index for `--bbox`, then exits, so later parallel cells read the buildings from disk instead of each refetching them. Requires only `--bbox`. Usage: `arnis --bbox <region-bbox> --prewarm-overture`.

### Fixed

- **Triangle and rectangle water floods (the "wedge").** A water multipolygon whose member ways were not all present (for example a lake that extends beyond the loaded tiles) left its outer ring open. The bbox clip then closed that open ring with a straight chord and the scanline fill flooded the whole side of it with water, drawing a hard diagonal edge across the cell. `clip_water_ring_to_bbox` now rejects an open input ring (first node not matching last by id or within one block) and returns nothing, so a broken outline is dropped rather than closed with a fake edge. Sibling rings that are properly closed still render, so legitimate water is preserved. Verified on real data: a wedge cell dropped from 135,600 to 4,555 water blocks (triangle gone, real river kept) while a clean river cell was byte identical.

### Changed / Engine

- **Overture buildings gated on `--no-buildings`.** The Overture Maps building fetch and render now run only when buildings are enabled, so a `--no-buildings` run no longer downloads building partitions it will not use (measured: a roads-and-ground cell dropped from about 28.8 s to about 4.2 s).
- **Overture disk cache.** The STAC index is cached locally with a 7 day TTL, and per request byte ranges are cached under the Arnis cache root, so repeated cells over an area read the buildings from disk with no extra network. The earlier full partition lock that could stall a whole batch on one slow download was removed.

## [2.9.0] - 2026-06-16

A fork of louis-e/arnis 2.9.0 tuned for large parallel "Meld" generation: one orchestrator slices a region into many adjacent cells and bakes them into a single Minecraft world. This release pulls the fork up to the upstream 2.9.0 line (53 merged commits, in-process tile parallelization, stream-to-disk eviction) and adds the cross-tile machinery (shared origin, global elevation band, seam-free buildings, parallel-safe terrain fetch) that keeps those cells lining up block for block.

### Added

- **`--no-buildings` (alias `--no-structures`).** Drops all OSM buildings plus building-adjacent features (man_made, power, barriers, doors, advertising, historic, emergency, tourism) while keeping roads, rail, water, land cover and terrain, for a roads-and-ground-only Meld base layer; parking and some leisure/surface features are deliberately kept. Usage: `arnis --output-dir ./world --bbox <bbox> --no-buildings`.
- **`--tile-invariant-rendering` (alias `--seed`).** Makes a building that straddles two adjacent tiles render byte-identical in both, by reading pre-clip bounds and mixing the seed into every RNG stream. Bare flag means seed 1; omitting it keeps upstream behaviour. Usage: `arnis --output-dir ./world --bbox <cell-bbox> --seed 42`.
- **`--road-detail max|clean|compact`.** Trades road detail for legibility at low scale where footways, crossings and lane dividers stack into checker noise. `max` (default) is upstream-exact, `clean` is a cleanup pass for scale >= 0.7, `compact` keeps vehicle roads only and caps lanes to 2. Gates both the Overpass query and per-element render. Usage: `arnis --output-dir ./world --bbox <bbox> --scale 0.1 --road-detail compact`.
- **`--overpass-url`.** Overrides the Overpass endpoint(s) with a comma-separated, priority-ordered list that replaces the built-in public mirror pool, so a self-hosted instance can absorb large parallel batches without per-IP rate limits. Usage: `arnis --output-dir ./world --bbox <bbox> --overpass-url http://localhost:12345/api/interpreter`.
- **`--download-only` plus `--save-json-file`.** Fetches the OSM data for `--bbox` to a JSON file and exits, so a scheduler can pull a whole region's OSM once and feed it to many cells via `--file` instead of each cell tripping the public Overpass limit. Honours `--overpass-url` and `--road-detail`. Usage: `arnis --bbox <region-bbox> --download-only --save-json-file region.json`.
- **`--download-terrain-only`.** Warms the AWS terrain (elevation) tile cache for `--bbox` in one single-process pass (8 concurrent) and exits, so later parallel cells hit the cache instead of bursting S3. Requires only `--bbox`; exits 0 if all tiles cached, 2 if any failed. Usage: `arnis --bbox <region-bbox> --download-terrain-only`.
- **`--offline` (alias `--elevation-cache-only`).** Hard offline elevation: cache hits serve as usual, but a cache miss or corrupt tile returns an error instead of re-downloading (Arnis then falls back to flat ground), so a batch run never quietly hammers S3 or regional providers. Pair it with a prior `--download-terrain-only`. Usage: `arnis --output-dir ./world --bbox <bbox> --terrain --offline`.
- **`ARNIS_ELEV_ZOOM` env var.** Caps the terrain tile zoom for the whole run (clamped to the valid band) so elevation stays lighter and hole-free; a coarser zoom such as z13 still carries the full roughly 30 m signal while sidestepping the z14/z15 no-data holes and downloading far fewer tiles. No CLI flag. Usage (bash): `ARNIS_ELEV_ZOOM=13 arnis --output-dir ./world --bbox <bbox> --terrain` (PowerShell: `$env:ARNIS_ELEV_ZOOM='13'`).
- **`--master-origin-lat` / `--master-origin-lng` (tile mode).** Anchors the projection and the elevation/land-cover grid to one global lat/lng so every cell shares a single Minecraft XZ ruler and lines up block for block; also flips Arnis into "tile mode" (skips the per-run stale-tile cache cleanup and the global outlier-filter pass) and fixes the roughly 0.1% avg-lat haversine stretch. Both must be passed together. Usage: `arnis --output-dir ./world --bbox <cell-bbox> --master-origin-lat 52.5200 --master-origin-lng 13.4050`.
- **`--elevation-min` / `--elevation-max` (global band).** Pins one shared real-world min/max in metres so every tile maps height to Y identically, instead of each tile picking its own range and meeting neighbours at a vertical staircase. Both required together; set alongside `--master-origin`. Usage: `arnis ... --master-origin-lat <lat> --master-origin-lng <lng> --elevation-min 0 --elevation-max 1200`.
- **Automatic flat low-scale bridges.** At `--scale 0.3` or lower, every bridge becomes a flat 1-block deck that hugs the terrain (Beam style forced, no arch), because at 1:3 or smaller a rising arch with columns and clearance collapses into noise and overshoots the tiny span. Not a flag; triggered purely by `--scale`. Usage (implicit): `arnis --output-dir ./world --bbox <bbox> --scale 0.1`.
- **Big water / shore / wetland system plus 9 new block IDs.** A large multi-pass water, underwater, shore and wetland system (flat per-component water surface, single-cell SAND shore swap, depth-tiered bed palette, water carving under bridges, stoney-shore ring, thin-land drown) that resolves the long-running stepped-water, double-slope, AIR-hole and stray-sand artifacts. Adds 9 new block IDs (256 to 265) and widens `Block::id()` from u8 to u16. Automatic when `--terrain` is on; shore and water noise respond to the `--seed` value, with no dedicated flag.
- **Snow-capped terrain above the real-world snow line.** Terrain above the latitude-derived climatic snow line gets a thin snow layer (a new `SNOW_LAYER` block at id 266), with a 6-block noise jitter so the edge is not a hard contour. It honours the `--elevation-min/--elevation-max` lock so the cap lands at the same Y on both sides of a tile seam, and it skips water and shore (gated on the same water-blend isoline the shore uses). Automatic when `--terrain` is on.
- **Parking lots rendered like roads.** Parking areas use a speckled asphalt mix (honouring `surface=*`) with white `WHITE_CONCRETE` space markings and a slim metal lamp post, instead of flat gray concrete. The lamp post is gated behind buildings, so `--no-buildings` draws only the flat pavement, no posts.

### Changed / Engine

- **Merged 53 upstream louis-e/arnis commits.** Brings in-process tile parallelization, stream-to-disk region eviction, the mimalloc allocator, the large-area warning, and the GUI ETA, while keeping the cross-tile seam intact (0 of 1024 chunks differ in verification). The `transformation.rs` Local plus master-origin path is the seam crux.
- **Product renamed to "Arnis Meld Fork".**
- **GUI footer now credits louis-e and Teddy563.**
- **Upstream sync.** Ported the genuinely-new commits from the upstream 2.9.0 line that compose with
  the fork: the progress-bar sheen animation, a clearer extend-build-height tooltip, leisure=marina
  maps to water, a corrected u16 block-id note, the snow line, the parking visual upgrade, and the
  one-byte section storage split (see Added and Performance). The CI was also brought green (rustfmt
  plus a dead-code allow on the unused Web Mercator path). Upstream changes that fight the fork's
  design were deliberately skipped: road-width-by-lanes (our `--road-detail` already caps lanes in
  compact mode), and the upstream water beds (the fork's water rewrite is already a superset).

### Performance

- **One byte per cell for the common section.** Widening `Block` to u16 had doubled every full
  section's backing vector (a roughly 70% peak-memory jump on a dense city). Section storage now uses
  `Full(Vec<u8>)` for the overwhelmingly common case where every block id fits in a byte, and
  `FullWide(Vec<Block>)` only for the rare sections that hold an id of 256 or more. Reads, writes,
  iteration and the seam-merge content hash are representation-independent (a section hashes
  identically either way), verified by a round-trip plus a Full-versus-FullWide canary test.

### Fixed

- **Parallel-safe terrain tile fetch.** Under Meld's roughly 64 concurrent fetches, cells were getting rate-limited and truncated tiles that became flat seams. Fetch now uses 6 retries with exponential backoff, a deterministic per-tile jitter, an atomic temp-then-rename cache write, and retry-missing rounds, killing the staircase seam at the build center.
- **i32 corner-sum overflow panic on far-from-origin master coordinates.**
- **Cross-tile coordinate drift under master-origin.** `transform_point` derived metres-per-degree-longitude from `avg_lat`, shearing the grid like a fan so cells slid off the shared 512-block region grid by an amount that grows with distance from the origin, leaving flat-grass strips at tile seams; now anchored at `origin_lat` so a longitude maps to one block-X project-wide. Plain single-world generation is byte-identical.
- **Tree trunk apex cap.**
- **Minecraft 1.21.4 to 26.1.x datapack schema overlays.**
- **GUI disk-probe false-block fixes.**

## [1.8.4] - 2026-06-09

### Fixed
- **Cross-tile coordinate drift under `--master-origin-lat/lng`.** `transform_point` derived metres-per-degree-longitude from `avg_lat = (point_lat + origin_lat) / 2`, so the **same longitude mapped to a different block-X depending on the point's latitude** - the grid sheared like a fan instead of a rectangle. Invisible in a single world, but for external schedulers that stitch adjacent bboxes into one Minecraft world (Meld) it pushed each tile off the shared 512-block region grid by an amount that **grows with distance from the origin**, leaving flat-grass **"unrendered" strips at tile seams** that widen toward the far corners. Now anchored at `origin_lat` so a longitude maps to **one** block-X project-wide, matching the latitude axis (which already uses the constant `METERS_PER_DEG_LAT`). Plain single-world generation (no master origin) is byte-identical. Adds a latitude-invariance regression test.

## [1.8.3] - 2026-06-02

### Added
- 9 new Block IDs (256-265, u16 widening): `MAGMA_BLOCK`, `SUGAR_CANE`, `KELP`, `TALL_SEAGRASS_BOTTOM/TOP`, `SEA_PICKLE`, `BROWN_CANDLE_{2,3,4}`, `SOUL_SAND`, each with correct NBT side-table arms.
- Per-cell underwater bed picker with domain-warped noise - organic vanilla-MC-style patches (CLAY/SAND/DIRT/COARSE_DIRT) on a GRAVEL background.
- Rare 5-13 cell MAGMA + SOUL_SAND vents at depth ≥ 5 (bubble columns in MC).
- Underwater dunes: width-aware amplitude 2-4 blocks, domain-warped, prominent waves.
- SEAGRASS meadow mix (short + tall + sea pickle); KELP min-3-cell variable-height columns.
- Wetland post-pass: MOSS_BLOCK ring + COARSE_DIRT 2-ring around water puddles.
- Tiered cattail: 1-2 stalks + single `candles=1/2/3/4` BROWN_CANDLE block.
- Shore-land rare cattail (2%) + sugar_cane (1%) on SAND/DIRT/COARSE/GRASS at water-edge.
- `sweep_floating_veg` post-pass: removes cattail/grass/candles/sugar_cane/flowers over water cells + roads; trees (LOG/LEAVES) explicitly excluded.
- `--seed` (alias of `--tile-invariant-rendering`) now drives global noise seed → identical seed reproduces identical bed/dune/shore patterns.
- STONE under-bed fill 12 cells below `bed_y-1` → no AIR pockets in neighbour columns.

### Changed
- Slope curve: linear → `depth = local_max × √(dist/span)` (rounded, steeper near shore).
- Shore palette: high-frequency coord-hash bins → smooth `value_noise` (scale 16) static fade SAND → COARSE_DIRT → GRASS_BLOCK over 5-cell band.
- Stoney shore variant (STONE_BRICKS / COBBLESTONE / etc neighbour): COBBLE 50% + MOSSY_COBBLE 30% + COARSE_DIRT 20%.
- Underwater bed `under_block` always STONE (was SANDSTONE under SAND surface).
- Wetland branch matches `arnis-source-water` per-subtype structure (drops earlier 8-bin roll); less grass overuse, more bare MUD.

### Fixed
- **Fauna carving holes in bed**: `set_block_absolute(veg, .., None, Some(&[]))` empty-list whitelist was *always-replace* (matched nothing → fell through). Now uses `Some(&[AIR])` so veg only paints into AIR cells.
- **Veg placed inside dunes** (visual holes): vegetation now planted at `bed_top + 1` where `bed_top = bed_y + dune_bump_at(x,z)` instead of `bed_y + 1`.
- **Bed surface AIR pockets visible through translucent water**: STONE under-fill.
- **Floating veg over flooded wetland cells + roads**: `sweep_floating_veg` post-pass.
- **5×5 bridge ring** in OSM water-polygon carve (matches LC_WATER pass) → clean GRAVEL bed under causeway shadows, no DIRT strips at diagonals.

### Versioning
- Bumped to **1.8.3** (Meld fork numbering, downstream of Teddy563/arnis 2.8.1 and louis-e/arnis v2.8.0 base).

### Internal
- `value_noise_01` reads from `OnceLock<NOISE_SEED>` set by `ground_generation::set_noise_seed`, called once at the top of `generate_world_with_options`.
- New helpers: `dune_bump_at`, `sweep_floating_veg`.
- `Block::id()` return type widened `u8` → `u16` (required for IDs 256-265).

---

## [2.8.1] - 2026-05-21 (Teddy563)

Previous Meld release. Voxelize / 3DMR work + Meld scheduler + tile-invariant rendering + road-detail palette improvements.

## [v2.8.0] - 2026-05-19 (louis-e upstream)

Base upstream release inherited by Meld v1.8.3. Adds the BigWaterField depth carve + initial wetland G3 + universal LC_WATER carve pass.
