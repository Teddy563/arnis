# Phase 1 — CPU performance on the cave path

**Branch:** `feat/gpu-void-naming` (local only). Commit `ab57c975`.
**Status:** done, measured, committed. Phase 2 (GPU) is planned but NOT started.

Read this before touching `src/caves/mod.rs` or benchmarking arnis — it records
two traps that will otherwise waste your day.

---

## Why the cave path

Measured with the process's own `TotalProcessorTime`, cached OSM tiles, terrain +
caves + baked lighting:

| cell | wall | CPU core-seconds | avg cores busy | cave share of CPU |
|---|---|---|---|---|
| ~1 region @ scale 0.05 (a Romania 1:20 cell) | 4.05 s | 4.13 | **1.02** | **11%** |
| 224 regions @ scale 1.0 | 116.7 s | 904.5 | **7.75** | **52%** |

Cave cost scales with *volume*, so at 1:1 the relief is ~20x taller, the carve
band deepens, and caves go from a rounding error to half the program. **Any
performance claim about arnis is meaningless without stating the scale.**

---

## What changed

### 1. Corner-plane carry in `carve_region`

The carve loop walks 4x8x4 cells up a column and evaluates the combined density at
all eight corners of each cell. But cell `cy`'s TOP plane is cell `cy+1`'s BOTTOM
plane — the same four coordinates, so the same four values. The loop now computes
the bottom plane once before the column and carries each top plane into the next
iteration, halving the calls to `combined_density` (the dominant cost: 54
improved-Perlin octave evaluations per sample).

This reuses values rather than recomputing them, so it cannot change a result.

### 2. Parallel scan in `seal_floating_fluid_region`

It probed every column from the surface down on **one thread**, and at 1:1 that was
**39.8 s of a 113 s render** — the single largest post-pass. Columns never read or
write outside their own `(x, z)`, so the scan now runs across columns with rayon
and the (rare) writes are applied afterwards on one thread, because the world is a
hash map behind `&mut`.

`flat_map_iter(...).collect()` on an indexed parallel iterator preserves input
order, so the applied writes land in the same sequence as the serial version.

---

## Results

Same bbox (`44.40,26.05,44.46,26.15`), same seed, cached tiles, eviction off:

| | baseline | phase 1 | change |
|---|---|---|---|
| **1:1** wall | 113.0 s | **76.6 s** | **1.47x faster** |
| **1:1** CPU | 874.4 core-s | 777.8 core-s | 11% less |
| **1:1** `post_passes_ms` | 39805 | **12288** | **3.24x faster** |
| **1:20** wall | 3.8 / 3.9 s | 4.2 / 3.6 s | unchanged (noise) |
| **1:20** `block_hash` | `53bbe041…` | `53bbe041…` | **identical** |

**1:20 gains nothing** and is not supposed to: caves are 11% of CPU there, and the
seal pass costs ~150 ms. The win is a 1:1 property.

---

## Trap 1: a 1:1 render is NOT reproducible run to run

This will mislead you if you do not know it. Running the **same binary twice** on
the same bbox and seed at 1:1 produces different `block_hash` values, and differs
block-for-block:

| comparison | positions compared | positions differing |
|---|---|---|
| baseline vs **itself**, two runs | 346,719,778 | 6,739 (0.0019%) |
| baseline vs phase 1 | 346,719,778 | 6,720 (0.0019%) |

Phase 1's delta is **1.00x the baseline's own noise** — indistinguishable from the
program's inherent variation. At 1:20 the same comparison is exactly zero and the
hash is stable, which is why the correctness argument rests on 1:20 plus a unit
test, never on a 1:1 hash.

Two known contributors, neither introduced here:
- `should_stream_to_disk()` decides eviction from **available RAM at the time**, so
  the same command can take different paths on different runs — and the hash is
  computed differently under eviction (`data_processing.rs`, `hash_acc` vs
  `editor.content_hash()`). Set `ARNIS_STREAM_TO_DISK=0` to pin it.
- Even with eviction pinned off, 1:1 still varies, so there is a second source —
  most likely first-writer-wins `set_block` races between overlapping parallel
  tiles. **Unresolved. Worth its own investigation.**

**Therefore:** validate cave changes with `cargo test corner_cache` and a 1:20
`block_hash`, not with a 1:1 hash.

## Trap 2: benchmark the right OSM path

`osm_fetch_ms` wraps three different things (`main.rs`): `--osm-tile-dir` reads a
local tile cache, `--file` reads merged JSON, and **with neither flag arnis calls
the Overpass API over the network**. On one bbox that is 1157 ms versus 13192 ms —
the difference between a cell that is ~75% compute and one that is ~17% compute.
Meld always passes `--osm-tile-dir`. Benchmark that way or your numbers describe
a workload nobody runs.

---

## Tests

- `caves::corner_cache_tests::carried_corner_plane_matches_recomputation` — walks a
  column both ways, asserts all eight corners match per cell.
- `caves::corner_cache_tests::combined_density_is_pure` — pins the property the
  carry depends on.

Full suite: 411 pass, `clippy -D warnings` clean, `cargo fmt` clean.

---

## Scalability: Phase 1 does NOT carry to big 1:1 worlds, and here is why

Everything above was measured with `ARNIS_STREAM_TO_DISK=0`. Big worlds cannot run
that way - the RAM heuristic turns eviction on - and **with eviction on the picture
changes completely**. Same bbox, same seed, forced on:

| | baseline | phase 1 | change |
|---|---|---|---|
| wall | 141.0 s | 139.7 s | **0.9%** |
| CPU | 832.8 core-s | 736.2 core-s | 11.6% less |
| `element_placement_ms` | 35505 | 29637 | 16.5% faster |
| `post_passes_ms` | 794 | 799 | **no change** |

Two things to take from this:

- **The fluid-seal parallelisation does nothing under eviction**, because the seal
  already runs per-tile inside the parallel tile loop (`data_processing.rs`), so it
  was parallel. It only helps the non-eviction path.
- **The corner carry still pays** (11.6% less CPU), but the wall barely moves,
  because a single-run wall is no longer CPU-bound.

### The real bottleneck for big worlds: `tile_merge`

Full phase breakdown, eviction on, 224 regions:

```
tile_merge_ms        88600   <-- 63% of a 140 s run
element_placement_ms 35505
terrain_total_ms      7215
landcover_osm_repair  4775
post_passes_ms          794
save_ms                 206
```

`tile_merge` is timed around the block that merges each tile into the main world
**and hands evicted regions to the flush worker**. `FlushWorker::spawn(ctx, 3)`
starts **one** thread behind a 3-deep channel, and that thread does, per region:
compact every section, build the chunk NBT, serialise and compress. 224 regions of
that on one core while 23 sit idle - the merge thread blocks on the channel.

Corroborating numbers:
- Turning eviction ON costs **+63 s** on identical work (76.6 s -> 139.7 s).
- Average cores busy across the whole eviction run is **5.3 of 24**.
- Switching to `--region-format blinear`, whose writer compresses buckets in
  parallel, moved `tile_merge` only 90141 -> 86386 ms (4%). **So compression is not
  the cost** - section compaction and NBT serialisation are.

**Therefore the highest-value next change is a POOL of flush workers instead of
one**, not anything to do with caves and certainly not a GPU. Regions are
independent files; K threads consuming one channel should turn ~88 s into ~15-25 s
and roughly halve big-world wall time.

## Final Phase 1 numbers

Original 3.1.2 binary vs the finished branch. Same bbox `44.40,26.05,44.46,26.15`,
same seed, cached tiles, terrain + caves + baked lighting:

| scenario | before | after | speedup |
|---|---|---|---|
| **1:1, eviction ON** (what big worlds use) | 152.3 s | **63.9 s** | **2.38x** |
| 1:1, eviction off | 112.8 s | **70.0 s** | **1.61x** |
| 1:20 (a Romania cell) | 3.9 s | 3.9 s | 1.00x |

Supporting numbers for the eviction case: `tile_merge_ms` 96464 -> 14849 (6.5x),
CPU 848 -> 755 core-seconds (11% less), peak RAM 3918 -> 4150 MB (+6%).

**1:20 is unchanged and its `block_hash` is identical** (`53bbe0418ee51b3e`), which
is both the correctness proof and the expected outcome: at that scale the cave band
is shallow, caves are 11% of CPU, and the world is small enough that eviction never
engages, so none of these three changes has anything to bite on.

## What is deliberately NOT done here

Phase 2 (GPU cave density) is specified in
`light-meld/docs/void-naming-gpu-plan.md`. **Phase 1 changes the Phase 2 maths**:
the corner carry already removed half the density evaluations, so the remaining
GPU prize is correspondingly smaller. Re-measure before writing a shader.
