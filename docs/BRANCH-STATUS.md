# Branch status — `feat/gpu-void-naming`

**Start here.** This is the index for the branch: what shipped, what it measured,
what is still open, and the traps that will cost you a day if you do not know them.

**Local only. Nothing is pushed. Nothing is on `main`.**

| repo | branch | commits ahead of `origin/main` |
|---|---|---|
| arnis fork (`arnis-283-src`) | `feat/gpu-void-naming` | 11 (incl. the 3.1.2 release) |
| Meld (`light-meld`) | `feat/gpu-void-naming` | 12 (incl. the 1.9.3 release) |

Companion documents:
- `docs/PHASE1-CPU-PERF.md` — the performance work in detail, with both traps
- `docs/PHASE2-GPU-PLAN.md` — the GPU plan, re-costed after Phase 1
- `light-meld/docs/void-naming-gpu-plan.md` — void worlds, GUI naming, GPU research
- `light-meld/docs/native-blinear-generation.md` / `-results.md` — the B_Linear work

---

## DONE — performance (Phase 1)

Four changes, all in the arnis fork. Full detail in `PHASE1-CPU-PERF.md`.

| # | change | commit |
|---|---|---|
| 1 | Corner-plane carry in `carve_region` — halves `combined_density` calls | `ab57c975` |
| 2 | Parallel scan in `seal_floating_fluid_region` | `ab57c975` |
| 3 | **Flush regions on a pool instead of one thread** | `418273ad` |
| 4 | Parallel scan in `sweep_floating_veg` | `aced72a7` |

**#3 was the big one.** Everything else is a rounding error next to it.

### Measured — original 3.1.2 vs the finished branch

`44.40,26.05,44.46,26.15`, seed 11, cached tiles, terrain + caves + baked lighting:

| scenario | before | after | speedup |
|---|---|---|---|
| **1:1, eviction ON** (what big worlds use) | 152.3 s | **63.9 s** | **2.38x** |
| 1:1, eviction off | 112.8 s | **70.0 s** | **1.61x** |
| 1:20 (a Romania cell) | 3.9 s | 3.9 s | 1.00x |

Supporting: `tile_merge_ms` 96464 -> 14849 (6.5x). CPU 848 -> 755 core-seconds.
Peak RAM 3918 -> 4150 MB (+6%).

Flush-pool tuning (`ARNIS_FLUSH_THREADS`, default `cores/4` clamped 2..6):

| threads | wall | `tile_merge_ms` | peak RAM |
|---|---|---|---|
| 1 (old behaviour) | 145.2 s | 96330 | 4125 MB |
| 6 (default) | 65.6 s | 15050 | 4208 MB |
| 12 | 57.1 s | 6175 | 4284 MB |

The default is deliberately modest, not core-count: **Meld runs several arnis
processes at once and each spawns its own pool.** Meld should set this per worker.

### The most valuable outcome: eviction stopped being a penalty

| | eviction off | eviction on |
|---|---|---|
| before Phase 1 | 112.8 s | **152.3 s** (35% *slower*) |
| after Phase 1 | 70.0 s | **63.9 s** (9% faster) |

Streaming used to cost 35% wall time for its memory saving, because all writing
went through one thread and the merge thread blocked behind it. Writing now
overlaps generation, so eviction is the faster path *and* the lean one: **10,120 MB
-> 4,150 MB peak.** That flip matters more than the raw 2.38x.

### Correctness

- **1:20 `block_hash` identical** before and after: `53bbe0418ee51b3e`, four runs.
- **1:1** cannot be checked by hash (see Trap 1). Block-for-block instead:

| comparison | positions | differing |
|---|---|---|
| baseline vs **itself** (noise floor) | 346,719,778 | 6,739 (0.0019%) |
| baseline vs Phase 1 | 346,719,778 | 6,720 (0.0019%) |
| 1 flush thread vs 12 | 346,710,821 | 6,744 (0.0019%) |

Every change sits exactly on the noise floor.

- `cargo test` 411 pass, `clippy -D warnings` clean, `cargo fmt` clean.
- Two new unit tests pin the corner carry (`caves::corner_cache_tests`).

### Peak RAM, explained (measured)

| configuration, 1:1, eviction off | peak RAM |
|---|---|
| caves + fillground | 10,120 MB |
| fillground only | 6,296 MB |
| no caves, no fill | 6,236 MB |

~6.2 GB is the resident world: 229,376 chunks x 24 sections, of which only **3.7
per chunk are non-uniform** (a dense 4096-entry array, 4 KB as `Full`/u8 or 8 KB as
`FullWide`/u16). The other 20 are `Uniform` — one block value — which is why solid
underground is nearly free. ~3.9 GB is cave bookkeeping: carving turns cheap
`Uniform(STONE)` sections into dense arrays, and the carve pass holds a
`Vec<(i32,i32,i32)>` of every carved coordinate plus a `HashSet<i64>` of every
cave-air cell.

---

## DONE — features (shipped in the releases on this branch)

- **Native B_Linear region output** (`--region-format blinear`), arnis 3.1.2 +
  Meld 1.9.3. Verified on a live Leaf 1.21.11 boot. See `native-blinear-results.md`.
- Meld UI: region-format picker, animated wordmark, drawer ordering fix.

---

## DONE — the four planned items (this session)

| item | commit | evidence |
|---|---|---|
| **World naming**, CLI + GUI | `917e1ee7` | 8 tests; `--level-name "Bucharest 1:1"` lands verbatim in level.dat while the folder becomes `Bucharest 1_1`, output dir untouched |
| **Void worlds** (`--void`) | `27fa504d` | 4 tests; booted on Leaf 1.21.11 -- **1,089 server chunks, 0 non-air blocks**; built region keeps 31,504 blocks of content and none of the 260,300 grass / 13,104 bedrock a normal world has |
| **Worker governor** | Meld `e13242e` | 17 tests; sampled a real cell at **1.06 cores**, matching the 1.02 measured externally; suggests 21 workers at 1:20 against a stored default of 4 |
| **1:1 nondeterminism — FIXED** | `b401d1d8` | three identical hashes where every run previously differed; also stable under eviction |

### The nondeterminism, since it was open for a while

Bisected rather than guessed. Without `--caves` a render was already deterministic
at one thread and at twenty-four; with `--caves` it changed every run. The cave
passes keep their working sets in **std `HashSet`, whose hasher is seeded randomly
per process**, and those sets are iterated to apply world edits -- despeckle, prune,
fluid support, decoration. Where edits interact the write ORDER decides the result,
so iteration order was part of the world and changed on every launch.

`FnvHashSet` hashes deterministically, and the rest of the world model already used
Fnv for this reason. Before: `1201d024…`, `fa509e70…`. After: `937c27f3…` three
times, `e1b16998…` twice under eviction.

**Consequence:** cave layout shifts slightly versus older worlds, because a fixed
order replaces a random one. Nothing without `--caves` changes.

**This also unblocks Trap 1** below: a 1:1 hash is now a valid way to validate a
change, which it was not before.

---

## NOT DONE — ordered by value

### 1. Turn the worker governor ON (`worker_autoscale`) once you trust it

Meld's `cpu_target_pct` (90) divides the budget by *assumed* threads per worker. A
1:20 cell actually uses **1.02 cores** while being allocated ~5 threads, so the box
runs at roughly **17% utilisation while the UI believes 90%**.

Measure `cores_per_cell` at runtime, then `workers = cores * pct/100 /
cores_per_cell`: **~21 workers at 1:20** (default is 4), **~3 at 1:1**. One
setting, correct at both ends. Worth roughly **5x on the Romania 1:20 render**,
which is far more than any GPU work. Confidence 90%.

RAM cross-check: a 1:20 cell peaks ~1.2 GB, so ~20 fit in 33.8 GB — CPU and RAM run
out at about the same point. At 1:1 a cell needs ~4.15 GB with eviction, so RAM
binds first at ~7 workers.

### 2. (was void worlds — DONE, see above)

`light-meld/docs/void-naming-gpu-plan.md` has the full plan. The mechanism is
already verified end to end: patching the bundled `level.dat` to vanilla's own
`the_void` preset and booting Leaf 1.21.11 produced **808 server-generated chunks
with zero non-air blocks**, biome `minecraft:the_void`.

Write vanilla's exact form — one air layer, not an empty list:
```json
{ "biome": "minecraft:the_void", "features": true, "lakes": false,
  "layers": [ { "block": "minecraft:air", "height": 1 } ],
  "structure_overrides": [] }
```
Remaining work: a `--void` flag, gating ground generation, making the base-chunk
pass emit air, skipping the `region.template` seed, refusing `--void` with
`--caves`, and fixing spawn (the template spawns the player at Y=-61 over nothing).

**The trap is `merge.py`'s drift guard**, not finalcheck: a cell whose content does
not reach both far edges raises `MeldCoordinateDriftError`, which `server.py`
classifies as deterministic — so it is **never retried and the cell is silently
lost**. A void cell over sea or forest fails outright. Fix that before shipping.

Void pairs naturally with B_Linear: an all-air region is 4.2 MB in Anvil but ~8 KB
in `.b_linear`.

### 3. (was GUI naming — DONE, see above)

arnis has no naming input anywhere, and Java is broken twice over: both branches
hard-code `None`, and a name would be dropped anyway because `WorldEditor` stores
it in a field only the Bedrock writer reads. Needs `--level-name` plus a GUI field,
sanitised for the folder but raw in the NBT. **Meld needs no change** — it already
names worlds end to end. Latent bug found while reading: `gui.rs:356` computes
`30 - base_name.len() - 2`, a usize underflow that becomes reachable the moment a
user can type a long name.

### 4. Phase 2, the GPU — planned, deliberately last

See `PHASE2-GPU-PLAN.md`. Phase 1 shrank the prize twice over: the corner carry
removed half the density calls, and `tile_merge` — which no shader can touch — went
from 96 s to 15 s. A perfect cave kernel now takes a 1:1 run from 63.9 s to roughly
48-50 s (**~1.3x**), needs **f32** (the 5080 Laptop does 384 GFLOPS f64 against
24.58 TFLOPS f32, and f64 cannot sustain even one worker), forks the golden hashes,
and requires `arnis --serve` so contexts amortise. VRAM is a non-issue: **under
200 MB**.

### 5. Smaller items

- **Meld should set `ARNIS_FLUSH_THREADS`** per worker from its own CPU budget,
  instead of letting every process pick `cores/4` independently.
- **Cave memory**: the carve `Vec` + `HashSet` are ~3.9 GB at 1:1. A bitmask over
  the carve band would cut that a lot and allow more parallel 1:1 workers.
- **`--map-item-only` on a b_linear world** fails with "world has no saved regions".
  Correct, but a clearer message would help.
- **Root `meld/` package tools** (`mca.py`, `chunk_protection.py`, `subworld.py`)
  still glob `*.mca` only and would silently no-op on a b_linear world. Outside the
  light-meld run path; `metadata.json` now carries `regionContainer` as the hook.

---

## Two traps that will waste your day

**Trap 1 — do not validate at 1:1 with a hash.** See item 2 above. Validate cave
changes with `cargo test corner_cache` plus a **1:20** `block_hash`, which is
stable and exact.

**Trap 2 — benchmark the OSM path Meld actually uses.** `osm_fetch_ms` wraps three
different things. With no source flag arnis calls **Overpass over the network**;
with `--osm-tile-dir` it reads a local cache. On one bbox that is **13192 ms vs
1157 ms** — the difference between a cell that is ~17% compute and one that is
~75% compute. Meld always passes `--osm-tile-dir`.

Also: pin `ARNIS_STREAM_TO_DISK` when benchmarking, or the run picks its path from
whatever RAM happens to be free and your numbers will not be comparable.
