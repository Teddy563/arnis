# Changelog — Meld fork

All releases of the Meld fork of louis-e/arnis. Follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format.

## [1.8.5] — 2026-06-09

### Fixed
- **Tile-invariant water depth (kills the cell-border water seam).** Underwater carve depth was scaled by the per-cell `component_max` (the connected water component's max distance-transform), which sets both the shore-to-deep `span` and the max depth. That value is a global property of the water body, but each tile only measures the part it can see, so two adjacent tiles compute a different `span`/`local_max` for the same body and carve a different depth at the shared border. After the canonical-region merge butts the two halves together, that shows as a straight step in the sea/lake floor (worst on wide water, where a larger seam buffer cannot help because it still cannot measure the body's full width). Under `--seed` / `--master-origin` (i.e. external schedulers like Meld), depth is now an **absolute** function of distance-from-shore that saturates at `MAX_WATER_DEPTH` within a fixed span, with no dependence on body width. Because the gradient saturates well inside the seam buffer, two tiles always agree on distance-to-shore where it matters, so they carve identical depth at the border. Single-world generation (no `--seed`) is unchanged. Adds a regression test that the tile-invariant profile is body-width-independent.

## [1.8.4] — 2026-06-09

### Fixed
- **Cross-tile coordinate drift under `--master-origin-lat/lng`.** `transform_point` derived metres-per-degree-longitude from `avg_lat = (point_lat + origin_lat) / 2`, so the **same longitude mapped to a different block-X depending on the point's latitude** — the grid sheared like a fan instead of a rectangle. Invisible in a single world, but for external schedulers that stitch adjacent bboxes into one Minecraft world (Meld) it pushed each tile off the shared 512-block region grid by an amount that **grows with distance from the origin**, leaving flat-grass **"unrendered" strips at tile seams** that widen toward the far corners. Now anchored at `origin_lat` so a longitude maps to **one** block-X project-wide, matching the latitude axis (which already uses the constant `METERS_PER_DEG_LAT`). Plain single-world generation (no master origin) is byte-identical. Adds a latitude-invariance regression test.

## [1.8.3] — 2026-06-02

### Added
- 9 new Block IDs (256–265, u16 widening): `MAGMA_BLOCK`, `SUGAR_CANE`, `KELP`, `TALL_SEAGRASS_BOTTOM/TOP`, `SEA_PICKLE`, `BROWN_CANDLE_{2,3,4}`, `SOUL_SAND`, each with correct NBT side-table arms.
- Per-cell underwater bed picker with domain-warped noise — organic vanilla-MC-style patches (CLAY/SAND/DIRT/COARSE_DIRT) on a GRAVEL background.
- Rare 5–13 cell MAGMA + SOUL_SAND vents at depth ≥ 5 (bubble columns in MC).
- Underwater dunes: width-aware amplitude 2–4 blocks, domain-warped, prominent waves.
- SEAGRASS meadow mix (short + tall + sea pickle); KELP min-3-cell variable-height columns.
- Wetland post-pass: MOSS_BLOCK ring + COARSE_DIRT 2-ring around water puddles.
- Tiered cattail: 1–2 stalks + single `candles=1/2/3/4` BROWN_CANDLE block.
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
- `Block::id()` return type widened `u8` → `u16` (required for IDs 256–265).

---

## [2.8.1] — 2026-05-21 (Teddy563)

Previous Meld release. Voxelize / 3DMR work + Meld scheduler + tile-invariant rendering + road-detail palette improvements.

## [v2.8.0] — 2026-05-19 (louis-e upstream)

Base upstream release inherited by Meld v1.8.3. Adds the BigWaterField depth carve + initial wetland G3 + universal LC_WATER carve pass.
