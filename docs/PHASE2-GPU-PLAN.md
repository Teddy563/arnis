# Phase 2 — GPU cave density (planned, not started)

**Branch:** `feat/gpu-void-naming`. Nothing in this document is implemented.
**Prerequisite:** Phase 1 (`docs/PHASE1-CPU-PERF.md`, commit `ab57c975`) is done
and **has already taken half the prize**. Re-measure before writing a shader.

---

## What is left to win, after Phase 1

Phase 1 measured, at 1:1 on `44.40,26.05,44.46,26.15`:

| | before | after Phase 1 |
|---|---|---|
| wall | 113.0 s | **76.6 s** |
| CPU | 874.4 core-s | **777.8 core-s** |

The corner carry already removed half the `combined_density` calls, so a GPU
kernel now competes for a smaller remainder. Before Phase 1 a free cave kernel was
worth ~1.85x per cell; against the Phase 1 baseline the same kernel is worth
roughly **1.3-1.5x per cell** — and that has to be re-measured, not assumed,
because `noodle_density` (per block, never cached) and the carve loop's own
lerps/probes are now a larger fraction of what remains.

**At 1:20 the GPU is worth essentially nothing** — caves are 11% of CPU and the
whole cell is 4 s. Do not ship a GPU path for Romania-scale renders.

---

## The one workload worth porting

`caves::density::combined_density` and `noodle_density`, driven from
`caves::carve_region`. It qualifies where nothing else does:

- pure function of `(seed, x, y, z)`, no world access, no allocation, no early exit
- evaluated on a regular lattice — the shape a GPU wants
- arithmetic-dense: 54 improved-Perlin octave evaluations per corner sample
- tiny transfer: heightmap in, carve bitmask out, a few MB per region

Everything else in arnis writes into
`FnvHashMap<region> -> FnvHashMap<chunk> -> FnvHashMap<section>` with `Arc<NBT>`
side tables and first-writer-wins semantics. That is not portable to a GPU without
rewriting the world model, and there is no prior art for GPU OSM-to-voxel.

---

## The blocker that decides everything: precision

arnis's noise is **entirely `f64`** (`caves/noise.rs`: 50 mentions of `f64`, one of
`f32`). Consumer GPUs are deliberately crippled at double precision.

| | FP32 | FP64 | ratio |
|---|---|---|---|
| RTX 5080 Laptop | 24.58 TFLOPS | **384 GFLOPS** | 1:64 |
| CPU (Ultra 9 275HX) achieved, measured | — | ~76 GFLOPS | — |

**f64 on the GPU is infeasible, not merely slow.** At ~5x the CPU's achieved rate,
the pre-Phase-1 cave load (473 CPU core-seconds per 1:1 cell) needs ~95
GPU-seconds against a ~63 s wall: one GPU cannot sustain even a single worker.

**So Phase 2 means f32**, which changes every noise value. Caves shift slightly.
That is a product decision, not a bug — and it forks `tests/golden_hashes.txt`.
Keep the CPU path as the reference implementation, run golden tests on it, and
make the GPU path opt-in and documented as approximate. (C2ME-OCL, the one
comparable project, ships exactly this caveat.)

---

## Cross-vendor: NVIDIA, AMD, Apple

Use **`wgpu`** with WGSL compute shaders. It is the only option that covers all
three from one codebase:

| backend | vendor | notes |
|---|---|---|
| Vulkan / DX12 | NVIDIA, AMD, Intel on Windows/Linux | primary target |
| Metal | Apple silicon | works; unified memory removes the PCIe cost entirely |
| — | — | falls back to CPU when no adapter is found |

Do **not** use `cudarc`/CUDA: NVIDIA-only, and it requires the user to install a
toolkit. `wgpu` ships inside the exe with no user-side dependency, and shaders can
be precompiled.

Consequence worth stating plainly: **results will differ between vendors.** WGSL
f32 is IEEE-754 but fast-math flags, FMA contraction and transcendental precision
vary by driver. So the golden hashes cannot be per-backend either — the CPU stays
the single source of truth, and the GPU path is "close, not equal", everywhere.

Apple is the most favourable target of the three (unified memory, no transfer),
Intel iGPU the least (bandwidth-starved) — gate it on adapter class.

---

## Architecture: stop spawning one process per cell

This is the part that makes GPU viable at all, and it is worth doing even if the
shader is never written.

Today Meld spawns one `arnis.exe` per cell (`server.py`, `run_arnis`). A GPU
context costs 100-300 ms to create, against a 4 s cell — and **CUDA MPS, which
lets processes share a GPU concurrently, is Linux-only**. On Windows/WDDM,
concurrent processes time-slice: N processes give the throughput of one.

**Proposal: `arnis --serve`.** The process reads job specs (bbox, output dir,
settings) as JSON lines on stdin, generates each cell, writes a completion line,
and stays alive. Meld keeps a small pool of long-lived workers and dispatches cells
to them.

Wins, only one of which is the GPU:
1. GPU context created once per worker instead of once per cell
2. caches survive between cells — elevation tiles, Overture, block registry, noise
   permutation tables, the rayon pool
3. no per-cell process spawn (~50-150 ms on Windows)
4. a handful of live GPU contexts instead of thousands sequentially, so VRAM and
   WDDM scheduling stop mattering
5. one worker holding several cells can batch dispatches, which is where a GPU
   actually earns its keep

Failure isolation is preserved: a crashed worker is restarted and its cell
re-queued, exactly as today.

**Risk to audit first:** arnis global state across cells — the `DATA_VERSION`
atomic, `OnceLock` caches (e.g. `BASE_CHUNK_SECTIONS`), world-bounds statics. Any
of these leaking between jobs is a correctness bug, not a performance one.

---

## Governor: percentages, not worker counts

Meld already has `cpu_target_pct` (90), but it divides the budget by *assumed*
threads per worker. Measurement shows a 1:20 cell uses **1.02 cores** while being
allocated ~5 threads — roughly 17% real utilisation while the UI believes 90%.

Measure `cores_per_cell` at runtime from the first few cells, then derive:

```
workers = cores * cpu_target_pct/100 / measured_cores_per_cell
```

- 1:20 -> 24 x 0.9 / 1.02 ~= **21 workers** (default is 4)
- 1:1  -> 24 x 0.9 / 7.75 ~= **3 workers**

One setting, correct at both ends. Add `gpu_target_pct` as a dispatch semaphore
once a GPU path exists. **This is worth more than the GPU on the project being
rendered today** and needs no shader.

---

## Order, and confidence

| step | what | confidence |
|---|---|---|
| 2a | Occupancy-driven worker governor (Meld) | **90%** — arithmetic over a measured number |
| 2b | `arnis --serve` batch mode + worker pool | **80%** — global-state audit is the unknown |
| 2c | `wgpu` device-init + round-trip spike (one afternoon, no arnis changes) | **95%** informative |
| 2d | f32 WGSL density kernel behind `--gpu` | **65%** it lands; **55%** it beats CPU end-to-end |
| 2e | Cross-vendor validation (NVIDIA / AMD / Apple) | **60%** — expect per-vendor drift |

**Do 2a first.** It is the largest measured win in the whole programme, it helps
the render running today, and it does not depend on any of the rest.
