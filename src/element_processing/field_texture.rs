//! Configurable farmland texturing.
//!
//! Splits OSM `landuse=farmland` into a weighted mix of five styles — coarse dirt,
//! plains grass, flower plains, tilled farmland, and mossy overgrowth — laid out as
//! rectangular **field parcels** (like real agricultural plots seen from above), with
//! occasional dirt-track boundaries between adjacent parcels of different kinds. Each
//! style also carries a fine internal sub-noise so it reads like real varied ground
//! (dirt patches, grass, etc.) rather than one flat block.
//!
//! Everything is a pure function of `(x, z)` → identical across tile seams. When
//! `--field-mix` is omitted the mix is `farm=100`, keeping every cell tilled farmland
//! → byte-identical to stock arnis.

use crate::block_definitions::*;
use crate::ground_generation::value_noise_01;
use crate::land_cover::coord_hash;

/// One farmland patch style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldCategory {
    /// Bare, disturbed ground: coarse dirt with dirt patches and dead bushes.
    Coarse,
    /// Open plains grass.
    Plains,
    /// Grass sprinkled with wildflowers.
    Flower,
    /// Stock tilled farmland with crops (unchanged behaviour).
    Farm,
    /// Overgrown mossy patch (moss with grass/coarse patches).
    Moss,
}

/// A resolved farmland cell: which style, the surface block to place, and whether this
/// cell is a dirt-track parcel boundary (which should carry no crops/decoration).
#[derive(Clone, Copy)]
pub struct FieldCell {
    pub cat: FieldCategory,
    pub surface: Block,
    pub is_track: bool,
}

/// Relative area shares for the five farmland categories.
#[derive(Clone, Copy)]
pub struct FieldMix {
    coarse: u16,
    plains: u16,
    flower: u16,
    farm: u16,
    moss: u16,
    default: bool,
}

/// Region (blocks) over which the parcel SIZE is held constant, so a whole area shares
/// small plots or large plots — mixing plot sizes across the map like real farmland.
const MACRO: i32 = 160;
/// Max block offset applied before quantising to a parcel, so parcel edges wander a
/// little instead of being pixel-straight (kept small — real fields are fairly square).
const WARP: f64 = 4.0;
/// Lattice period of the warp noise.
const WARP_SCALE: i32 = 24;
/// Lattice period of the intra-parcel surface sub-noise (small patches).
const SUB_SCALE: i32 = 6;

/// Parcel edge length for the region containing macro-cell `(mx, mz)`: small / medium /
/// large plots, so plot sizes vary across the map.
fn parcel_size(mx: i32, mz: i32) -> i32 {
    match coord_hash(mx ^ 0x0000_51ED, mz.wrapping_mul(7)) % 3 {
        0 => 18,
        1 => 30,
        _ => 46,
    }
}

/// Per-category surface block from a fine sub-noise, so each style reads as varied
/// ground (dirt/grass patches) instead of one flat block.
fn surface_block(cat: FieldCategory, x: i32, z: i32) -> Block {
    let n = (value_noise_01(x, z, SUB_SCALE) * 1000.0) as i32;
    match cat {
        // Mostly coarse dirt, with soil (dirt) patches and the odd grassy spot.
        FieldCategory::Coarse => {
            if n < 300 {
                DIRT
            } else if n < 360 {
                GRASS_BLOCK
            } else {
                COARSE_DIRT
            }
        }
        // Moss reclaimed by grass, with the odd bare patch.
        FieldCategory::Moss => {
            if n < 300 {
                GRASS_BLOCK
            } else if n < 370 {
                COARSE_DIRT
            } else {
                MOSS_BLOCK
            }
        }
        FieldCategory::Plains | FieldCategory::Flower => GRASS_BLOCK,
        FieldCategory::Farm => FARMLAND,
    }
}

impl FieldMix {
    /// Stock behaviour: all farmland stays tilled farmland.
    pub const fn stock() -> Self {
        FieldMix {
            coarse: 0,
            plains: 0,
            flower: 0,
            farm: 100,
            moss: 0,
            default: true,
        }
    }

    /// Parse a `name=pct` list, e.g. `plains=60,coarse=20,flower=10,farm=10,moss=15`.
    /// Order/subset are free; unknown keys ignored. `None`, empty, or an all-zero spec
    /// falls back to [`FieldMix::stock`] so a default run stays byte-identical.
    pub fn parse(spec: Option<&str>) -> Self {
        let Some(s) = spec.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::stock();
        };
        let (mut coarse, mut plains, mut flower, mut farm, mut moss) = (0u16, 0u16, 0u16, 0u16, 0u16);
        for tok in s.split(',') {
            if let Some((k, v)) = tok.split_once('=') {
                let val: u16 = v.trim().parse().unwrap_or(0);
                match k.trim().to_ascii_lowercase().as_str() {
                    "coarse" => coarse = val,
                    "plains" => plains = val,
                    "flower" => flower = val,
                    "farm" => farm = val,
                    "moss" => moss = val,
                    _ => {}
                }
            }
        }
        if coarse as u32 + plains as u32 + flower as u32 + farm as u32 + moss as u32 == 0 {
            return Self::stock();
        }
        FieldMix {
            coarse,
            plains,
            flower,
            farm,
            moss,
            default: false,
        }
    }

    /// True when the mix reproduces stock farmland (nothing to override).
    pub fn is_default(&self) -> bool {
        self.default
    }

    fn total(&self) -> u64 {
        self.coarse as u64 + self.plains as u64 + self.flower as u64 + self.farm as u64 + self.moss as u64
    }

    /// Category assigned to the whole parcel `(px, pz)`.
    fn category_for_parcel(&self, px: i32, pz: i32) -> FieldCategory {
        let total = self.total();
        if total == 0 {
            return FieldCategory::Farm;
        }
        let mut roll = coord_hash(px, pz ^ 0x5F35_6495) % total;
        for (share, cat) in [
            (self.coarse, FieldCategory::Coarse),
            (self.plains, FieldCategory::Plains),
            (self.flower, FieldCategory::Flower),
            (self.farm, FieldCategory::Farm),
            (self.moss, FieldCategory::Moss),
        ] {
            if roll < share as u64 {
                return cat;
            }
            roll -= share as u64;
        }
        FieldCategory::Farm
    }

    /// Warp `(x, z)` slightly and quantise to the region's parcel grid.
    /// Returns `(parcel_x, parcel_z, parcel_size, local_x, local_z)`.
    fn parcel_at(&self, x: i32, z: i32) -> (i32, i32, i32, i32, i32) {
        let wx = value_noise_01(x + 1000, z - 500, WARP_SCALE);
        let wz = value_noise_01(x - 700, z + 1300, WARP_SCALE);
        let sx = x + ((wx - 0.5) * 2.0 * WARP).round() as i32;
        let sz = z + ((wz - 0.5) * 2.0 * WARP).round() as i32;
        let ps = parcel_size(sx.div_euclid(MACRO), sz.div_euclid(MACRO));
        (
            sx.div_euclid(ps),
            sz.div_euclid(ps),
            ps,
            sx.rem_euclid(ps),
            sz.rem_euclid(ps),
        )
    }

    /// Category for the parcel containing `(x, z)`.
    pub fn category_at(&self, x: i32, z: i32) -> FieldCategory {
        let (px, pz, ..) = self.parcel_at(x, z);
        self.category_for_parcel(px, pz)
    }

    /// Full resolution of a farmland cell: style + surface block + track flag.
    pub fn cell_at(&self, x: i32, z: i32) -> FieldCell {
        let (px, pz, ps, lx, lz) = self.parcel_at(x, z);
        let cat = self.category_for_parcel(px, pz);
        // A dirt track appears on a parcel edge that borders a DIFFERENT style, on ~45%
        // of such seams — the visible field boundaries, not a full grid.
        let mut is_track = false;
        if lx == 0 || lz == 0 || lx == ps - 1 || lz == ps - 1 {
            let (nx, nz) = if lx == 0 {
                (px - 1, pz)
            } else if lx == ps - 1 {
                (px + 1, pz)
            } else if lz == 0 {
                (px, pz - 1)
            } else {
                (px, pz + 1)
            };
            if self.category_for_parcel(nx, nz) != cat
                && coord_hash(px ^ nx, (pz ^ nz) ^ 0x0000_7A11) % 100 < 45
            {
                is_track = true;
            }
        }
        let surface = if is_track {
            DIRT_PATH
        } else {
            surface_block(cat, x, z)
        };
        FieldCell {
            cat,
            surface,
            is_track,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_farm() {
        let m = FieldMix::parse(None);
        assert!(m.is_default());
        for x in -40..40 {
            for z in -40..40 {
                assert_eq!(m.category_at(x, z), FieldCategory::Farm);
                let c = m.cell_at(x, z);
                assert_eq!(c.cat, FieldCategory::Farm);
                assert!(!c.is_track);
            }
        }
    }

    #[test]
    fn empty_and_zero_fall_back_to_stock() {
        assert!(FieldMix::parse(Some("")).is_default());
        assert!(FieldMix::parse(Some("plains=0,farm=0")).is_default());
    }

    #[test]
    fn weights_roughly_match_area_share() {
        let m = FieldMix::parse(Some("plains=50,farm=50"));
        assert!(!m.is_default());
        let (mut plains, mut farm, mut other) = (0, 0, 0);
        // Wide area so many independent parcels are sampled (law of large numbers).
        for x in (0..6000).step_by(13) {
            for z in (0..6000).step_by(13) {
                match m.category_at(x, z) {
                    FieldCategory::Plains => plains += 1,
                    FieldCategory::Farm => farm += 1,
                    _ => other += 1,
                }
            }
        }
        assert_eq!(other, 0, "no weight assigned to other categories");
        let ratio = plains as f64 / (plains + farm) as f64;
        assert!(ratio > 0.38 && ratio < 0.62, "plains share {ratio} not ~0.5");
    }

    #[test]
    fn tracks_only_between_different_styles() {
        // A single-category mix can never form a track (no differing neighbours).
        let m = FieldMix::parse(Some("plains=100"));
        for x in 0..200 {
            for z in 0..200 {
                assert!(!m.cell_at(x, z).is_track);
            }
        }
    }
}
