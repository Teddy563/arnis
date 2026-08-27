//! Diagnose which density sub-term (cheese/entrances/spaghetti/pillars) drives tall/round "egg"
//! caverns. Scans a block area with the SAME seed formula as caves::mod.rs (noise_seed=0,
//! since our test worlds don't pass --seed), finds the tallest per-column contiguous carved run
//! (cell_density <= 0), and prints the sub-term breakdown at the top/mid/bottom of that run so we can
//! see which term stays negative across the whole span (i.e. which system produced the tall shape).
//!   cargo run --release --example caveshape_probe -- <bx0> <bz0> <span>
#![allow(
    dead_code,
    unused_mut,
    clippy::unnecessary_sort_by,
    clippy::manual_range_contains,
    clippy::type_complexity,
    clippy::identity_op,
    clippy::needless_range_loop,
    clippy::too_many_arguments
)]
// ^ diagnostic tooling: kept readable over lint-perfect; dead_code covers the shared
//   src modules compiled into this standalone example target.

// density.rs reads the world top for its cave pinch band; outside the real crate this
// shim supplies the vanilla top (319), reproducing the historical 240..256 band.
mod world_editor {
    pub fn world_max_y() -> i32 {
        319
    }
}

#[path = "../src/caves/density.rs"]
mod density;
#[path = "../src/caves/noise.rs"]
mod noise;
#[path = "../src/caves/rng.rs"]
mod rng;

use density::CaveGen;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    // XSECT MODE: `caveshape_probe xsect <cx> <cy> <cz>` — ASCII horizontal (XZ @ cy) and vertical
    // (XY @ cz) cross-sections centered on a found column, to see if a tall run is actually WIDE
    // (a cavern) or just a thin near-vertical tube segment (spaghetti/entrance tunnel).
    if a.get(1).map(|s| s == "xsect").unwrap_or(false) {
        let cx: i32 = a[2].parse().unwrap();
        let cy: i32 = a[3].parse().unwrap();
        let cz: i32 = a[4].parse().unwrap();
        let seed: i64 = (0u64 ^ 0xCA7E_CA7E) as i64;
        let gen = CaveGen::new(seed);
        let r = 24;
        println!(
            "horizontal XZ @ y={cy} (x {}..{}, z {}..{}):",
            cx - r,
            cx + r,
            cz - r,
            cz + r
        );
        for z in cz - r..=cz + r {
            let mut row = String::new();
            for x in cx - r..=cx + r {
                row.push(if gen.cell_density(x, cy, z) <= 0.0 {
                    '#'
                } else {
                    '.'
                });
            }
            println!("  {row}");
        }
        println!();
        println!(
            "vertical XY @ z={cz} (x {}..{}, y {}..{}):",
            cx - r,
            cx + r,
            cy - r,
            cy + r
        );
        for y in (cy - r..=cy + r).rev() {
            let mut row = String::new();
            for x in cx - r..=cx + r {
                row.push(if gen.cell_density(x, y, cz) <= 0.0 {
                    '#'
                } else {
                    '.'
                });
            }
            println!("  {row}");
        }
        return;
    }
    let bx0: i32 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let bz0: i32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let span: i32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(256);
    let seed: i64 = (0u64 ^ 0xCA7E_CA7E) as i64;
    let gen = CaveGen::new(seed);
    let (y0, y1) = (-56, 50);

    let mut best: Vec<(i32, i32, i32, i32, i32)> = Vec::new(); // (run, x, z, ylo, yhi)
    for x in bx0..bx0 + span {
        for z in bz0..bz0 + span {
            let mut run = 0i32;
            let mut run_lo = y0;
            let mut col_best: Option<(i32, i32, i32)> = None; // (run, ylo, yhi)
            for y in y0..=y1 {
                if gen.cell_density(x, y, z) <= 0.0 {
                    if run == 0 {
                        run_lo = y;
                    }
                    run += 1;
                    if col_best.map(|(r, _, _)| run > r).unwrap_or(true) {
                        col_best = Some((run, run_lo, y));
                    }
                } else {
                    run = 0;
                }
            }
            if let Some((r, lo, hi)) = col_best {
                best.push((r, x, z, lo, hi));
            }
        }
    }
    best.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    best.dedup_by(|a, b| (a.1 - b.1).abs() < 8 && (a.2 - b.2).abs() < 8);
    println!(
        "top tall columns in [{bx0}..{},{bz0}..{}] y[{y0}..{y1}]:",
        bx0 + span,
        bz0 + span
    );
    for &(run, x, z, lo, hi) in best.iter().take(8) {
        println!("  run={run:3}  x={x} z={z}  y{lo}..{hi}");
    }
    println!();
    println!("sub-term breakdown for the top column (cheese_term, entrances, spag, pillars — carve iff min(cheese,entrances,spag)<=0 or pillars>=0.03, whichever survives):");
    if let Some(&(run, x, z, lo, hi)) = best.first() {
        for y in (lo..=hi).step_by(((hi - lo).max(1) / 10).max(1) as usize) {
            let (cheese, entrances, spag, pillars) = gen.density_breakdown(x, y, z);
            let (arg_a, arg_b) = gen.entrances_breakdown(x, y, z);
            let combined = gen.cell_density(x, y, z);
            println!(
                "  y={y:4}  cheese={cheese:7.3}  entrances={entrances:7.3} (arg_a={arg_a:7.3} arg_b={arg_b:7.3})  spag={spag:7.3}  pillars={pillars:7.3}  combined={combined:7.3}"
            );
        }
        println!("(run={run} at x={x} z={z})");
    }
}
