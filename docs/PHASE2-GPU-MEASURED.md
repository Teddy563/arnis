# Phase 2 GPU — DELIVERED, with measured results

**Status: implemented and shipped on this branch** (`5fae029d` arnis, `gpu_accel`
in Meld). The plan below was written first; delivered numbers, which landed inside
every predicted band:

| aspect | predicted | **measured** |
|---|---|---|
| 1:1 cell wall | 1.13–1.19× | **1.18× (dGPU), 1.10× (iGPU)** |
| 1:1 fleet (core-s/cell) | 1.36–1.55× | **1.37×** (852 → 623) |
| kernel parity vs CPU | <0.1% | **0.0005%** (11 / 2.26 M) |
| full-render diff vs CPU | "shifted not corrupt" | **0.0005%** (905 / 176 M) |
| vendor drift dGPU vs iGPU | "expect some" | **1 block / 176 M** |
| GPU memory | <200 MB | corner+mask buffers ≈ 10 MB/region in flight |

One surprise beyond the plan: the two runs differ by only ~5 s of wall between the
5080 and the iGPU — the GPU is idle most of the time either way. GPU *speed* was
never the constraint; the offloadable share was, exactly as the spike said.

---

# The original plan, re-costed on measurements (kept for the record)

**Branch:** `feat/gpu-void-naming`. Supersedes the estimates in `PHASE2-GPU-PLAN.md`;
that document's architecture reasoning stands, but its numbers were projections and
these are measurements. Spike source: `docs/spike-gpu/` (build with cargo, run the
exe; `ARNIS_SPIKE_ADAPTER=<substring>` picks an adapter).

Everything below was measured on the target laptop (RTX 5080 Laptop + Intel iGPU,
24-core Ultra 9 275HX) unless marked derived.

---

## Spike results

| measurement | value |
|---|---|
| wgpu adapter + device init | **123–137 ms** |
| tiny dispatch round-trip (submit → readback) | **0.19–0.66 ms** median |
| f32 Perlin throughput, **Intel iGPU** | **5.0–5.3 × 10⁹ octave-evals/s** |
| same workload, one CPU core (measured earlier) | 45.2 × 10⁶ evals/s |
| same workload, all 24 cores (ideal) | 1.09 × 10⁹ evals/s |
| **iGPU vs the whole CPU** | **4.7–4.9×** |

The kernel mirrors `combined_density`'s shape — 54 octaves per sample, integer-hash
gradients, 8 gradient dots + quintic fade + trilerp per octave — so it measures the
hardware on the right arithmetic, not a toy.

### Finding 1: the RTX 5080 is currently INVISIBLE to graphics APIs on this machine

`enumerate_adapters(Backends::all())` returns only Intel (Vulkan ×2, DX12 ×2) plus
the Microsoft software rasteriser. Both documented opt-ins were tried and did not
surface it:

- `HKCU\Software\Microsoft\DirectX\UserGpuPreferences` → `GpuPreference=2;` for the exe
- `NvOptimusEnablement` / `AmdPowerXpressRequestHighPerformance` exported from the
  exe's EXPORT table (see `docs/spike-gpu/build.rs`)

CUDA-side tooling saw the card earlier, so this is a hybrid-graphics/MUX state
(displays run through the iGPU/DisplayLink), not a missing driver. **It is fixable
from Windows graphics settings or the vendor tool, not from code.** Consequence for
the plan: any GPU feature MUST treat the dGPU as optional-at-runtime, pick adapters
by explicit enumeration (never `PowerPreference` — it chose the iGPU even when asked
for high performance), and fall back to CPU without ceremony.

### Finding 2: GPU speed is not the constraint — the offloadable share is

Even the iGPU beats the whole CPU by ~5× on the exact kernel shape. A visible 5080
would multiply that by roughly 5–10, and it would barely change the end-to-end
numbers, because the ceiling is how much of the program the kernel replaces.

### Finding 3: per-cell context cost is affordable — `--serve` demoted

Init is ~130 ms against a 62 s cell (0.2%), and a dispatch round-trip is
sub-millisecond. The earlier plan made a persistent worker pool a prerequisite on a
100–300 ms init assumption; it is now merely a nice-to-have.

---

## What is left to offload (measured on the Phase-1 binary)

224-region 1:1 cell, cached tiles, terrain + baked lighting:

| | caves on | caves off | cave delta |
|---|---|---|---|
| eviction ON: wall / CPU | 62.0 s / 809.7 core-s | 42.8 s / 379.1 | **19.2 s / 430.6 core-s** |
| eviction off: wall / CPU | 84.5 s / 941.7 | 43.8 s / 427.0 | 40.7 s / 514.7 |

Caves are **~53% of a 1:1 cell's CPU even after the corner carry**. Of that, the
noise fields (`combined_density` + `noodle_density`) are roughly half to two-thirds
(derived from the pre-carry 2/3 share, halved by the carry, plus uncached noodle) —
call it **215–290 core-seconds of GPU-able demand per 1:1 cell**.

At the measured iGPU rate that demand is **~2 GPU-seconds per cell**. Seven workers
sharing the one visible GPU ≈ 20–25% GPU utilisation. On a visible 5080: under 5%.

---

## Estimated gains (measured inputs, derived arithmetic)

| aspect | today (Phase 1) | with GPU noise kernel | gain | confidence |
|---|---|---|---|---|
| 1:1 cell wall, eviction on | 62.0 s | ~52–55 s | **1.13–1.19×** | 70% |
| 1:1 fleet throughput (CPU-bound) | 809.7 core-s/cell | ~520–595 | **1.36–1.55×** | 60% |
| 1:20 cell (Romania) | 3.9 s | ~3.9 s | **~1.0×** | 95% |
| GPU utilisation, 7×1:1 workers | — | 20–25% (iGPU), <5% (5080) | — | 75% |
| VRAM / memory | — | **< 200 MB** (few MB per in-flight region; iGPU shares system RAM) | — | 90% |

For contrast, already banked without any GPU: Phase 1 = **2.38×** on the same cell,
and the worker governor ≈ **5×** on the 1:20 project. The GPU's remaining prize is
real but third in line, exactly as the original plan predicted.

---

## The plan

| step | what | confidence it lands |
|---|---|---|
| 2a | `--gpu` flag + adapter enumeration with CPU fallback (the spike's selection logic, hardened) | 90% |
| 2b | Port `combined_density` (54 octaves) to WGSL f32, batch per tile-column, carve mask readback | 75% |
| 2c | Add `noodle_density` to the same dispatch (evaluated only where combined stays solid → do it unconditionally on GPU, it is cheap there) | 70% |
| 2d | Validation: CPU-vs-GPU carve-mask diff bounded (< 0.1% of cells, shifted-not-corrupt), golden gate stays CPU-only, GPU documented approximate | 80% |
| 2e | Meld toggle (`gpu_accel`, following `native_region_format`'s 8-step recipe), gated on an adapter actually enumerating | 90% |
| 2f | Optional: persistent worker (`--serve`) to amortise init — now an optimisation, not a prerequisite | 80% |

**Blocked on the user for full validation:** making the 5080 visible (Windows
Settings → Display → Graphics → add the exe → High performance; or the vendor MUX
switch), then re-running `docs/spike-gpu`. Until then every number above is the
iGPU's — which is already sufficient.

**Hard constraints carried over from the original plan, all still true:**
- **f32 only.** f64 at 384 GFLOPS on the 5080 cannot sustain one worker.
- Results differ slightly per vendor/driver → CPU stays the reference
  implementation; no per-backend golden hashes.
- Windows has no MPS: N processes time-slice the GPU. At ~2 GPU-s per 62 s cell the
  time-slicing is immaterial, which the original plan could not yet know.

**Overall: ~65% confidence the full stack ships and delivers ≥1.3× fleet throughput
on 1:1 cave renders; ~95% it delivers nothing at 1:20 (do not enable it there).**
