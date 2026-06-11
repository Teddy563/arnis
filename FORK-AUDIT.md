# Arnis fork audit — was every change needed, or slop?

Honest review of the uncommitted Meld-seam work, for you to check before release notes.

- Base: git `0dc5678`. Diff: **13 files, +454 / −69** (after comment cleanup; was +506).
- **Dead code found: 0.** **Hallucinated/unused changes: 0.** Every change maps to a
  real code path and a real on-screen symptom.
- All changes are **gated to tile mode** (`--seed` / master-origin); single-world output
  is byte-identical to upstream louis-e.
- Gate: `fmt` 0 · `clippy -D warnings` 0 · `build` 0 · `187 tests` pass.

Confidence legend:
- **CONFIRMED** — you saw the fix work in-game.
- **SOUND** — mechanism verified in code + reasoning; contributes to a confirmed fix.
- **DEFENSIVE** — correct and cheap, but I'm not 100% sure it was the *visible* cause.

---

## The changes, grouped

### A. Trees / scatter meld  — CONFIRMED (you said "trees look fine now")
| Change | File | Why it was needed |
|---|---|---|
| Per-tile `coord_rng` in forest/wood/tree_row spawn | landuse.rs, natural.rs | Shared id-RNG was consumed in flood-fill order → border tiles disagreed. |
| Per-tile `coord_rng` for leisure + all landuse arms | leisure.rs, landuse.rs | The RNG was drawn **inside** a terrain gate → one cell drew, the other didn't → stream desynced → whole area mismatched on every seam. This was the real "all axes" cause. |
| Un-clip scatter ways/relations (`is_scatter_area_tags`, `tile_unclip_within_cap`) | osm_parser.rs, buildings.rs, floodfill.rs | A clipped polygon flood-fills a different interior per cell → membership differs at the seam. Un-clip → identical interior. |
| `tile_unclip_within_cap` shared helper | osm_parser.rs | Dedups the cap guard so way + relation paths can't drift apart. |
| `MAX_FLOOD_FILL_AREA` made `pub` | floodfill.rs | So the shared cap guard can read it. (1 line.) |

**Needed?** Yes — this is the bulk of the seam work and you confirmed trees now meld.

### B. Building seams — CONFIRMED (earlier: buildings stopped cutting / floating in water)
| Change | File | Why |
|---|---|---|
| Un-clip relation building rings (`maybe_clip_ring`) | buildings.rs | Multipolygon buildings were sealed with a wall at the cut → half a building per cell. |
| Skip buildings ≥60% over water | buildings.rs | Floor Y ignored water → the carve flooded houses → houses standing in open water. |

**Needed?** Yes — both were visible and you confirmed fixed.

### C. Biome tint seam — CONFIRMED (earlier: vertical grass/leaf colour line gone)
| Change | File | Why |
|---|---|---|
| Subtract cell origin before `cover_class` lookup (origin_x/z) | biome.rs, java.rs | It was feeding absolute coords into a cell-local grid → each tile clamped to the grid edge → wrong, different biome per cell. |
| `biome_lat_for_chunk` (per-chunk latitude from world Z) | java.rs | **DEFENSIVE** — fixes the N/S temperature-biome band so cells agree on a shared chunk's latitude. Correct + cheap, but for small cells the latitude delta is tiny, so I'm not certain it was a *visible* seam on its own. Safe to keep; could be dropped if you want to be minimal. |

### D. Terrain alignment + flat regions — CONFIRMED (terrain lines up with roads; no whole-flat cells)
| Change | File | Why |
|---|---|---|
| `compute_grid_dims` origin-anchored (+master_origin args) | elevation/mod.rs, ground.rs | The elevation/land-cover grid was sized with a different ruler (haversine avg-lat) than the world → ~0.1% stretch → terrain slid 1-3 blocks off OSM + disagreed across tiles. |
| AWS: retries 3→6, retry-missing-tiles rounds, atomic cache write | aws_terrain.rs | Under 8 parallel cells a few tiles got rate-limited/half-written → NaN → a whole cell rendered flat → staircase cliff at the seam. |
| AWS: deterministic per-tile retry jitter | aws_terrain.rs | **DEFENSIVE** — de-syncs the ~64 parallel retries. A refinement on top of the retry fix; helps but not certain it was load-bearing on its own. |

### E. Coastal vertical step — SOUND (you tested; step reduced)
| Change | File | Why |
|---|---|---|
| Skip global `filter_elevation_outliers` in tile mode | elevation/mod.rs | Its reject band is a per-cell global IQR → two tiles drop different border cells → vertical step (worst on coastal bathymetry). Bounded MAD repair already covers local spikes. |

### F. Plumbing / enabling — SOUND (no behaviour of their own; required by the above)
| Change | File | Why |
|---|---|---|
| `tile_invariant_enabled()` helper | ground_generation.rs | Single source for "are we tiling?" used by biome/tree/relation code. |
| Thread `master_origin_lat/lng` through Ground/fetch | ground.rs, elevation/mod.rs | Required to size the grid (D). |
| Regression test `test_grid_dims_match_world_extent…` | transformation.rs | **DEFENSIVE** — guards D from silently regressing. A test, not shipped logic. |

---

## Honest "could be cut" list (your call)
These are correct but the *weakest* on "was it the visible cause":
1. `biome_lat_for_chunk` (C) — keep unless you want strict minimalism.
2. AWS retry **jitter** (D) — refinement on the retry fix.
3. The regression **test** (F) — insurance, not shipped behaviour.

Everything else is load-bearing and tied to a symptom you saw and confirmed.

## Not touched (on purpose)
Roads/bridges, water carving/leveling (`level_water_surfaces`, `water_depth`), highways —
**zero** files. The coastal "road looks like a bridge" you saw is OSM-tag-driven
(`bridge`/`layer`) in upstream code, not from this work.

## Known residuals (documented, not slop — deliberately deferred)
- `natural=scrub/heath/grassland/wetland` still use the shared-stream RNG (they pass
  `&mut rng` to helpers) → not yet converted; kept clipped so they don't cascade. Convert
  next if you hit one.
- Forest/area with unclipped bbox ≥ 12.5M blocks still clips (cap guard).
- Large-lake surface/underwater-depth seams live in the do-not-touch water code.
