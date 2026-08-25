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

---

# Branch status — `perf/speed-to-worldgen` (arnis fork)

Added 2026-08-25. **Local only, uncommitted working tree.** Nothing pushed,
nothing on `main`.

**Full write-up lives Meld-side:**
`meld-triagefix/docs/generation-performance.md` — the governor, the protocol, the
settings, the bench, the determinism rules, and the open question. This section is
the arnis half of the index.

The theme is different from Phase 1 above. Phase 1 made **one cell** faster (2.38x
at 1:1, from the flush pool). This branch is about how many cells Meld should run
**at once**, so the arnis work here is almost entirely *instrumentation and
hardening*: the generator has to be able to report what a cell really costs, and has
to stop being quietly non-deterministic under load, so Meld's governor can measure
instead of assume.

## What landed

### 1. Meld stdout protocol v1 — `src/meld_telemetry.rs` (new)

Machine-readable phase markers, **opt-in via `ARNIS_PHASE_MARKERS=1`**. Unset, both
entry points return before any clock read or formatting, so a default run is
byte-identical to a build without the module.

```text
[meld] v=1 phase=<name> t=<ms_since_process_start>
[meld] v=1 phase=done wall_s=<f.3> cpu_s=<f.3> peak_mb=<f.1> gpu_ms=<u64>
```

Markers fire at the **start** of a phase, so a duration is `t(next) - t(this)` and
the last one is `wall_s*1000 - t(last)`. Instrumented boundaries:

| phase | site |
|---|---|
| `fetch` | `main.rs:467`, before the OSM source read |
| `elevation` | `main.rs:489`, before `ground::generate_ground_data` |
| `parse` | `main.rs:494`, before `osm_parser::parse_osm_data` |
| `overture` | `main.rs:523`, inside the gate, so it only fires when the fetch runs |
| `place` | `data_processing.rs:579`, both parallel and sequential paths |
| `merge` | `data_processing.rs:938`, parallel path only |
| `ground` | `data_processing.rs:1051` |
| `post` | `data_processing.rs:1082` |
| `save` | `data_processing.rs:1184`, immediately before `editor.save()` |
| `done` | `main.rs:729`, end of the Ok arm, so `wall_s` covers the whole run |

**No new dependency.** `Cargo.toml` already pins `windows = "0.62.0"` but only with
`Win32_System_Console`, and Cargo.toml was out of scope, so a private `mod win`
hand-rolls `#[link(name = "kernel32")]` bindings for `GetCurrentProcess`,
`GetProcessTimes` and `K32GetProcessMemoryInfo` (kernel32 is linked by default on
both toolchains). `cpu_s` = (kernel+user FILETIME)/1e7, `peak_mb` =
`PeakWorkingSetSize`/1 MiB. Non-Windows returns `-1.000` / `-1.0` in the same float
shape, and Meld's psutil sampler covers that case.

Caveat for whoever reads the numbers: on the parallel path, placement and per-batch
merges **interleave**, so `place` to `merge` is the combined loop and `merge` to
`ground` is a short teardown. The `bench` output keeps the true `element_placement` /
`tile_merge` split; v1 has no field for it.

### 2. Deterministic fill budget — `src/floodfill.rs`

`--timeout` was enforced with `Instant::now()` **inside the fill loop**, which makes
output a function of machine load and worker count rather than of input. That is
exactly the wrong property for a program Meld runs twelve copies of at once: two runs
of the same cell on a busier box could truncate different polygons.

`ARNIS_FILL_BUDGET=1` (accepts `1|true|yes|on`) re-expresses the same limit in work
units: `budget_units = timeout_secs * BUDGET_UNITS_PER_SECOND`, the constant being
`2_000_000`. A work unit is one `polygon.contains` decision plus its bitmap/queue
bookkeeping. **2 M/s deliberately takes the pessimistic end** of a reasoned
2-4 M/s/core range: erring low means the budget can only bind *sooner* than the wall
clock would on an idle machine, never later, so budget mode can never let a
previously-truncated polygon run away. In practice it does not bind at all, since
Meld passes `--timeout 600..1200`, i.e. 1.2-2.4 billion units against a worst case
near 125 M. The per-unit cost figure is reasoned from the code's structure, not from
an instrumented micro-benchmark.

The check **cadence** is unchanged (`filled_area.len() % 100` on the optimized path,
once per seed candidate on the original), the legacy `Instant::now()` is still taken
at the same line, and the scanline path is untouched because it never read the
timeout.

**Default path proven identical**: `scripts/golden_hash.sh` OK on all 5 fixtures, and
the same 5 re-run live with `--timeout 600` gave identical hashes with the env var set
and unset.

Inherited limitation, not new: the budget is only consulted between seed fronts, so
one enormous connected component still floods to completion once seeded.

### 3. Process-global config asserts

Four setters now panic with a named message on a **conflicting** re-set, instead of
silently keeping the first value or the last:

| static | setter |
|---|---|
| `WORLD_BOUNDS` | `src/world_editor/common.rs:40` |
| `DATA_VERSION` | `src/world_editor/java.rs:38` |
| `NOISE_SEED` | `src/ground_generation.rs:1778` |
| `BIOME_AMOUNTS` | `src/caves/decoration.rs:163` |

**No assert can fire today.** Every call site runs once per process, audited:
`data_processing.rs:321/336/338`, `main.rs:264` (the `--cave-zone-map` preview path,
which exits before world creation) and `caves/mod.rs:114`. They exist to make the
one-cell-per-process assumption explicit and loud: the day someone adds a `--serve`
mode or an in-process batch (the natural next step once Meld is paying process-spawn
cost twelve times a minute), they get a named panic instead of blocks clamped to the
wrong world height or chunks stamped with mismatched DataVersions. Signatures and
call sites are unchanged; `BiomeAmounts` gained `Debug, PartialEq`, derive-only.

## Verification

- `cargo check --all-targets`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt -- --check`: clean.
- `cargo test`: **486 passed, 0 failed** (2 telemetry, 6 new floodfill).
- `scripts/golden_hash.sh` on a release build: **OK on all 5 fixtures** against the
  committed pre-change baseline (levittown `82d4196da33dd23c`, marrakech
  `9b7d1bede9fa0ea0`, midtown `368810a05a6ceea0`, munich_altstadt `55fd390be3b95991`,
  rovaniemi `e7ffec6d815ace39`).
- Markers live-checked on both the sequential and the 18-tile parallel path; the CPU
  counters are real (79.6 CPU-s over 12.1 wall-s on 21 threads). With the env var
  unset and with `=0`: zero `[meld]` lines, and a `Compare-Object` of marker-run
  stdout (markers stripped) against unset-run stdout differs only in the output path.

## Not done

- **The `overture` marker has never fired in a live run.** It sits inside the exact
  `if` that gates the fetch, so it is correct by construction, but both smokes were
  `--mode terrain-only --offline`.
- **Only a debug build was smoke-tested for markers**; no release marker run.
- **Nothing sets `ARNIS_FILL_BUDGET`.** Flipping it on is Meld's call.
- **No `#[should_panic]` test** for the four asserts: the statics are process-global
  and already set by other tests in the same binary, so tripping one would poison the
  shared state for the rest of the suite.
- **No Cargo.toml edit.** If the raw kernel32 FFI in `meld_telemetry.rs` is ever
  unwanted, the alternative is adding `Win32_System_Threading` /
  `Win32_System_ProcessStatus` to the existing `windows` pin and rewriting `mod win`.

## The open question, and it is the valuable one

**Nobody has named the 1:1 shared-resource wall.**

Measured on the reference box (24 logical cores, 8P+16E Ultra 9 275HX, 31.4 GB, NVMe),
1:1 cell size 4, workers raised live mid-run: median cell time goes 21.1 s (8w) to
33.9 s (12w) to 41.6 s (16w) to 56.5 s (20w) to 63.5 s (24w) while **throughput stays
flat at 21-23 cells/min the whole way**, at 79% average CPU. There was CPU headroom,
and adding workers still bought nothing. Something else is saturated.

Candidates, in rough order of suspicion:

1. **Memory bandwidth and last-level cache.** Each 1:1 cell holds a multi-GB world
   model: measured previously at ~6.2 GB resident world plus ~3.9 GB cave bookkeeping
   without eviction, ~4.15 GB with. Twelve of those streaming through a shared LLC is
   the obvious suspect.
2. **NVMe write pressure** during region flush, now that flushing is a pool and
   overlaps generation instead of serialising behind it.
3. **The Windows heap under concurrent NBT allocation.** A known past offender in this
   codebase: the B_Linear converter was 10-30x slow for exactly this reason and
   mimalloc fixed it. arnis does its own dense-section allocation under rayon.
4. **P-core versus E-core placement** past 8 workers. The measured taper does start
   there, which is why the governor's marginal-gain threshold drops above 8.

Meld's governor **routes around this wall by measuring it. It does not explain it.**
Whoever names it gets the next real speedup, and the instruments now exist: run
`bench/bench_scheduler.py` over a worker sweep with `ARNIS_PHASE_MARKERS=1` and see
which phase inflates. If it is `place`, suspect 1 or 3. If it is `save` or `merge`,
suspect 2.

**The GPU is not the answer to this.** See `PHASE2-GPU-MEASURED.md`: the dGPU and the
iGPU finished within about 5 s of each other, so GPU speed was never the constraint,
the offloadable share was.
