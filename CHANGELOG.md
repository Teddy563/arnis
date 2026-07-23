# Changelog - Meld fork

All releases of the Meld fork of louis-e/arnis. Follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format.

This fork is the generator behind [Meld](https://github.com/Teddy563/meld): Meld slices a real-world
selection into region-aligned cells and runs one arnis process per cell; everything the fork adds
(shared master origin, global elevation band, tile-invariant rendering, tile-mode OSM/terrain reads,
tree packs, terrain shaping, caves) exists so that many independently generated cells merge into one
seamless world. Every flag is additive — omit it and upstream behaviour is preserved.

Starting with 2.9.0 the fork tracks the upstream Arnis version number; earlier entries used an internal 1.8.x sequence.

## [3.0.4] - 2026-07-23

Configurable farmland texturing plus scattered rocks and bushes. Every change is
seam-safe (a pure function of world coordinates) and additive; omit the new flags and
default runs stay byte-identical.

### Added

- **`--field-mix <LIST>` — configurable farmland texturing.** Splits `landuse=farmland`
  into a weighted mix of five styles: `coarse` (coarse dirt + dead bush), `plains`
  (grass), `flower` (grass + wildflowers), `farm` (stock tilled crops), and `moss`
  (overgrown moss). Value is a `name=pct` list of relative area shares, e.g.
  `plains=60,coarse=20,flower=10,farm=10,moss=15`. The category is chosen per
  domain-warped ~24-block blob, so each style forms large coherent fields with organic
  edges (not per-block static) while still covering ≈ its weight share of the area; all
  seam-safe. Omitted (or a farm-only mix) reproduces stock tilled farmland exactly.
- **`--rocks` / `--rock-density <0-64>` — scattered rock formations.** Evenly scatters
  bundled andesite/tuff rock schematics (8 shapes) on farmland at random rotations, in
  small numbers set by the density. Off by default.
- **`--bushes` / `--bush-density <0-64>` — scattered bushes.** Evenly scatters bundled
  bush schematics (10 species × 6 shapes) on farmland at random rotations, gently
  clumped (bushes grow in small groups). Off by default. Each bush carries its own bark
  pole within leaf-decay distance, so the foliage survives in-game.

  Both scatter families use a jittered-grid distribution (no clumping artefacts, reliably
  hits the target count) seeded purely by position → identical across tile seams.

All three reuse the existing generic Sponge stamping engine; anchors are sampled from
the stable field-cell list and rotations are position-hashed, so placement is identical
from any tile (no seam artefacts).

## [3.0.3] - 2026-07-15

Water and cave rendering fixes plus organic climate boundaries. Every change is seam-safe (a pure function of world coordinates) and additive; default runs stay byte-identical away from the affected water/cave surfaces.

### Fixed

- **Water colour bands.** The water biome no longer uses the warm, lukewarm, and cold ocean temperature variants, which gave inland lakes and wide rivers visible colour lines and wrongly tinted temperate inland water. Non-freezing water is now river, ocean, or deep_ocean (all one shared tint) and freezing water is the frozen trio, so a water body reads as a single uniform colour. The ocean vs deep_ocean split is kept for mob spawning and is colour identical, so it adds no seam.
- **Thin land and road lines across water.** Untagged roads and tracks crossing water at small scale are now drowned instead of painting a 1-block causeway straight across a lake or river, and interior terrain ridges that poked above the water surface are submerged instead of rendering as a thin grass line. Real shores, islands, and tagged bridges are unaffected. The road drown is kept in sync with the road mask so no dry gap is left where a crossing is removed.
- **Cave lava seam trench.** The deep lava sea left a 2-block-wide air trench at every cell and region seam because its containment check treated an out-of-bbox neighbour as open, so both tiles skipped their boundary columns. The check now ignores out-of-bbox neighbours, so both tiles fill the seam and the lava is continuous. Interior generation is byte identical.

### Changed

- **Climate boundaries render as organic blobs.** The Koppen lookup is domain warped by a smooth low-frequency noise field before sampling the 0.1 degree grid, so the blocky rectangular climate edges bend into natural blobs. It is a pure function of latitude and longitude, so it stays seam-safe across tiles, and the world and the climate preview change together.

## [3.0.2] - 2026-07-14

Additive feature release for the Meld climate, heightmap, tree, and seam work. Every change is additive and seam-safe; omit the new flags and behaviour is unchanged, and default runs stay byte-identical.

### Added

- **Per-position Koppen climate.** Climate (biome tint and arid/polar surface blocks) is now sampled per block from the bundled Koppen grid instead of once per cell, so a large tiled region varies smoothly across the map instead of snapping to one climate per cell. New `Climate::at(lat, lon)` and `Ground::climate_at(x, z)` drive both the biome and surface-palette paths.
- **`--climate-map <PREFIX>`.** Early-exit render mode: writes a top-down Koppen climate PNG for the bbox (one colour per grouped climate, the same grouping generation uses) plus a `CLIMATEMAP {json}` stats line. Pure function of the bbox, lines up with a geographic overlay.
- **Tree size popularity weights.** New `--tree-size-weights small=..,big=..,giant=..` sets relative per-tier popularity (100 = the pack default share, 0 = off, 200 = about double) by reweighting the existing tile-invariant size roll. Scale gates still apply (Giant only at 1:1, tiny maps never place tall/giant). Legacy `--tree-sizes` stays as an on/off alias. Default weights run the original picker verbatim, so output is byte-identical.
- **`--prewarm-elevation`.** Early-exit that fills the exact per-tile elevation cache the generation cells read (Mapterhorn globally, or the regional/AWS provider the fork picks) without sampling a grid. Companion to `--prewarm-overture`; lets Meld bake a region offline so cells never rate-limit the tile server.
- **`--elevation-map <PREFIX>`.** Early-exit that renders the heightmap for the bbox to a PNG using the real provider stack (so the preview matches the world), hillshade or grayscale, with an `ELEVMAP {json}` line carrying min/max metres and bbox for a map overlay.

### Fixed

- **Boundary props no longer clip at cell seams.** Node-placed prop schematics (wind turbine, lighthouse, crane, excavator, tractor, tombstone, and the rest) were assigned to exactly one strict tile with no halo, while area ways get the editor halo, so a prop straddling a cell boundary lost the voxels that crossed into the neighbour tile at merge. Nodes now get the same halo as area ways, so both cells stamp the prop; safe because every node handler places deterministically on world-absolute coordinates (idempotent, byte-identical writes).

## [3.0.1] - 2026-07-12

Upstream parity release: brings the fork to feature parity with **louis-e/arnis 3.0.0** (plus fork-only fixes upstream does not have), while keeping every Meld differentiator: tile-invariant rendering, shared master origin, `--road-detail`, the native cave engine, the scale-aware water/shore work, offline mode, and the master-origin elevation grid. Every feature below was ported faithfully and audited for cross-tile seam-safety by an adversarial review pass before shipping; the fork's #1 invariant is that independently generated cells merge into one seamless world.

### Added

- **New CLI commands and flags.** `--props all|none|<list>` (bundled 3D props per family), `--loot-table <json>` / `--dump-loot-table` (configurable chest loot), `--gamemode survival|creative|spectator`, `--world-time <ticks>`, `--map-item` (locked world map in the player inventory), and `--map-item-only` (add the map to an existing merged world in one post-pass, the Meld path).
- **Bundled 3D props at real OSM features.** Boats on open water, parked cars in car parks, cranes and excavators on construction sites, tractors on farmland, wind turbines at wind generators, lighthouses, fountains, playgrounds, cemetery tombstones, and the Starbase Pad 2 Starship landmark easter egg. Per-family toggle; omitting the flag keeps every family on.
- **Skyscraper facade taxonomy + OSM StyleHint.** True skyscrapers render in six distinct facades (glass curtain, glass with concrete corner pillars, glass in a concrete grid, contemporary concrete frame, horizontal-band modern, and stone/art-deco masonry) instead of two. A tag classifier reads `building:material` / `facade:material`, `roof:material=glass`, `historic` / `heritage` / `ref:nrhp` / `listed_status`, masonry and concrete material and cladding lists, `building:architecture`, and `start_date` (pre-1945 = masonry) to pin a tower's style, propagated across all parts of a multi-part (S3DB) building so untagged parts match.
- **Facade detailing.** A darker stone base course / plinth on normal buildings; full-glass storefront bays across commercial and hotel ground floors; string courses and a crown cornice on plain facades; coherent per-building window frames (bands, posts, hanging lanterns, French-balcony rails, stud buttons) on commercial, hotel and historic buildings; and a per-building window phase so blocks of buildings no longer share one citywide window grid.
- **S3DB multi-part buildings.** Outlines are suppressed only when their `building:part` members actually cover at least half of them, plus a spatial pass for relation-less S3DB; all parts of one building share a style seed so colour and roof stay coherent.
- **Furnished interiors, sport pitch markings, world settings.** Interior furniture with real beds, `leisure=pitch` line markings, and game mode / time of day written into `level.dat`.
- **Climate.** Koppen-driven biomes by real-world climate, plus climate-aware water: warm / lukewarm / cold / frozen oceans (with deep variants offshore), frozen rivers in polar and boreal regions, and tropical `sparse_jungle` shrubland.
- **Infrastructure.** Covered highway tunnels, street lamps along lit ways, electrified railway catenary, modular bridge deck schematics (flat low-detail bridges preserved), more OSM surface materials, `amenity=bicycle_parking` surfaces, `landuse=farmyard`, `aeroway=helipad` pads (light-gray pad, white ring, painted "H") for way and node helipads, and power line spans that hang straight between poles instead of hugging the terrain.
- **Trees.** Deliberately-mapped street trees may stand on plazas and sidewalks; trees no longer grow up through bridge decks; a canopy drapes over an adjacent low roof instead of being sliced flat at the building edge.
- **Mapterhorn global elevation** (default). Global terrarium tiles (GLO-30 floor, national LiDAR to z18) with pyramid-parent hole-proofing that structurally eliminates the broken-tile cliff artifacts of the legacy AWS source. Regional high-res providers (USGS, IGN France/Spain, Japan GSI) still win in their coverage; legacy AWS stays available via `--aws-only-elevation`, and a fork-only offline gate makes Mapterhorn honour `--offline`.

### Fixed

- **Stream-to-disk corruption guard.** Sign, entity and chest writers now bail on an already-flushed region like the block writer does, so large exports can no longer resurrect a stale region and truncate saved chunk data.
- **CLI spawn-Y.** The post-generation spawn-Y correction targets the actual world folder, so nested and desktop-output worlds get a correct spawn height.
- **Farmland irrigation** water is placed only where it sits in a basin, so it no longer runs downhill and washes out crops on sloped fields.
- **Helipad seam.** The pad renderer's aggregate rooftop-skip and the parked-helicopter prop (both keyed on cell-local state) were removed after the seam audit flagged them; the tile-invariant pad, ring and "H" remain.

### Notes

- Only the parked helicopter prop remains deferred: it needs a seam-safe region-ownership guard before it can be placed. The fork's residential window decorator is kept for houses; window frames apply only to the building types it does not cover, so the two never double-decorate.
- Block ids that collide with upstream were assigned at the fork's next-free ids rather than adopting upstream's palette wholesale, so existing fork worlds and assets are unaffected.

## [2.9.3] - 2026-07-04

Underground update: one cave system, done right. The experimental cave engine is gone; `--caves` now runs a from-scratch Rust port of Minecraft 1.21.8 cave worldgen, carved directly into the filled ground at generation time. This is the engine behind Meld 1.5.0 "Meld Depths" — Meld's Caves toggle passes `--caves` to every cell, and because every cave pass is a pure function of (seed, position), the caves line up across cell seams like the surface does.

### Added

- **`--caves` (cave worldgen).** Clean-room port of vanilla 1.21.8 cave generation: the noise density field (cheese caverns, spaghetti tunnels, entrance pockets, pillars, noodle worms) sampled with vanilla's 4x8x4 cell interpolation, plus the random-walk tunnel/ravine carvers. On top of the vanilla base: pool caves (multi-lobe, half-flooded, grand and coral-reef variants), snake rivers (long, downhill, up to 3 streams per source, breach into caves only while descending), a contained deep lava sea below y=-54 with obsidian/magma rims, the vanilla ore table plus three size tiers of stone-variant patches, and 8 cave biome themes (lush, dripstone, deep dark, mushroom, ice under mountains only, amethyst, volcanic at the bottom of the world, coral in water pools) covering about half the underground with plain-rock buffer strips between zones. Implies `--fillground`. `--vanilla-caves` is accepted as a legacy alias. Usage: `arnis --output-dir ./world --bbox <bbox> --terrain --caves`.
- **Cave asset packs.** A `cave-pack/` directory next to the arnis executable (or `--cave-asset-pack <DIR>`) supplies Sponge `.schem` formations - ice spikes, dripstone columns, amethyst clusters, clay pool basins, snow piles - stamped onto cave floors and ceilings, themed by biome zone, with block-states preserved and rotated. Formations sink into the ground, clip safely against walls, and never touch fluids. Without the directory, caves still generate fully (procedural decoration only).
- **Deterministic + seam-safe.** Every cave pass is a pure function of (seed, position): adjacent tiles generated independently carve the same caves at the seam.
- **`--cave-biomes <list>` (biome mix control).** Per-theme amounts as `name=percent` pairs (`lush=150,deepdark=0,amethyst=50`): 100 = the default share, 0 = theme off, 200 ≈ double its area (the percent shifts that theme's noise threshold on a log2 curve, so growth stays smooth and the field stays a pure function of seed + position — seam-safety unaffected). Omitting the flag reproduces the default distribution byte-for-byte; depth/terrain gates (volcanic bottom-only, ice under mountains, coral in pools) always apply.
- **`--cave-zone-map <prefix>` (layout preview).** Renders the biome zone layout for `--bbox` without generating a world: `<prefix>-upper.png` (y=-20 band) + `<prefix>-deep.png` (y=-48 band), transparent where the cave is plain rock, plus a `ZONEMAP {json}` stdout line with the measured share of every theme. Uses the exact zone picker, seed and `--cave-biomes` values generation uses, so the preview IS the layout the world gets. Meld drives this for its on-map cave-biome preview.

### Fixed (cave polish)

- **Deep dark is deep-only now.** Its depth gate moved from `y <= -18` (mid-cave) to `y <= -35` (below the deepslate line), so sculk belongs to the depths like vanilla instead of bleeding into the shallow/mid caves.
- **Stone-variant patches actually appear.** Diorite/granite/andesite/dirt were placed with absolute Y ranges (0-60), which land mostly above ground in low, valley-height real-world terrain and were pruned to near-nothing (a valley region measured diorite=4). They are now placed **surface-relative** (2-72 blocks below the local surface), so they fill the rock under every column at any terrain height — measured ~26k of each variant per region (evenly balanced), the vanilla "patches of diorite/granite/andesite everywhere" look.

### Removed

- **The old experimental cave engine** (`src/cave/`, ~3,900 lines) and its flags `--carver-caves` and `--cave-pack` (the cave.json biome-pack override). The stone-to-deepslate fillground transition it hosted moved to its own module (`src/deepslate.rs`, 123 lines) and behaves identically. Why: two cave systems meant double maintenance, and the experimental one produced artifacts the new engine's verification harness would never accept.
- **`--underilla`** (building-palette remap + `underilla_mask.bin` sidecar). Why: it existed solely for the external Paper+Underilla cave pipeline, which the native `--caves` engine supersedes (~100× faster, no Java server, no restore mask).
- Dead `ore_generation` module (157 lines). Why: only the deleted engine called it; `--caves` has its own vanilla ore table.

### Stats

Measured against the 2.9.2 release commit: **31 files, +6,178 / −169 lines** in `src/` + `examples/`. Breakdown: `src/caves/` + `src/deepslate.rs` = 4,024 lines (density-field port 402, noise 265, RNG 267, carvers 330, water features 404, formation engine 714, biome decoration 820, ores 238, pipeline 461, deepslate 123); verification tooling in `examples/` ≈ 1,900 lines (cavecheck region auditor, caveshape probe, mask preview) — the harness that gated every build on zero floating fluid / zero water-lava contact / zero ore-masking errors; the rest is wiring (flag unification, direct `--output-dir` world output, Underilla removal, 72 new block definitions for the coral/ice/snow/deep-dark biome themes).

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
