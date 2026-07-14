//! Climate axis: a bundled Koppen grid, sampled once per generation, drives arid/polar surfaces and biomes; temperate is unchanged.

use crate::block_definitions::*;
use crate::coordinate_system::geographic::LLBBox;
use crate::land_cover::{
    coord_hash, LC_BARE, LC_CROPLAND, LC_GRASSLAND, LC_MOSS, LC_SHRUBLAND, LC_SNOW_ICE,
    LC_TREE_COVER,
};

// Global Koppen-Geiger grid, 0.1 deg, 1 byte/cell (class 1..30, 0 = ocean/nodata).
static KOPPEN: &[u8] = include_bytes!("../assets/climate/koppen_0p1.bin");
const KOPPEN_COLS: usize = 3600;
const KOPPEN_ROWS: usize = 1800;
const KOPPEN_RES: f64 = 0.1;

fn koppen_class(lat: f64, lon: f64) -> u8 {
    if KOPPEN.len() != KOPPEN_COLS * KOPPEN_ROWS {
        return 0;
    }
    let col = (((lon + 180.0) / KOPPEN_RES).floor() as isize).clamp(0, KOPPEN_COLS as isize - 1);
    let row = (((90.0 - lat) / KOPPEN_RES).floor() as isize).clamp(0, KOPPEN_ROWS as isize - 1);
    KOPPEN[row as usize * KOPPEN_COLS + col as usize]
}

// ── Domain-warp: bend the grid's rectangular class edges into organic blobs ───────────────
// The Koppen grid is 0.1-deg cells, so a raw nearest-cell lookup gives blocky, axis-aligned
// climate boundaries. We perturb the LOOKUP COORDINATE by a smooth low-frequency noise field
// before sampling, so straight edges curve into blobs. It is a pure function of (lat, lon), so
// two adjacent Meld cells warp a shared point identically -> byte-identical across tile seams.
//
// Noise wavelength (LATTICE_DEG) is a few climate cells wide so the warp reshapes edges into
// large smooth curves rather than high-frequency ragged fringe; amplitude (AMP_DEG) is about one
// climate cell so blobs wiggle without teleporting a climate far from where it really is.
const WARP_LATTICE_DEG: f64 = 0.45; // coarse noise lattice (~50 km) -> big smooth blob shapes
const WARP_AMP_DEG: f64 = 0.22; // displacement ~2-3 grid cells: bends edges AND breaks the
                                // 0.1-deg nearest-cell staircase into organic fringe (the fbm's
                                // finer octaves reach ~0.05 deg, below the cell size)

/// Deterministic integer hash -> [0, 1). Pure; no RNG state.
fn hash01(ix: i64, iy: i64, seed: u64) -> f64 {
    let mut h = (ix as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (iy as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ seed.wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    // top 53 bits -> [0, 1)
    (h >> 11) as f64 / ((1u64 << 53) as f64)
}

/// Smooth (smoothstep-interpolated) value noise on a unit lattice, returns [-1, 1].
fn value_noise(x: f64, y: f64, seed: u64) -> f64 {
    let x0 = x.floor();
    let y0 = y.floor();
    let ix = x0 as i64;
    let iy = y0 as i64;
    let fx = x - x0;
    let fy = y - y0;
    let sx = fx * fx * (3.0 - 2.0 * fx); // smoothstep
    let sy = fy * fy * (3.0 - 2.0 * fy);
    let n00 = hash01(ix, iy, seed);
    let n10 = hash01(ix + 1, iy, seed);
    let n01 = hash01(ix, iy + 1, seed);
    let n11 = hash01(ix + 1, iy + 1, seed);
    let nx0 = n00 + (n10 - n00) * sx;
    let nx1 = n01 + (n11 - n01) * sx;
    (nx0 + (nx1 - nx0) * sy) * 2.0 - 1.0
}

/// 4-octave fractal noise: octave 1 sets the blob shape, the finer octaves (down to ~1/8 the
/// lattice, i.e. below the 0.1-deg cell) break the nearest-cell staircase into organic fringe.
/// Returns ~[-1, 1].
fn fbm(x: f64, y: f64, seed: u64) -> f64 {
    let mut v = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    let mut norm = 0.0;
    for o in 0..4 {
        v += amp * value_noise(x * freq, y * freq, seed.wrapping_add(o * 0x9E37));
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    v / norm // keep the range ~[-1, 1] regardless of octave count
}

/// Warp (lat, lon) by the smooth noise field so climate boundaries read as blobs, not blocks.
fn warp_lookup(lat: f64, lon: f64) -> (f64, f64) {
    let nx = lon / WARP_LATTICE_DEG;
    let ny = lat / WARP_LATTICE_DEG;
    let dlon = WARP_AMP_DEG * fbm(nx, ny, 0x0C11_A7E0_u64);
    let dlat = WARP_AMP_DEG * fbm(nx + 47.13, ny - 19.7, 0x5EED_B10B_u64);
    (lat + dlat, lon + dlon)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Climate {
    /// C*, humid-continental D*, tropical rainforest, and ocean/nodata: existing behaviour.
    Temperate,
    TropicalSavanna,
    HotDesert,
    HotSteppe,
    ColdDesert,
    ColdSteppe,
    DryContinental,
    Boreal,
    Tundra,
    IceCap,
}

impl Climate {
    fn from_class(c: u8) -> Climate {
        match c {
            3 => Climate::TropicalSavanna,                  // Aw
            4 => Climate::HotDesert,                        // BWh
            5 => Climate::ColdDesert,                       // BWk
            6 => Climate::HotSteppe,                        // BSh
            7 => Climate::ColdSteppe,                       // BSk
            17 | 18 | 21 | 22 => Climate::DryContinental,   // Dsa/Dsb, Dwa/Dwb
            19 | 20 | 23 | 24 | 27 | 28 => Climate::Boreal, // Dsc/Dsd, Dwc/Dwd, Dfc/Dfd
            29 => Climate::Tundra,                          // ET
            30 => Climate::IceCap,                          // EF
            _ => Climate::Temperate,                        // Af/Am, C*, Dfa/Dfb, 0
        }
    }

    /// Sample the climate at a single lat/lon (one cheap Koppen-grid lookup). This is the
    /// per-position entry point so the climate can vary across a tiled world instead of being
    /// fixed per cell.
    pub fn at(lat: f64, lon: f64) -> Climate {
        let (wlat, wlon) = warp_lookup(lat, lon);
        Climate::from_class(koppen_class(wlat, wlon))
    }

    /// Sample the climate at the bbox center (used for the single-world / non-tiled fallback).
    pub fn classify(bbox: &LLBBox) -> Climate {
        let lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
        let lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;
        Climate::at(lat, lon)
    }

    /// Surface palette (surface, under) for veg/bare cover, or None to keep the baseline.
    pub fn surface_palette(self, cover: u8, x: i32, z: i32) -> Option<(Block, Block)> {
        // DryContinental (Grand Canyon) keeps baseline blocks; only its biome is adapted.
        if matches!(
            self,
            Climate::Temperate | Climate::TropicalSavanna | Climate::DryContinental
        ) {
            return None;
        }
        let veg = matches!(
            cover,
            LC_TREE_COVER | LC_SHRUBLAND | LC_GRASSLAND | LC_CROPLAND | LC_MOSS
        );
        let bare = cover == LC_BARE || cover == LC_SNOW_ICE;
        if !veg && !bare {
            return None;
        }
        let h = coord_hash(x, z);
        let pal = match self {
            Climate::IceCap => {
                if h.is_multiple_of(6) {
                    (PACKED_ICE, PACKED_ICE)
                } else {
                    (SNOW_BLOCK, SNOW_BLOCK)
                }
            }
            Climate::HotDesert => match h % 12 {
                0 => (SANDSTONE, SANDSTONE),
                1 => (SMOOTH_SANDSTONE, SANDSTONE),
                _ => (SAND, SANDSTONE),
            },
            Climate::HotSteppe if bare => match h % 10 {
                0..=4 => (SAND, SANDSTONE),
                _ => (COARSE_DIRT, DIRT),
            },
            Climate::HotSteppe => match h % 10 {
                0..=2 => (SAND, SANDSTONE),
                3..=5 => (COARSE_DIRT, DIRT),
                _ => (GRASS_BLOCK, DIRT),
            },
            Climate::ColdDesert if bare => match h % 12 {
                0..=4 => (GRAVEL, STONE),
                5..=8 => (COARSE_DIRT, DIRT),
                _ => (STONE, STONE),
            },
            Climate::ColdDesert => match h % 10 {
                0..=4 => (COARSE_DIRT, DIRT),
                5..=7 => (GRAVEL, STONE),
                _ => (GRASS_BLOCK, DIRT),
            },
            Climate::ColdSteppe if bare => match h % 10 {
                0..=5 => (COARSE_DIRT, DIRT),
                _ => (GRAVEL, STONE),
            },
            Climate::ColdSteppe => match h % 10 {
                0..=2 => (COARSE_DIRT, DIRT),
                _ => (GRASS_BLOCK, DIRT),
            },
            Climate::Boreal if bare => match h % 10 {
                0..=4 => (COARSE_DIRT, DIRT),
                _ => (GRAVEL, STONE),
            },
            Climate::Boreal => match h % 10 {
                0..=3 => (PODZOL, DIRT),
                4..=5 => (COARSE_DIRT, DIRT),
                _ => (GRASS_BLOCK, DIRT),
            },
            Climate::Tundra if bare => match h % 10 {
                0..=4 => (GRAVEL, STONE),
                5..=7 => (COARSE_DIRT, DIRT),
                _ => (STONE, STONE),
            },
            Climate::Tundra => match h % 10 {
                0..=3 => (COARSE_DIRT, DIRT),
                4..=5 => (MOSS_BLOCK, DIRT),
                _ => (GRASS_BLOCK, DIRT),
            },
            Climate::Temperate | Climate::TropicalSavanna | Climate::DryContinental => return None,
        };
        Some(pal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_groups() {
        assert_eq!(Climate::from_class(4), Climate::HotDesert);
        assert_eq!(Climate::from_class(7), Climate::ColdSteppe);
        assert_eq!(Climate::from_class(30), Climate::IceCap);
        assert_eq!(Climate::from_class(15), Climate::Temperate); // Cfb
        assert_eq!(Climate::from_class(0), Climate::Temperate); // ocean
    }

    #[test]
    fn temperate_never_overrides() {
        assert!(Climate::Temperate.surface_palette(LC_BARE, 1, 2).is_none());
    }

    #[test]
    fn desert_overrides_to_sand() {
        let (s, _) = Climate::HotDesert
            .surface_palette(LC_GRASSLAND, 7, 7)
            .unwrap();
        assert!(matches!(s, SAND | SANDSTONE | SMOOTH_SANDSTONE));
    }

    #[test]
    fn embedded_grid_size_matches() {
        // If this fails the embedded grid is wrong; koppen_class then safely returns 0.
        assert_eq!(KOPPEN.len(), KOPPEN_COLS * KOPPEN_ROWS);
    }

    #[test]
    fn warp_is_deterministic_and_bounded() {
        // Same point -> same warp (seam-safety: two cells sampling a shared point must agree).
        let a = warp_lookup(45.6, 24.4);
        let b = warp_lookup(45.6, 24.4);
        assert_eq!(a, b);
        // Displacement never exceeds the amplitude (climate can't teleport far).
        let (wlat, wlon) = warp_lookup(45.6, 24.4);
        assert!((wlat - 45.6).abs() <= WARP_AMP_DEG + 1e-9);
        assert!((wlon - 24.4).abs() <= WARP_AMP_DEG + 1e-9);
        // value_noise stays in [-1, 1].
        for i in 0..50 {
            let n = value_noise(i as f64 * 0.37, i as f64 * 0.91, 42);
            assert!((-1.0..=1.0).contains(&n));
        }
    }

    #[test]
    fn classify_real_locations() {
        use crate::coordinate_system::geographic::LLBBox;
        let cases = [
            ("22.9,12.9,23.1,13.1", Climate::HotDesert),   // Sahara
            ("48.1,8.1,48.3,8.3", Climate::Temperate),     // Black Forest
            ("71.9,-40.1,72.1,-39.9", Climate::IceCap),    // Greenland
            ("-3.2,-60.1,-3.0,-59.9", Climate::Temperate), // Amazon (Af -> latitude jungle)
        ];
        for (bb, want) in cases {
            let bbox = LLBBox::from_str(bb).unwrap();
            assert_eq!(Climate::classify(&bbox), want, "bbox {bb}");
        }
    }
}
