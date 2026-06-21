# Arnis fork 2.9.2 - release scope (prep notes)

Local planning doc. Not committed to main. No co-authoring. Implementation is gated
on explicit approval. Theme of 2.9.2: small-scale water realism.

Status legend: PLANNED / DESIGN-LOCKED / IN-PROGRESS / DONE.

---

## Item 1 - Scale-aware underwater depth + bed (DESIGN-LOCKED, impl pending)

### Problem
`water_depth.rs` carves river/pool depth from a chamfer distance transform, but the
model never reads `scale`. The shoal is a fixed 3 blocks and the slope is gentle, both
in block units. At small scale (scale < 0.5) a river is only a few blocks wide, falls
entirely inside the shoal, and renders FLAT (no depth, no bed, no banks). Confirmed:
`compute_big_water_field` and `estimate_max_carve_depth` take no scale today.

### Design (hardened by adversarial audit, run wl0nxaccz)
The audit REJECTED the first smoothstep idea (it produced 2-3 block vertical cliffs at
W14/0.1, W7+W10/0.5, W5+W7/1.0) and a `shoal = 3*scale` term that swallowed narrow
rivers. Replaced with a LINEAR unit-step bowl, gated strictly to `scale < 0.5`. The
`scale >= 0.5` path stays byte-identical to today (existing tier model untouched).

Depth (baked per cell inside `compute_big_water_field` / `ocean_depth_for_cell`):

```
shoal_blocks = 1.0                                   // constant for all scale<0.5
d_blocks     = dt_units / 3.0                        // baked chamfer DT, NOT a block index
hw_blocks    = component_max_units / 3.0             // each body's own half-width / radius
run          = max(0.0, hw_blocks - shoal_blocks)
target_max   = clamp(min(round(hw_blocks*0.7), floor(run)), 0, 5)   // floor(run) is the anti-cliff
steps        = floor(d_blocks - shoal_blocks)        // 0 at shoal edge, +1 per inward block
depth        = clamp(steps, 0, target_max)           // linear unit-step, no smoothstep
```

The `floor(run)` cap on `target_max` is load-bearing: it guarantees the center is
reachable in unit steps, so the bowl can never climb more than 1 block per horizontal
block. Cap 5 (vs today's 6) keeps it under `MAX_WATER_DEPTH` and under the carve floor
clamp. Big lakes and oceans get a wide gentle bowl to 5; tiny streams get a 1-block dip.
Same formula, keyed off each body's own width.

Verified cross-sections (real chamfer DT), worst-jump = 1, no flats, for scale {0.1, 0.2, 0.3, 0.49}:

```
W=3  -> 0,1,0
W=5  -> 0,1,2,1,0
W=7  -> 0,1,2,3,2,1,0
W=10 -> 0,1,2,3,4,4,3,2,1,0
W=14 -> 0,1,2,3,4,5,5,5,5,4,3,2,1,0
W=20 -> wide flat bottom capped at 5
```

### Bed / bottom visual (the "bottom looks better" ask)
All inside the `scale < 0.5` branch; every multiplier uses `scale.min(1.0)` so the
`scale >= 0.5` path is character-for-character unchanged.

- Bed-palette domain-warp: `warp_amp = (24.0 * scale.min(1.0)).max(4.0)` (was fixed 24).
  Palette feature sizes `(36.0*scale).round().max(3.0)` (and 40/42/26/30 likewise, floor 3).
  Kills the salt-and-pepper bed; patches stay coherent within a narrow river.
- Dune domain-warp: `warp_amp = (30.0 * scale.min(1.0)).max(5.0)` (was fixed 30).
- Contour wobble (`ocean_depth_for_cell`): amplitude `* (4.0 * scale.min(1.0))`, wavelength
  `(12.0/scale.min(1.0)).round().max(12.0)`. Removes the +/-1 depth flicker on thin beds.
- Seagrass gate: `veg_min = if scale < 0.5 { 2 } else { 3 }`. Keep kelp at depth>=4 (do NOT lower).
- Bed micro-relief (breaks the flat-slab bottom on depth 1-2 bodies): gate
  `scale<0.5 && depth>=2 && !near_bridge && dune_bump==0`; `micro = noise>0.62 ? 1 : 0`;
  lower `bed_y` by `micro` AND extend the water fill to `0..=(depth+micro)`, keep bed above
  `MIN_Y+1`. This is the single highest-risk edit (water-fill bounds).

### World height (consistency)
`estimate_max_carve_depth` (drives the reserved world floor at ground.rs:119-121) gets
`scale` and, at `scale < 0.5`, returns the bowl cap (5) whenever any water exists. This is
a true upper bound, so `water_floor` never under-reserves. Cap 5 < today's 6, so it
reserves equal-or-less, never clips bedrock.

### Signatures to change (3 fns gain a trailing `scale: f64`)
1. `compute_big_water_field(ground, xzbbox, scale)` [water_depth.rs:113]; caller data_processing.rs:299 (args.scale in scope). Threads to `ocean_depth_for_cell(.., scale)`.
2. `estimate_max_carve_depth(lc_grid, world_w, world_h, scale)` [water_depth.rs:305]; caller ground.rs:120 (scale in scope); tests at :661,:667 pass 1.0.
3. `carve_water_column(.., scale)` [water_depth.rs:339] (scale used ONLY for bed visuals; depth stays baked). Plumb through `carve_lc_water_region` [:587] (data_processing.rs:545), `carve_lc_water_pass` [:569] (data_processing.rs:760), and the water_areas.rs chain (`generate_water_areas` -> `generate_water_area_from_way` / `_from_relation`, callers data_processing.rs:102,196). Tests at :646,:647,:652 pass 1.0.

`depth_from_dt` / `polygon_local_max` / today's `ocean_depth_for_cell` math: KEPT, used only by the `scale >= 0.5` path. Add a new bowl helper used only when `scale < 0.5`.

### Safety
- Gated `scale < 0.5`: normal maps and 1:1 are byte-identical. Prove with a scale=1.0
  region diff vs current HEAD.
- Tile-invariant: depth baked per cell in the BFS, carve is vertical-only, jitter is pure
  `value_noise_01(x,z,..)` / `coord_hash(x,z)`. No neighbour reads.
- World floor threaded, cap 5 -> reserves equal-or-less, no bedrock clip.
- Local-only, reversible.

### Confidence (audit)
depth-math 88, bed-visual 86, consistency 88, skeptic 82. Depth/bowl math is >90% and
should land first try. The two edits most likely to need a second tuning render: the bed
micro-relief WATER-fill bounds, and proving the scale>=0.5 byte-identity diff.

### Next steps
1. Implement behind the `scale < 0.5` gate (one helper shared by carve + estimate so they cannot drift).
2. fmt / clippy / test green; add unit tests (cross-section shape, cap 5, no >1 step, scale=1.0 unchanged).
3. Test renders at 1:5 / 1:7 / 1:10 (rivers + a lake) plus a 1:1 byte-identity render.
4. Deploy rebuilt arnis.exe to light-meld (local) for in-game eyeball. Tune.

---

## Item 2 - shore "foul line" follow-up (PLANNED, optional)
The SAND/SANDSTONE waterline rim in `ground_generation.rs` also thins at small scale.
Same scale-aware treatment can widen it. Do after Item 1 lands and looks right.

---

## Not part of 2.9.2 (tracked elsewhere)
- Water-wedge clip fix lives on branch `fix/water-ring-wedge` as an UPSTREAM PR to
  louis-e/arnis. If accepted it ships in upstream, not as a Meld-only change. Keep separate.

---

## Release checklist (when we cut 2.9.2)
- [ ] Item 1 implemented, green, renders approved.
- [ ] scale=1.0 byte-identity diff vs HEAD proven.
- [ ] Bump Cargo.toml version to 2.9.2.
- [ ] Rebuild arnis.exe, deploy to light-meld.
- [ ] CHANGELOG note.

Constraints: no commits to main, no co-authoring, local-only until approved.
