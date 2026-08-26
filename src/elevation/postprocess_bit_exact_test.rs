//! D2 (perf phase 2): the bit-exactness gate for the built-up elevation Gaussian.
//!
//! [`crate::elevation::postprocess::gaussian_blur_grid`] is the blur that
//! `smooth_built_up_gaussian` runs over the elevation grid (2049x2049 x 183 taps x 2
//! passes x 2 blurs on a real cell). Phase-2 task D1 will re-lay it from `Vec<Vec<f64>>`
//! onto a flat `Vec<f64>` + stride, transpose between passes, and parallelise the scatter
//! and the blend. That rewrite ships with **no flag**, on the sole claim that it is
//! bit-for-bit identical - so the claim needs a gate that exists BEFORE the rewrite does.
//!
//! This is that gate (plan gate G4). It pins the current output two ways:
//!
//! 1. against a reference implementation kept in this file - a deliberately dumb,
//!    single-threaded transcription of today's algorithm: same kernel, same left-to-right
//!    tap order, same running `sum`/`wsum` accumulation, same edge renormalisation, same
//!    `wsum == 0 -> f64::NAN` rule; and
//! 2. against a committed checksum of every output bit pattern, so a future change that
//!    "fixes" the implementation and this reference together still fails.
//!
//! Comparison is by [`f64::to_bits`], never `==`: an `==` on f64 would accept a result
//! that drifted in the last mantissa bit (and would reject NaN against itself), which is
//! exactly the class of change D1 must not make. Any reordering of the taps, any change of
//! accumulation order (pairwise/SIMD/rayon reduction over the kernel), any f32
//! intermediate, and any change to the NaN or edge handling moves at least one bit and
//! fails here.
//!
//! Placed next to the code it gates; declared from `src/world_editor/mod.rs` only because
//! this crate is a binary with no `tests/` target and the phase-2 file ownership split put
//! `postprocess.rs` in another agent's hands. D1's owner should move the `mod` declaration
//! into `postprocess.rs` and delete this note.

use crate::elevation::postprocess::gaussian_blur_grid;

/// Deterministic, portable value source. A tiny SplitMix64 - no `rand`, no float parsing,
/// identical on every platform, and it fills the whole mantissa so a one-ulp drift shows.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A plausible elevation in metres: 53 random mantissa bits scaled into 0..250.
    fn next_height(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64) * 250.0
    }
}

/// `h` x `w` pseudo-random grid with NaN holes, which is what the real elevation grid looks
/// like before `fill_nan_values`: every `hole_every`-th cell in raster order is a hole, so
/// the "skip this tap" branch is exercised throughout.
fn grid(h: usize, w: usize, seed: u64, hole_every: usize) -> Vec<Vec<f64>> {
    let mut rng = SplitMix64(seed);
    let mut out = Vec::with_capacity(h);
    for y in 0..h {
        let mut row = Vec::with_capacity(w);
        for x in 0..w {
            let v = rng.next_height();
            let hole = hole_every > 0 && (y * w + x).is_multiple_of(hole_every);
            row.push(if hole { f64::NAN } else { v });
        }
        out.push(row);
    }
    out
}

// --- reference implementation -------------------------------------------------------
// Transcribed from `postprocess.rs` as it stands. Intentionally the slow, obvious form:
// no rayon, no chunking, no column gather. It must never be "optimised" - its whole job is
// to be a second, independent statement of the arithmetic.

fn reference_kernel(size: usize, sigma: f64) -> Vec<f64> {
    let mut kernel = vec![0.0f64; size];
    let center = (size - 1) as f64 / 2.0;
    for (i, value) in kernel.iter_mut().enumerate() {
        let x = i as f64 - center;
        *value = (-x * x / (2.0 * sigma * sigma)).exp();
    }
    let mut sum = 0.0f64;
    for k in kernel.iter() {
        sum += *k; // left-to-right, matching `kernel.iter().sum::<f64>()`
    }
    for k in kernel.iter_mut() {
        *k /= sum;
    }
    kernel
}

/// One 1-D pass over `line` with `kernel`, renormalising over the finite taps in range.
fn reference_pass(line: &[f64], kernel: &[f64], half: i32) -> Vec<f64> {
    let len = line.len() as i32;
    (0..line.len())
        .map(|i| {
            let mut sum = 0.0f64;
            let mut wsum = 0.0f64;
            for (j, &k) in kernel.iter().enumerate() {
                let idx = i as i32 + j as i32 - half;
                if idx >= 0 && idx < len {
                    let v = line[idx as usize];
                    if v.is_finite() {
                        sum += v * k;
                        wsum += k;
                    }
                }
            }
            if wsum > 0.0 {
                sum / wsum
            } else {
                f64::NAN
            }
        })
        .collect()
}

fn reference_blur(input: &[Vec<f64>], sigma: f64) -> Vec<Vec<f64>> {
    let kernel_size: usize = (sigma * 3.0).ceil() as usize * 2 + 1;
    let kernel = reference_kernel(kernel_size, sigma);
    let half = kernel_size as i32 / 2;

    let h = input.len();
    if h == 0 {
        return Vec::new();
    }
    let w = input[0].len();
    if w == 0 {
        return vec![Vec::new(); h];
    }

    let after_h: Vec<Vec<f64>> = input
        .iter()
        .map(|row| reference_pass(row, &kernel, half))
        .collect();

    let mut out = vec![vec![0.0f64; w]; h];
    for x in 0..w {
        let column: Vec<f64> = after_h.iter().map(|row| row[x]).collect();
        let col = reference_pass(&column, &kernel, half);
        for (y, v) in col.into_iter().enumerate() {
            out[y][x] = v;
        }
    }
    out
}

// --- the gate -----------------------------------------------------------------------

/// FNV-1a over the raw bit patterns, so the committed constant is a function of the exact
/// f64s and nothing else (no formatting, no rounding).
fn bits_digest(grids: &[&Vec<Vec<f64>>]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for g in grids {
        for row in g.iter() {
            for v in row.iter() {
                for byte in v.to_bits().to_le_bytes() {
                    hash ^= byte as u64;
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
        }
    }
    hash
}

#[track_caller]
fn assert_bit_identical(got: &[Vec<f64>], want: &[Vec<f64>], case: &str) {
    assert_eq!(got.len(), want.len(), "{case}: row count");
    for (y, (grow, wrow)) in got.iter().zip(want.iter()).enumerate() {
        assert_eq!(grow.len(), wrow.len(), "{case}: row {y} width");
        for (x, (g, w)) in grow.iter().zip(wrow.iter()).enumerate() {
            assert_eq!(
                g.to_bits(),
                w.to_bits(),
                "{case}: cell ({x},{y}) differs by bit pattern: got {g:?} want {w:?}. \
                 The built-up elevation Gaussian is NOT bit-exact any more - see the \
                 header of this file (plan tasks D1/D2)."
            );
        }
    }
}

/// Digest of the three cases below, hashed together. Regenerate ONLY on a deliberate,
/// reviewed decision to change the blur's output - which, for D1, means the task failed.
const EXPECTED_DIGEST: u64 = 0xbca0_675b_8254_83d6;

#[test]
fn gaussian_blur_is_bit_exact() {
    // Case A: the normal shape. 37x41 is non-square and divides unevenly by the blur's
    // internal 10 progress chunks (row_chunk = 4, col_chunk = 5), so a change that let a
    // chunk boundary leak into the arithmetic is visible here.
    let a_in = grid(37, 41, 0x5EED_0001, 13);
    let a_got = gaussian_blur_grid(&a_in, 2.5);
    let a_want = reference_blur(&a_in, 2.5);
    assert_bit_identical(&a_got, &a_want, "A/sigma=2.5");

    // Not a vacuous pass: most of case A must actually be finite numbers.
    let finite = a_got
        .iter()
        .flat_map(|r| r.iter())
        .filter(|v| v.is_finite())
        .count();
    assert!(
        finite > (37 * 41 * 9) / 10,
        "case A produced only {finite} finite cells of {}; the gate would be vacuous",
        37 * 41
    );

    // Case B: kernel (49 taps) wider than the grid, so EVERY output is edge-renormalised
    // over a partial window - the branch a flat-layout rewrite is most likely to reorder.
    let b_in = grid(11, 13, 0x5EED_0002, 7);
    let b_got = gaussian_blur_grid(&b_in, 8.0);
    let b_want = reference_blur(&b_in, 8.0);
    assert_bit_identical(&b_got, &b_want, "B/sigma=8.0");

    // Case C: an all-NaN patch. Every window is empty, so `wsum == 0` everywhere and the
    // output must be the literal `f64::NAN` bit pattern rather than a 0/0 NaN carrying
    // some other payload.
    let c_in = vec![vec![f64::NAN; 5]; 5];
    let c_got = gaussian_blur_grid(&c_in, 0.5);
    let c_want = reference_blur(&c_in, 0.5);
    assert_bit_identical(&c_got, &c_want, "C/all-NaN");
    for row in &c_got {
        for v in row {
            assert_eq!(v.to_bits(), f64::NAN.to_bits(), "C: canonical NaN expected");
        }
    }

    // The committed expectation, so implementation and reference cannot drift together.
    let digest = bits_digest(&[&a_got, &b_got, &c_got]);
    assert_eq!(
        digest, EXPECTED_DIGEST,
        "blur output digest changed: got {digest:#018x}, committed {EXPECTED_DIGEST:#018x}"
    );
}

/// Empty and degenerate inputs, pinned so a rewrite cannot start returning a differently
/// shaped grid for them (`smooth_built_up_gaussian` feeds it whatever the elevation grid
/// is).
#[test]
fn gaussian_blur_degenerate_shapes() {
    assert!(gaussian_blur_grid(&[], 2.0).is_empty());
    let zero_width: Vec<Vec<f64>> = vec![Vec::new(); 3];
    let out = gaussian_blur_grid(&zero_width, 2.0);
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|r| r.is_empty()));
}
