//! Per-cell water depth carving from a chamfer-3-4 distance transform over the LC_WATER mask.

use crate::block_definitions::{
    Block, AIR, BLACK_CONCRETE, BLUE_FLOWER, BROWN_CANDLE, BROWN_CANDLE_2, BROWN_CANDLE_3,
    BROWN_CANDLE_4, CLAY, COARSE_DIRT, CYAN_TERRACOTTA, DEAD_BUSH, DIRT, FERN, GRASS, GRAVEL,
    GRAY_CONCRETE, GRAY_CONCRETE_POWDER, KELP, KELP_PLANT, LARGE_FERN_LOWER, LARGE_FERN_UPPER,
    LIGHT_GRAY_CONCRETE, MAGMA_BLOCK, RED_FLOWER, SAND, SEAGRASS, SEA_PICKLE, SOUL_SAND, STONE,
    SUGAR_CANE, TALL_GRASS_BOTTOM, TALL_GRASS_TOP, TALL_SEAGRASS_BOTTOM, TALL_SEAGRASS_TOP, WATER,
    WHITE_CONCRETE, WHITE_FLOWER, YELLOW_FLOWER,
};
use crate::block_definitions::{
    ACACIA_LEAVES, ACACIA_LOG, AZALEA_LEAVES, BIRCH_LEAVES, BIRCH_LOG, CHERRY_LEAVES, CHERRY_LOG,
    DARK_OAK_LEAVES, DARK_OAK_LOG, JUNGLE_LEAVES, JUNGLE_LOG, MANGROVE_LEAVES, MANGROVE_LOG,
    OAK_LEAVES, OAK_LOG, SPRUCE_LEAVES, SPRUCE_LOG,
};

/// Tree logs + leaves. The water carve never overwrites these, so a bank tree's canopy that
/// overhangs the water is kept whole instead of being sliced flat at the waterline.
const TREE_PROTECT: &[Block] = &[
    OAK_LOG,
    BIRCH_LOG,
    SPRUCE_LOG,
    DARK_OAK_LOG,
    JUNGLE_LOG,
    ACACIA_LOG,
    CHERRY_LOG,
    MANGROVE_LOG,
    OAK_LEAVES,
    BIRCH_LEAVES,
    SPRUCE_LEAVES,
    DARK_OAK_LEAVES,
    JUNGLE_LEAVES,
    ACACIA_LEAVES,
    CHERRY_LEAVES,
    MANGROVE_LEAVES,
    AZALEA_LEAVES,
];
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::floodfill_cache::RoadMaskBitmap;
use crate::ground::Ground;
use crate::land_cover::LC_WATER;
use crate::world_editor::{WorldEditor, MIN_Y};

/// Flat shoal width in chamfer-DT units; slope only starts past it.
const SHOAL_DT_UNITS: u16 = 9;

/// DT saturation cap. Depth clamps to <=6 long before this.
const DT_MAX: u8 = u8::MAX;

/// Maximum water carve depth, in blocks (the deepest tier).
const MAX_WATER_DEPTH: i32 = 6;

/// Cap on water sub-rect cells (bounds memory, keeps u32 indices valid); ~1000 km².
const MAX_WATER_FIELD_CELLS: usize = 1_000_000_000;

// Meld additions: puddle threshold + dirt-blob bed overlay + dunes.

/// Component DT threshold below which a water body is treated as a puddle
/// and stays surface-only (no carve). Matches SHOAL_DT_UNITS — a body that
/// doesn't even reach past the shoal ring shouldn't carve depth.
const PUDDLE_DT_THRESHOLD: u8 = 9;

/// Component cell-count threshold below which a body stays surface-only.
/// A 5x5 cell body (~25 m^2 at ESA 10 m/cell) is a puddle by any sane def.
const PUDDLE_CELL_THRESHOLD: usize = 25;

/// Scale below which the per-body bowl model replaces the tiered/sqrt depth, so that
/// narrow, few-block-wide rivers and pools still carve instead of rendering flat.
/// At or above this scale the original model runs unchanged (byte-identical).
const SMALL_SCALE_THRESHOLD: f64 = 0.5;

/// Small-scale carve cap, in blocks. The bowl tops out here; kept under
/// `MAX_WATER_DEPTH` so the carve clamp and debug-assert always hold. Also the cap for the
/// line-waterway channel bowl (`element_processing::waterways`).
pub const SMALL_SCALE_MAX_DEPTH: i32 = 5;

#[inline]
fn nibble_get(buf: &[u8], i: usize) -> u8 {
    let byte = buf[i >> 1];
    if i & 1 == 0 {
        byte & 0x0F
    } else {
        byte >> 4
    }
}

#[inline]
fn nibble_set(buf: &mut [u8], i: usize, v: u8) {
    debug_assert!(v < 16, "nibble value {v} out of range");
    let byte = &mut buf[i >> 1];
    if i & 1 == 0 {
        *byte = (*byte & 0xF0) | (v & 0x0F);
    } else {
        *byte = (*byte & 0x0F) | ((v & 0x0F) << 4);
    }
}

#[inline]
fn bit_get(b: &[u64], i: usize) -> bool {
    (b[i >> 6] >> (i & 63)) & 1 == 1
}

#[inline]
fn bit_set(b: &mut [u64], i: usize) {
    b[i >> 6] |= 1u64 << (i & 63);
}

/// Baked per-cell carve depth (0..=6), nibble-packed over the water sub-rect.
pub struct BigWaterField {
    depth: Vec<u8>,
    width: usize,
    height: usize,
    min_x: i32,
    min_z: i32,
}

impl BigWaterField {
    fn empty() -> Self {
        Self {
            depth: Vec::new(),
            width: 0,
            height: 0,
            min_x: 0,
            min_z: 0,
        }
    }

    #[inline]
    fn local_idx(&self, x: i32, z: i32) -> Option<usize> {
        let lx = i64::from(x) - i64::from(self.min_x);
        let lz = i64::from(z) - i64::from(self.min_z);
        if lx < 0 || lz < 0 || lx as usize >= self.width || lz as usize >= self.height {
            return None;
        }
        Some(lz as usize * self.width + lx as usize)
    }

    /// Carve depth at the cell; 0 outside the water sub-rect.
    #[inline]
    pub fn depth_at(&self, x: i32, z: i32) -> i32 {
        match self.local_idx(x, z) {
            Some(i) => i32::from(nibble_get(&self.depth, i)),
            None => 0,
        }
    }
}

/// Map a grid-cell span to the block span that samples into it (a safe superset).
pub(crate) fn grid_span_to_block_span(
    g_lo: usize,
    g_hi: usize,
    world_dim: usize,
    grid_dim: usize,
) -> (i32, i32) {
    if grid_dim <= 1 || world_dim <= 1 {
        return (0, world_dim.saturating_sub(1) as i32);
    }
    let f = (world_dim - 1) as f64 / (grid_dim - 1) as f64;
    let lo = ((g_lo as f64 - 0.5) * f).floor() as i32 - 1;
    let hi = ((g_hi as f64 + 0.5) * f).ceil() as i32 + 1;
    (lo.max(0), hi.min(world_dim as i32 - 1))
}

/// Run a chamfer DT over the LC_WATER mask and bake per-cell carve depth.
pub fn compute_big_water_field(ground: &Ground, xzbbox: &XZBBox, scale: f64) -> BigWaterField {
    let min_x = xzbbox.min_x();
    let max_x = xzbbox.max_x();
    let min_z = xzbbox.min_z();
    let max_z = xzbbox.max_z();

    // Water bbox from the small land-cover grid, avoiding a full-world scan.
    let (wmin_x, wmin_z, wmax_x, wmax_z) = match ground.lc_water_block_bounds() {
        Some((lx, lz, hx, hz)) => (min_x + lx, min_z + lz, min_x + hx, min_z + hz),
        None => return BigWaterField::empty(),
    };

    let is_lc_water =
        |x: i32, z: i32| ground.cover_class(XZPoint::new(x - min_x, z - min_z)) == LC_WATER;

    // Pad by one for the shore ring (clamped); keeps the DT identical to full-bbox.
    let smin_x = (wmin_x - 1).max(min_x);
    let smax_x = (wmax_x + 1).min(max_x);
    let smin_z = (wmin_z - 1).max(min_z);
    let smax_z = (wmax_z + 1).min(max_z);
    let sw = (smax_x - smin_x + 1) as usize;
    let sh = (smax_z - smin_z + 1) as usize;
    // Cap cells to bound memory; above it, render flat water instead of crashing.
    let total = match sw.checked_mul(sh) {
        Some(t) if t <= MAX_WATER_FIELD_CELLS => t,
        _ => {
            eprintln!("Warning: water area too large for depth carving; rendering flat water");
            return BigWaterField::empty();
        }
    };

    // Seed DT: water = DT_MAX (unreached), shore/land = 0.
    let mut dt = vec![0u8; total];
    for z in smin_z..=smax_z {
        let row = (z - smin_z) as usize * sw;
        for x in smin_x..=smax_x {
            if is_lc_water(x, z) {
                dt[row + (x - smin_x) as usize] = DT_MAX;
            }
        }
    }
    chamfer_3_4_dt(&mut dt, sw, sh);

    // Per-component BFS for the max DT, then bake each cell's depth.
    let mut depth = vec![0u8; total.div_ceil(2)];
    let mut visited = vec![0u64; total.div_ceil(64)];
    let mut comp: Vec<u32> = Vec::new();
    // Small-scale maps shrink real rivers/lakes to a few blocks; relax the puddle
    // gate so narrow bodies still carve instead of being dropped as puddles.
    let (puddle_dt, puddle_cells) = if scale < SMALL_SCALE_THRESHOLD {
        (3u8, 8usize)
    } else {
        (PUDDLE_DT_THRESHOLD, PUDDLE_CELL_THRESHOLD)
    };
    for start in 0..total {
        if dt[start] == 0 || bit_get(&visited, start) {
            continue;
        }
        comp.clear();
        comp.push(start as u32);
        bit_set(&mut visited, start);
        let mut comp_max = 0u8;
        let mut head = 0;
        while head < comp.len() {
            let idx = comp[head] as usize;
            head += 1;
            comp_max = comp_max.max(dt[idx]);
            let i = idx % sw;
            let j = idx / sw;
            let mut visit = |n: usize, comp: &mut Vec<u32>| {
                if dt[n] != 0 && !bit_get(&visited, n) {
                    bit_set(&mut visited, n);
                    comp.push(n as u32);
                }
            };
            if i > 0 {
                visit(idx - 1, &mut comp);
            }
            if i + 1 < sw {
                visit(idx + 1, &mut comp);
            }
            if j > 0 {
                visit(idx - sw, &mut comp);
            }
            if j + 1 < sh {
                visit(idx + sw, &mut comp);
            }
        }
        // Meld puddle threshold: components whose peak DT is below the
        // shoal-band reach OR whose cell count is tiny stay surface-only
        // (depth nibbles stay 0; carve_water_column paints the WATER+SAND
        // shoal stack with no carve below water_y).
        if comp_max < puddle_dt || comp.len() < puddle_cells {
            continue;
        }
        let cm = u16::from(comp_max);
        for &c in &comp {
            let idx = c as usize;
            let x = smin_x + (idx % sw) as i32;
            let z = smin_z + (idx / sw) as i32;
            let d = ocean_depth_for_cell(x, z, u16::from(dt[idx]), cm, scale);
            nibble_set(&mut depth, idx, d as u8);
        }
    }

    BigWaterField {
        depth,
        width: sw,
        height: sh,
        min_x: smin_x,
        min_z: smin_z,
    }
}

/// In-place two-sweep chamfer-3-4 DT. Input: 0 = shore, `DT_MAX` = water seed.
fn chamfer_3_4_dt(d: &mut [u8], w: usize, h: usize) {
    let step = |v: u8, add: u8| v.saturating_add(add);
    for j in 0..h {
        for i in 0..w {
            let idx = j * w + i;
            if d[idx] == 0 {
                continue;
            }
            let mut best = d[idx];
            if i > 0 {
                best = best.min(step(d[idx - 1], 3));
            }
            if j > 0 {
                best = best.min(step(d[idx - w], 3));
                if i > 0 {
                    best = best.min(step(d[idx - w - 1], 4));
                }
                if i + 1 < w {
                    best = best.min(step(d[idx - w + 1], 4));
                }
            }
            d[idx] = best;
        }
    }
    for j in (0..h).rev() {
        for i in (0..w).rev() {
            let idx = j * w + i;
            if d[idx] == 0 {
                continue;
            }
            let mut best = d[idx];
            if i + 1 < w {
                best = best.min(step(d[idx + 1], 3));
            }
            if j + 1 < h {
                best = best.min(step(d[idx + w], 3));
                if i > 0 {
                    best = best.min(step(d[idx + w - 1], 4));
                }
                if i + 1 < w {
                    best = best.min(step(d[idx + w + 1], 4));
                }
            }
            d[idx] = best;
        }
    }
}

/// Width-tiered max carve depth. Bigger bodies reach a deeper floor.
#[inline]
fn polygon_local_max(component_max_units: u16) -> i32 {
    if component_max_units < 21 {
        2
    } else if component_max_units < 45 {
        3
    } else if component_max_units < 75 {
        4
    } else {
        6
    }
}

/// Depth from an effective DT and component max.
///
/// v2.8.8 — SQRT curve (rounded, replaces v2.8.7 tier-bands which were
/// visibly staircased in MC). Steep near shore (first cells reach a real
/// depth fast), smooth plateau far out (large central areas converge to
/// `local_max`). Symmetric per body-width via `span` lookup.
///
/// depth = local_max * sqrt(dist / span), clamped to 0..local_max.
fn depth_from_dt(dt_eff: f64, component_max_units: u16) -> i32 {
    if dt_eff < f64::from(SHOAL_DT_UNITS) {
        return 0;
    }
    let local_max = polygon_local_max(component_max_units);
    let dist_blocks = (dt_eff - f64::from(SHOAL_DT_UNITS)) / 3.0;
    let span: f64 = if component_max_units < 21 {
        6.0
    } else if component_max_units < 45 {
        12.0
    } else if component_max_units < 75 {
        20.0
    } else {
        35.0
    };
    let t = (dist_blocks / span).clamp(0.0, 1.0);
    let depth_f = (local_max as f64) * t.sqrt();
    (depth_f.floor() as i32).clamp(0, local_max)
}

/// Per-cell carve depth, with deterministic contour wobble on the bank lines.
fn ocean_depth_for_cell(
    x: i32,
    z: i32,
    dt_units: u16,
    component_max_units: u16,
    scale: f64,
) -> i32 {
    if dt_units == 0 {
        return 0;
    }
    if scale < SMALL_SCALE_THRESHOLD {
        return bowl_depth_small_scale(x, z, dt_units, component_max_units, scale);
    }
    let wobble = (crate::ground_generation::value_noise_01(x, z, 12) - 0.5) * 4.0;
    depth_from_dt(f64::from(dt_units) + wobble, component_max_units)
}

/// Linear unit-step "bowl" step count from a (block) distance-from-shore and a body
/// half-width, both in blocks. Depth rises one block per inward block past the shoal,
/// and `target_max` is capped by the horizontal run so the bank can never need to
/// climb more than one block per block (no cliffs). Capped at `SMALL_SCALE_MAX_DEPTH`.
fn bowl_steps(d_blocks: f64, hw_blocks: f64) -> i32 {
    const SHOAL_BLOCKS: f64 = 1.0;
    let run = (hw_blocks - SHOAL_BLOCKS).max(0.0);
    let target_max =
        (((hw_blocks * 0.7).round()).min(run.floor()) as i32).clamp(0, SMALL_SCALE_MAX_DEPTH);
    let steps = (d_blocks - SHOAL_BLOCKS).floor() as i32;
    steps.clamp(0, target_max)
}

/// Small-scale (`scale` < `SMALL_SCALE_THRESHOLD`) per-cell carve depth: a linear
/// unit-step bowl keyed to each water body's own half-width, so narrow rivers and
/// pools carve a sloped channel instead of rendering flat. The bank meander wobble is
/// scaled down so it never dominates a body that is only a few blocks wide.
fn bowl_depth_small_scale(
    x: i32,
    z: i32,
    dt_units: u16,
    component_max_units: u16,
    scale: f64,
) -> i32 {
    let s = scale.min(1.0);
    let wl = (12.0 / s).round().max(12.0) as i32;
    let wobble = (crate::ground_generation::value_noise_01(x, z, wl) - 0.5) * (4.0 * s);
    let d_blocks = (f64::from(dt_units) + wobble) / 3.0;
    let hw_blocks = f64::from(component_max_units) / 3.0;
    bowl_steps(d_blocks, hw_blocks)
}

/// Safe upper bound on a map's deepest carve, from the pre-repair water mask.
pub fn estimate_max_carve_depth(
    lc_grid: &[Vec<u8>],
    world_width: usize,
    world_height: usize,
    scale: f64,
) -> i32 {
    let gh = lc_grid.len();
    let gw = lc_grid.first().map_or(0, Vec::len);
    if gw == 0 || gh == 0 {
        return 0;
    }
    let mut dt = vec![0u8; gw * gh];
    let mut any_water = false;
    for (z, row) in lc_grid.iter().enumerate() {
        for (x, &c) in row.iter().enumerate() {
            if c == LC_WATER {
                dt[z * gw + x] = DT_MAX;
                any_water = true;
            }
        }
    }
    if !any_water {
        return 0;
    }
    // Small-scale uses the bowl model (cap SMALL_SCALE_MAX_DEPTH); return that as the
    // upper bound so the reserved world floor always covers the deepest carve.
    if scale < SMALL_SCALE_THRESHOLD {
        return SMALL_SCALE_MAX_DEPTH;
    }
    chamfer_3_4_dt(&mut dt, gw, gh);
    let grid_max_dt = dt.iter().copied().max().unwrap_or(0);
    // Inclusive-span ratio, matching the grid<->block mapping elsewhere (safe upper bound).
    let wr = world_width.saturating_sub(1).max(1) as f64 / gw.saturating_sub(1).max(1) as f64;
    let hr = world_height.saturating_sub(1).max(1) as f64 / gh.saturating_sub(1).max(1) as f64;
    let block_max_dt = f64::from(grid_max_dt) * wr.max(hr).max(1.0);
    let comp_max = block_max_dt.min(f64::from(u16::MAX)) as u16;
    depth_from_dt(block_max_dt + 2.0, comp_max)
}

/// Place the underwater stack: WATER column, then a SAND/GRAVEL bed over SANDSTONE/STONE.
/// Suppresses the Meld blob/dune/vegetation overlays when
/// the cell is adjacent to a bridge/road. Plain WATER + GRAVEL bed under
/// causeways so the bridge "shadow" doesn't get textured with DIRT strips.
pub fn carve_water_column_with_flags(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    water_y: i32,
    depth: i32,
    near_bridge: bool,
    body_max: i32,
) {
    debug_assert!(
        depth <= MAX_WATER_DEPTH,
        "water carve depth {depth} exceeds the max tier {MAX_WATER_DEPTH}"
    );
    // Clamp to the valid tier range, and keep the bed above bedrock.
    let depth = depth
        .clamp(0, MAX_WATER_DEPTH)
        .min((water_y - MIN_Y - 2).max(0));
    for dy in 0..=depth {
        // Blacklist tree logs/leaves so the carve never slices an overhanging bank-tree canopy
        // (it still overwrites terrain to carve the channel/lake).
        editor.set_block_absolute(WATER, x, water_y - dy, z, None, Some(TREE_PROTECT));
    }
    let bed_y = water_y - depth - 1;

    // Meld v2.8.3 multi-layer wobbly blob bed. Three noise layers stack on
    // top of upstream's SAND/GRAVEL depth-drift base, decorrelated by
    // distinct (offset_x, offset_z) so blobs don't lock in phase.
    //
    //   Layer A — big COARSE_DIRT blobs, scale 28, ~20% coverage
    //   Layer B — medium DIRT blobs,     scale 16, ~30% coverage
    //   Layer C — SAND pockets,          scale 22, ~12% coverage (reinjects
    //             SAND into deep tiers so the floor isn't a uniform GRAVEL plane)
    //   Layer D — upstream SAND vs GRAVEL drift (scale 6, depth-tiered)
    //
    // Shoal (d=0/1) still always SAND (the doc 05 mossy accents come in
    // a separate match arm).
    // v2.8.6 — bed palette via blob noise + STONE bedrock under everything.
    //
    // F1 (doc 01): SAND blob restricted to depth<=3 only — no deep SAND
    //              terraces carving through the GRAVEL ocean floor.
    // F3 (doc 03): each palette block has its own coarse blob noise field
    //              (scale 14-22) so patches are 5-10 cells wide, not pepper.
    // F8 (doc 08): under_block is STONE for every sea cell (vanilla bedrock
    //              under bed). Shore-band SANDSTONE moved to ground_generation.
    let (top_block, under_block) = match depth {
        0..=1 => (SAND, STONE),
        2..=6 if near_bridge => {
            // Under-bridge zone: plain bed, no blobs, no veg.
            if crate::ground_generation::value_noise_01(x, z, 6) < 0.4 && depth <= 3 {
                (SAND, STONE)
            } else {
                (GRAVEL, STONE)
            }
        }
        2..=6 => {
            // v2.8.10 — vanilla-MC bed pattern: INDEPENDENT per-block noise
            // fields with DIFFERENT scales so each block type forms its own
            // patch size (small SAND specks 3-5 cells, medium DIRT patches
            // 8-12 cells, larger CLAY/COARSE blobs 12-18 cells, tiny MAGMA
            // pockets 2-3 cells). No hierarchy — each block independently
            // tested against its threshold, FIRST positive wins by depth
            // priority, else GRAVEL.
            //
            // Depth-tier jitter via a coarse value-noise (NOT per-block coord_hash) so it breaks
            // concentric depth rings as COHERENT patches instead of per-block static speckle.
            let jn = crate::ground_generation::value_noise_01(x + 7, z + 13, 22);
            let jitter = if jn < 0.34 {
                -1
            } else if jn > 0.66 {
                1
            } else {
                0
            };
            let d = (depth + jitter).max(1);

            // Per-block independent noise — different seeds + scales.
            // Scale ≈ patch radius in blocks.
            // v2.8.10 iter5 — RARE BIG patches per user spec.
            //   * SOUL_SAND + MAGMA: 5-13 cell clusters, spread apart, RARE
            //   * SAND: BIGGER patches (scale 36, ~6% coverage)
            //   * CLAY: BIGGER patches (scale 42, ~5%) — 2nd most common per user
            //   * DIRT: medium patches (scale 26, ~4%)
            //   * COARSE_DIRT: medium patches (scale 30, ~3%)
            //   * GRAVEL: ~80% background (vanilla MC ocean floor)
            //
            // Domain-warp displaces sampling coords → patches are organic
            // (long rounded AND bubble shapes), not circles.
            let warp_x = crate::ground_generation::value_noise_01(x + 901, z + 33, 52);
            let warp_z = crate::ground_generation::value_noise_01(x + 17, z + 811, 52);
            let wx = x + ((warp_x - 0.5) * 28.0) as i32;
            let wz = z + ((warp_z - 0.5) * 28.0) as i32;

            // Bigger patch periods so dirt/clay/gravel form coherent vanilla-style blobs.
            let n_sand = crate::ground_generation::value_noise_01(wx + 53, wz + 97, 56);
            let n_clay = crate::ground_generation::value_noise_01(wx + 73, wz + 109, 64);
            let n_dirt = crate::ground_generation::value_noise_01(wx + 211, wz + 41, 44);
            let n_coarse = crate::ground_generation::value_noise_01(wx + 311, wz + 17, 50);
            // 5-13 cell clusters: scale 8 → cluster radius ~3-6 cells.
            // Very tight threshold 0.96 → very few sites pass → spread apart.
            let n_magma = crate::ground_generation::value_noise_01(wx + 401, wz + 503, 8);
            let n_soul = crate::ground_generation::value_noise_01(wx + 727, wz + 911, 8);

            let top = if d <= 1 {
                SAND
            } else if d == 2 {
                // Shallow shore-band: SAND dominant near shore, transitions
                // to GRAVEL outward. No DIRT here.
                if n_sand > 0.50 {
                    SAND
                } else {
                    GRAVEL
                }
            } else if d >= 5 && n_magma > 0.96 {
                MAGMA_BLOCK
            } else if d >= 5 && n_soul > 0.96 {
                SOUL_SAND
            } else if n_clay > 0.74 {
                CLAY
            } else if n_sand > 0.81 {
                SAND
            } else if n_dirt > 0.88 {
                DIRT
            } else if n_coarse > 0.90 {
                COARSE_DIRT
            } else {
                GRAVEL
            };
            (top, STONE)
        }
        _ => (GRAVEL, STONE),
    };

    if bed_y > MIN_Y {
        editor.set_block_absolute(top_block, x, bed_y, z, None, Some(&[]));
    }
    if bed_y - 1 > MIN_Y {
        editor.set_block_absolute(under_block, x, bed_y - 1, z, None, Some(&[]));
    }
    // v2.8.7 F2 — fill STONE from bed_y-2 down 12 cells so neighbour-column
    // side faces never expose AIR pockets (image 2 noise-hole artefact).
    // skip_existing=true means we only paint AIR — pre-existing terrain stays.
    let fill_to = (bed_y - 2).max(MIN_Y + 1);
    let fill_from = (bed_y - 12).max(MIN_Y + 1);
    if fill_from <= fill_to {
        editor.fill_column_absolute(STONE, x, z, fill_from, fill_to, true);
    }

    // v2.8.7 F8 — dunes fire at depth>=1 (was >=2). Shallow shore-band dunes
    // give the bed natural undulation even where the water is just 1 block deep.
    if depth >= 1 && !near_bridge {
        place_underwater_dunes(editor, x, z, water_y, bed_y, depth, body_max, top_block);
    }

    // v2.8.4 — sparse underwater vegetation in deep water far from bridges.
    if depth >= 3 && !near_bridge {
        place_underwater_vegetation(editor, x, z, water_y, bed_y, depth, body_max);
    }
}

/// v2.8.10 — Compute the dune bump height at (x, z). Mirrors the calc
/// inside `place_underwater_dunes` exactly so vegetation can plant ABOVE
/// the dune top instead of inside it (which dug holes in the bed when
/// veg paint was AIR-gated).
fn dune_bump_at(x: i32, z: i32, depth: i32, body_max: i32) -> i32 {
    let target_amp: i32 = match body_max {
        0..=3 => 2,
        4..=6 => 3,
        _ => 4,
    };
    let amp_cap = (depth - 1).max(0);
    let amp = target_amp.min(amp_cap);
    if amp <= 0 {
        return 0;
    }
    let warp_x = crate::ground_generation::value_noise_01(x + 901, z + 33, 50);
    let warp_z = crate::ground_generation::value_noise_01(x + 17, z + 811, 50);
    let wx = x + ((warp_x - 0.5) * 30.0) as i32;
    let wz = z + ((warp_z - 0.5) * 30.0) as i32;
    let n_large = crate::ground_generation::value_noise_01(wx + 113, wz + 257, 44) as f32;
    let n_med = crate::ground_generation::value_noise_01(wx + 31, wz + 71, 18) as f32;
    let n_sharp = crate::ground_generation::value_noise_01(wx + 7, wz + 11, 10) as f32;
    let h_f = 0.40 * n_large + 0.30 * n_med + 0.30 * n_sharp;
    if h_f < 0.28 {
        return 0;
    }
    let t = (h_f - 0.28) / 0.72;
    let bump = (t.powf(0.45) * (amp as f32 + 0.99)).floor() as i32;
    if bump <= 0 {
        return 0;
    }
    bump.clamp(1, amp)
}

/// v2.8.5 — Clumped underwater vegetation. Two decorrelated noise fields
/// (`field_noise` scale 30 + `cluster_noise` scale 10) gate SEAGRASS meadows;
/// rarer KELP_PLANT columns (capped by KELP tip at the top) only fire where
/// BOTH noises are high and `depth >= 5`.
///
/// User feedback v2.8.5: "patches of seagrass should be like bigger clumps
/// and better like random patches with the tall kelp rarer".
fn place_underwater_vegetation(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    water_y: i32,
    bed_y: i32,
    depth: i32,
    body_max: i32,
) {
    // v2.8.10 — true bed top = bed_y + dune bump for this cell. Veg plants
    // ABOVE the dune (avoids carving "holes" where veg cells lack dune blocks).
    let bump = dune_bump_at(x, z, depth, body_max);
    let bed_top = bed_y + bump;
    // Big patches (scale 30) carve broad meadows; cluster (scale 10) carves
    // the clumpy interior. Multiply them so SEAGRASS lands in 8-20-cell
    // patches with empty gravel between, instead of pepper-shot single cells.
    let field_noise = crate::ground_generation::value_noise_01(x + 53, z + 89, 30);
    let cluster_noise = crate::ground_generation::value_noise_01(x + 401, z + 17, 10);
    let combined = field_noise * cluster_noise;

    // KELP — only deep cells where BOTH noises ring high. v2.8.5 spec:
    // "the tall kelp rarer". Gates tightened (was 0.70/0.70 → 0.78/0.80)
    // plus 25% per-cell dropout. KELP tip on top of column.
    let kelp_pick = (crate::land_cover::coord_hash(x + 91, z + 41) % 100) as i32;
    // v2.8.7 F4 — kelp gate depth>=4 (was 6) so shallow kelp forests fire.
    // Min 3 cells per column ("lowest 3 tall"), variance 30..=100% of avail.
    if depth >= 4 && field_noise > 0.78 && cluster_noise > 0.80 && kelp_pick < 25 {
        let plant_top_full = water_y - 1;
        let plant_bottom = bed_top + 1;
        let avail = plant_top_full - plant_bottom;
        if avail >= 3 {
            let hgt_pick = (crate::land_cover::coord_hash(x + 211, z + 503) % 100) as i32;
            let share = 30 + hgt_pick * 70 / 100;
            let used = ((avail as i64 * share as i64) / 100) as i32;
            let used = used.max(3).min(avail);
            let plant_top = plant_bottom + used;
            // v2.8.8 F4 — veg paints ONLY into AIR. Empty `Some(&[])`
            // was a whitelist that matched nothing → fell through to
            // ALWAYS REPLACE → overwrote dune blocks → visible holes
            // in bed surface. AIR-only whitelist fixes this.
            for y in plant_bottom..plant_top {
                editor.set_block_absolute(KELP_PLANT, x, y, z, None, Some(&[AIR]));
            }
            editor.set_block_absolute(KELP, x, plant_top, z, None, Some(&[AIR]));
        }
        return;
    }

    // v2.8.7 F3 — seagrass meadow mix:
    //   50% short SEAGRASS
    //   35% TALL_SEAGRASS (2-block) when there's room
    //   15% SEA_PICKLE (waterlogged pickle stack)
    // v2.8.8 F4 — paint into AIR only (no dune overwrite).
    if combined > 0.42 {
        let dropout = crate::land_cover::coord_hash(x + 17, z + 31) % 10;
        if dropout < 4 {
            let plant_y = bed_top + 1;
            if plant_y < water_y {
                let pick = (crate::land_cover::coord_hash(x + 211, z + 73) % 100) as i32;
                if pick < 50 {
                    editor.set_block_absolute(SEAGRASS, x, plant_y, z, None, Some(&[AIR]));
                } else if pick < 85 && plant_y + 1 < water_y {
                    editor.set_block_absolute(
                        TALL_SEAGRASS_BOTTOM,
                        x,
                        plant_y,
                        z,
                        None,
                        Some(&[AIR]),
                    );
                    editor.set_block_absolute(
                        TALL_SEAGRASS_TOP,
                        x,
                        plant_y + 1,
                        z,
                        None,
                        Some(&[AIR]),
                    );
                } else {
                    editor.set_block_absolute(SEA_PICKLE, x, plant_y, z, None, Some(&[AIR]));
                }
            }
        }
    }
}

/// v2.8.6 — Width-aware multi-octave dunes 1-4 blocks tall on top of bed.
///
/// `body_max` is the BWF-derived max depth of the water body (7x7 sample
/// from caller). Wider bodies (DT_max >= 8) get full 3-4 cell amplitude
/// for visible vanilla-MC-style rolling waves; narrow rivers stay flat.
#[allow(clippy::too_many_arguments)]
fn place_underwater_dunes(
    editor: &mut WorldEditor,
    x: i32,
    z: i32,
    water_y: i32,
    bed_y: i32,
    depth: i32,
    body_max: i32,
    bed_block: Block,
) {
    // v2.8.10 iter4 — taller more-prominent dunes. Coverage gate lowered
    // 0.38→0.28 (more cells have a dune), amp 3→4 max, sharper power 0.45
    // (rapid rise to max in high-noise cells), domain-warped sampling for
    // organic non-circular dune ridges.
    let target_amp: i32 = match body_max {
        0..=3 => 2,
        4..=6 => 3,
        _ => 4,
    };
    let amp_cap = (depth - 1).max(0);
    let amp = target_amp.min(amp_cap);
    if amp <= 0 {
        return;
    }

    // Domain-warped sampling so dune ridges curve organically.
    let warp_x = crate::ground_generation::value_noise_01(x + 901, z + 33, 50);
    let warp_z = crate::ground_generation::value_noise_01(x + 17, z + 811, 50);
    let wx = x + ((warp_x - 0.5) * 30.0) as i32;
    let wz = z + ((warp_z - 0.5) * 30.0) as i32;

    let n_large = crate::ground_generation::value_noise_01(wx + 113, wz + 257, 44) as f32;
    let n_med = crate::ground_generation::value_noise_01(wx + 31, wz + 71, 18) as f32;
    let n_sharp = crate::ground_generation::value_noise_01(wx + 7, wz + 11, 10) as f32;
    let h_f = 0.40 * n_large + 0.30 * n_med + 0.30 * n_sharp;

    if h_f < 0.28 {
        return;
    }
    let t = (h_f - 0.28) / 0.72;
    let bump = (t.powf(0.45) * (amp as f32 + 0.99)).floor() as i32;
    if bump <= 0 {
        return;
    }
    let bump = bump.clamp(1, amp);

    for dy in 1..=bump {
        let y = bed_y + dy;
        if y >= water_y {
            break;
        }
        editor.set_block_absolute(bed_block, x, y, z, None, Some(&[]));
    }
}

/// Post-pass carving every LC_WATER cell, so ESA-only water gets depth too.
pub fn carve_lc_water_pass(
    editor: &mut WorldEditor,
    ground: &Ground,
    xzbbox: &XZBBox,
    bwf: &BigWaterField,
    road_mask: &RoadMaskBitmap,
    tunnel_footprint: &RoadMaskBitmap,
) {
    let x1 = bwf.min_x + bwf.width as i32 - 1;
    let z1 = bwf.min_z + bwf.height as i32 - 1;
    carve_lc_water_region(
        editor, ground, xzbbox, bwf, road_mask, tunnel_footprint, bwf.min_x, x1, bwf.min_z, z1,
    );
}

/// `carve_lc_water_pass` restricted to the inclusive `[iter_min..=iter_max]` block
/// range (intersected with the water sub-rect). Per-tile callers pass strict tile
/// bounds; writes are vertical-only so output is identical regardless of tiling.
#[allow(clippy::too_many_arguments)]
pub fn carve_lc_water_region(
    editor: &mut WorldEditor,
    ground: &Ground,
    xzbbox: &XZBBox,
    bwf: &BigWaterField,
    road_mask: &RoadMaskBitmap,
    tunnel_footprint: &RoadMaskBitmap,
    iter_min_x: i32,
    iter_max_x: i32,
    iter_min_z: i32,
    iter_max_z: i32,
) {
    let off_x = xzbbox.min_x();
    let off_z = xzbbox.min_z();
    // Only the water sub-rect can hold LC_WATER cells; intersect it with the range.
    let x0 = bwf.min_x.max(iter_min_x);
    let x1 = (bwf.min_x + bwf.width as i32 - 1).min(iter_max_x);
    let z0 = bwf.min_z.max(iter_min_z);
    let z1 = (bwf.min_z + bwf.height as i32 - 1).min(iter_max_z);
    for z in z0..=z1 {
        for x in x0..=x1 {
            // Keep road/bridge surfaces (causeways, decks) and tunnel bores dry.
            if road_mask.contains(x, z) || tunnel_footprint.contains(x, z) {
                continue;
            }
            let coord = XZPoint::new(x - off_x, z - off_z);
            if ground.cover_class(coord) != LC_WATER {
                continue;
            }
            let water_y = ground.water_level(coord);
            // Skip land bumps an over-claiming water polygon sits above.
            if editor.get_ground_level(x, z) > water_y {
                continue;
            }
            // v2.8.5 — widened from 4-cardinal to 5x5 ring (24 cells).
            // Catches diagonal + 1-cell-out neighbours of bridge decks so
            // the "shadow" zone under causeways stays clean GRAVEL/SAND
            // with no DIRT strips peeking out at the bridge edges.
            let near_bridge = (-2..=2).any(|dx: i32| {
                (-2..=2).any(|dz: i32| !(dx == 0 && dz == 0) && road_mask.contains(x + dx, z + dz))
            });
            // v2.8.6 F2 — body_max via 7x7 BWF sample (proxy for body width).
            let mut body_max = 0;
            for dx in -3..=3 {
                for dz in -3..=3 {
                    let d = bwf.depth_at(x + dx, z + dz);
                    if d > body_max {
                        body_max = d;
                    }
                }
            }
            carve_water_column_with_flags(
                editor,
                x,
                z,
                water_y,
                bwf.depth_at(x, z),
                near_bridge,
                body_max,
            );
        }
    }
}

/// v2.8.10 F10 — sweep floating veg (cattail, grass, candles, flowers,
/// sugar_cane) placed BEFORE water/road overlays. Scans every bbox cell:
/// if ground-level block is WATER or the cell is in road_mask, AIR-out
/// any veg cells y=1..=5 above it.
pub fn sweep_floating_veg(editor: &mut WorldEditor, xzbbox: &XZBBox, road_mask: &RoadMaskBitmap) {
    sweep_floating_veg_region(
        editor,
        road_mask,
        xzbbox.min_x(),
        xzbbox.max_x(),
        xzbbox.min_z(),
        xzbbox.max_z(),
    );
}

/// Region variant of [`sweep_floating_veg`] over strict bounds
/// `[g_min_x..=g_max_x] x [g_min_z..=g_max_z]`. Used per-tile (inside the
/// parallel tile closure) so the sweep is eviction-safe: regions freed to disk
/// under stream-to-disk never reach the post-merge full-bbox sweep.
pub fn sweep_floating_veg_region(
    editor: &mut WorldEditor,
    road_mask: &RoadMaskBitmap,
    g_min_x: i32,
    g_max_x: i32,
    g_min_z: i32,
    g_max_z: i32,
) {
    let min_x = g_min_x;
    let max_x = g_max_x;
    let min_z = g_min_z;
    let max_z = g_max_z;
    let veg_set: &[Block] = &[
        TALL_GRASS_BOTTOM,
        TALL_GRASS_TOP,
        GRASS,
        // Ferns + dead bush are scattered at y=1/y=2 over parks/gardens/natural areas and were
        // never swept, so they floated over pond water (confirmed gap). The water/road gate below
        // keeps them on dry land; only the ones over water/roads are cleared.
        FERN,
        LARGE_FERN_LOWER,
        LARGE_FERN_UPPER,
        DEAD_BUSH,
        BROWN_CANDLE,
        BROWN_CANDLE_2,
        BROWN_CANDLE_3,
        BROWN_CANDLE_4,
        SUGAR_CANE,
        BLUE_FLOWER,
        RED_FLOWER,
        WHITE_FLOWER,
        YELLOW_FLOWER,
    ];
    let road_blocks: &[Block] = &[
        BLACK_CONCRETE,
        GRAY_CONCRETE_POWDER,
        GRAY_CONCRETE,
        LIGHT_GRAY_CONCRETE,
        WHITE_CONCRETE,
        CYAN_TERRACOTTA,
    ];
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            let gy = editor.get_ground_level(x, z);
            let is_water = editor.check_for_block_absolute(x, gy, z, Some(&[WATER]), None);
            let is_road_ground = road_mask.contains(x, z)
                || editor.check_for_block_absolute(x, gy, z, Some(road_blocks), None);
            if !(is_water || is_road_ground) {
                continue;
            }
            // Loose ground veg (flowers/grass/ferns) over water or roads, low. Trees are NOT
            // stripped here any more: tree placement now skips water cells up front
            // (`in_water_mask`), so nothing roots in the water, and the canopy that remains over
            // water is legitimate overhang from a bank tree - which we want to keep.
            for dy in 1..=5 {
                let y = gy + dy;
                if editor.check_for_block_absolute(x, y, z, Some(veg_set), None) {
                    editor.set_block_absolute(AIR, x, y, z, None, None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dt_distance_from_shore() {
        let (w, h) = (7usize, 7usize);
        let mut d = vec![0u8; w * h];
        for j in 1..6 {
            for i in 1..6 {
                d[j * w + i] = DT_MAX;
            }
        }
        chamfer_3_4_dt(&mut d, w, h);
        assert_eq!(d[0], 0, "shore stays 0");
        assert_eq!(d[w + 1], 3, "edge water is one ortho step from shore");
        assert_eq!(d[3 * w + 3], 9, "centre is three ortho steps in");
    }

    #[test]
    fn depth_zero_inside_shoal_and_on_land() {
        assert_eq!(ocean_depth_for_cell(0, 0, 0, 0, 1.0), 0);
        assert_eq!(ocean_depth_for_cell(10, 10, 3, 200, 1.0), 0);
    }

    #[test]
    fn depth_clamps_to_tier_max() {
        assert_eq!(ocean_depth_for_cell(5, 5, u16::from(DT_MAX), 200, 1.0), 6);
        assert_eq!(polygon_local_max(10), 2);
        assert_eq!(polygon_local_max(60), 4);
        assert_eq!(polygon_local_max(100), 6);
    }

    #[test]
    fn small_scale_bowl_no_cliff_and_capped() {
        // 7-block-wide river: center DT 12 -> hw 4.0 blocks, target 3.
        assert_eq!(bowl_steps(1.0, 4.0), 0, "shoal edge is flat");
        assert_eq!(bowl_steps(2.0, 4.0), 1);
        assert_eq!(bowl_steps(3.0, 4.0), 2);
        assert_eq!(bowl_steps(4.0, 4.0), 3, "center reaches target");
        // Wide body clamps to the small-scale cap.
        assert_eq!(bowl_steps(100.0, 20.0), SMALL_SCALE_MAX_DEPTH);
        // Adjacent integer distances never jump more than one block (no cliffs).
        for d in 1..16 {
            let a = bowl_steps(d as f64, 8.0);
            let b = bowl_steps((d + 1) as f64, 8.0);
            assert!((a - b).abs() <= 1, "cliff at d={d}: {a}->{b}");
        }
        // A narrow 3-block stream (center DT 6 -> hw 2.0) still carves, not flat.
        assert_eq!(bowl_steps(1.0, 2.0), 0);
        assert_eq!(
            bowl_steps(2.0, 2.0),
            1,
            "3-wide stream carves a 1-deep notch"
        );
    }

    #[test]
    fn small_scale_estimate_returns_cap() {
        let grid = vec![vec![LC_WATER; 32]; 32];
        assert_eq!(
            estimate_max_carve_depth(&grid, 32, 32, 0.2),
            SMALL_SCALE_MAX_DEPTH
        );
        let empty = vec![vec![0u8; 16]; 16];
        assert_eq!(estimate_max_carve_depth(&empty, 16, 16, 0.2), 0);
    }

    #[test]
    fn estimate_no_water_is_zero() {
        let grid = vec![vec![0u8; 16]; 16];
        assert_eq!(estimate_max_carve_depth(&grid, 16, 16, 1.0), 0);
    }

    #[test]
    fn estimate_large_open_water_reaches_max() {
        let grid = vec![vec![LC_WATER; 32]; 32];
        assert_eq!(estimate_max_carve_depth(&grid, 32, 32, 1.0), 6);
    }

    #[test]
    fn grid_span_covers_all_mapped_blocks() {
        // Brute-force every block that samples into grid cells [1,2]; the span must cover them.
        let (world_dim, grid_dim) = (100usize, 10usize);
        let (g_lo, g_hi) = (1usize, 2usize);
        let (lo, hi) = grid_span_to_block_span(g_lo, g_hi, world_dim, grid_dim);
        for bx in 0..world_dim {
            let gx =
                ((bx as f64 / (world_dim - 1) as f64) * (grid_dim - 1) as f64).round() as usize;
            if gx >= g_lo && gx <= g_hi {
                assert!(
                    bx as i32 >= lo && bx as i32 <= hi,
                    "block {bx} (grid {gx}) not covered by span [{lo},{hi}]"
                );
            }
        }
    }

    #[test]
    fn nibble_round_trip() {
        let mut buf = vec![0u8; 4];
        for (i, v) in [0u8, 6, 3, 1, 5, 2, 4, 0].iter().enumerate() {
            nibble_set(&mut buf, i, *v);
        }
        for (i, v) in [0u8, 6, 3, 1, 5, 2, 4, 0].iter().enumerate() {
            assert_eq!(nibble_get(&buf, i), *v);
        }
    }
}
