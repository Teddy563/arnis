//! Configurable land texturing.
//!
//! Splits farmland/grassland into a weighted mix of five styles — coarse dirt, plains
//! grass, flower plains, tilled farmland, and mossy overgrowth — laid out as rectangular
//! **parcels** (like real plots from above), with occasional dirt-track boundaries and a
//! fine internal sub-noise so each style reads as varied ground rather than one flat block.
//!
//! A [`FieldProfile`] bundles a mix with a parcel-size band and track rate, so different
//! land kinds get a distinct look: tight tilled farmland vs large loose meadows. Every
//! value is a pure function of `(x, z)` → identical across tile seams. An inactive/stock
//! profile reproduces the original surface exactly (byte-identical).

use crate::block_definitions::*;
use crate::ground_generation::value_noise_01;
use crate::land_cover::coord_hash;

/// One patch style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldCategory {
    Coarse,
    Plains,
    Flower,
    Farm,
    Moss,
}

/// A resolved cell: style, surface block, and whether it's a dirt-track boundary.
#[derive(Clone, Copy)]
pub struct FieldCell {
    pub cat: FieldCategory,
    pub surface: Block,
    pub is_track: bool,
}

/// Relative area shares for the five categories.
#[derive(Clone, Copy)]
pub struct FieldMix {
    coarse: u16,
    plains: u16,
    flower: u16,
    farm: u16,
    moss: u16,
    default: bool,
}

/// A land-kind texture: a mix plus its parcel-size band and track probability.
#[derive(Clone, Copy)]
pub struct FieldProfile {
    mix: FieldMix,
    sizes: [i32; 3],
    track_pct: u64,
    salt: i32,
}

const MACRO: i32 = 160;
const WARP: f64 = 4.0;
const WARP_SCALE: i32 = 24;
const SUB_SCALE: i32 = 6;

/// Per-category surface from a fine sub-noise, so each style is varied ground.
fn surface_block(cat: FieldCategory, x: i32, z: i32) -> Block {
    let n = (value_noise_01(x, z, SUB_SCALE) * 1000.0) as i32;
    match cat {
        FieldCategory::Coarse => {
            if n < 300 {
                DIRT
            } else if n < 360 {
                GRASS_BLOCK
            } else {
                COARSE_DIRT
            }
        }
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
    /// Stock behaviour: all-farmland.
    pub const fn stock() -> Self {
        FieldMix { coarse: 0, plains: 0, flower: 0, farm: 100, moss: 0, default: true }
    }

    /// Built-in meadow/grassland mix: mostly grass with wildflowers and a little bare/moss.
    pub const fn grass_auto() -> Self {
        FieldMix { coarse: 6, plains: 64, flower: 22, farm: 0, moss: 8, default: false }
    }

    /// Parse a `name=pct` list. `None`/empty/all-zero → [`FieldMix::stock`].
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
        FieldMix { coarse, plains, flower, farm, moss, default: false }
    }

    pub fn is_default(&self) -> bool {
        self.default
    }

    fn total(&self) -> u64 {
        self.coarse as u64 + self.plains as u64 + self.flower as u64 + self.farm as u64 + self.moss as u64
    }
}

impl FieldProfile {
    /// Tilled-farmland texture: tight plots, frequent tracks.
    pub fn farmland(mix: FieldMix) -> Self {
        FieldProfile { mix, sizes: [18, 30, 46], track_pct: 45, salt: 0 }
    }

    /// Meadow/grassland texture: large loose plots, few tracks. Salted so grass parcels
    /// don't line up with farmland parcels at the same coordinates.
    pub fn grass() -> Self {
        FieldProfile { mix: FieldMix::grass_auto(), sizes: [40, 80, 140], track_pct: 18, salt: 0x0000_2B57 }
    }

    /// True when this profile actually changes the surface (mix is non-stock).
    pub fn is_active(&self) -> bool {
        !self.mix.is_default()
    }

    fn parcel_size(&self, mx: i32, mz: i32) -> i32 {
        self.sizes[(coord_hash(mx ^ 0x0000_51ED ^ self.salt, mz.wrapping_mul(7)) % 3) as usize]
    }

    fn category_for_parcel(&self, px: i32, pz: i32) -> FieldCategory {
        let total = self.mix.total();
        if total == 0 {
            return FieldCategory::Farm;
        }
        let mut roll = coord_hash(px, (pz ^ 0x5F35_6495) ^ self.salt) % total;
        for (share, cat) in [
            (self.mix.coarse, FieldCategory::Coarse),
            (self.mix.plains, FieldCategory::Plains),
            (self.mix.flower, FieldCategory::Flower),
            (self.mix.farm, FieldCategory::Farm),
            (self.mix.moss, FieldCategory::Moss),
        ] {
            if roll < share as u64 {
                return cat;
            }
            roll -= share as u64;
        }
        FieldCategory::Farm
    }

    fn parcel_at(&self, x: i32, z: i32) -> (i32, i32, i32, i32, i32) {
        let s = self.salt;
        let wx = value_noise_01(x + 1000 + s, z - 500 - s, WARP_SCALE);
        let wz = value_noise_01(x - 700 - s, z + 1300 + s, WARP_SCALE);
        let sx = x + ((wx - 0.5) * 2.0 * WARP).round() as i32;
        let sz = z + ((wz - 0.5) * 2.0 * WARP).round() as i32;
        let ps = self.parcel_size(sx.div_euclid(MACRO), sz.div_euclid(MACRO));
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

    /// Full resolution of a cell: style + surface block + track flag.
    pub fn cell_at(&self, x: i32, z: i32) -> FieldCell {
        let (px, pz, ps, lx, lz) = self.parcel_at(x, z);
        let cat = self.category_for_parcel(px, pz);
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
                && coord_hash(px ^ nx, ((pz ^ nz) ^ 0x0000_7A11) ^ self.salt) % 100 < self.track_pct
            {
                is_track = true;
            }
        }
        let surface = if is_track {
            DIRT_PATH
        } else {
            surface_block(cat, x, z)
        };
        FieldCell { cat, surface, is_track }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_farmland_is_all_farm() {
        let p = FieldProfile::farmland(FieldMix::parse(None));
        assert!(!p.is_active());
        for x in -40..40 {
            for z in -40..40 {
                let c = p.cell_at(x, z);
                assert_eq!(c.cat, FieldCategory::Farm);
                assert!(!c.is_track);
                assert_eq!(c.surface, FARMLAND);
            }
        }
    }

    #[test]
    fn empty_and_zero_fall_back_to_stock() {
        assert!(FieldMix::parse(Some("")).is_default());
        assert!(FieldMix::parse(Some("plains=0,farm=0")).is_default());
    }

    #[test]
    fn grass_profile_is_active_and_grassy() {
        let p = FieldProfile::grass();
        assert!(p.is_active());
        let (mut grassy, mut farm, mut n) = (0, 0, 0);
        for x in (0..8000).step_by(17) {
            for z in (0..8000).step_by(17) {
                match p.category_at(x, z) {
                    FieldCategory::Plains | FieldCategory::Flower => grassy += 1,
                    FieldCategory::Farm => farm += 1,
                    _ => {}
                }
                n += 1;
            }
        }
        assert_eq!(farm, 0, "grass profile has no farm category");
        assert!(grassy as f64 / n as f64 > 0.7, "grass profile should be mostly grassy");
    }

    #[test]
    fn weights_roughly_match_area_share() {
        let p = FieldProfile::farmland(FieldMix::parse(Some("plains=50,farm=50")));
        let (mut plains, mut farm) = (0, 0);
        for x in (0..6000).step_by(13) {
            for z in (0..6000).step_by(13) {
                match p.category_at(x, z) {
                    FieldCategory::Plains => plains += 1,
                    FieldCategory::Farm => farm += 1,
                    _ => {}
                }
            }
        }
        let ratio = plains as f64 / (plains + farm) as f64;
        assert!(ratio > 0.4 && ratio < 0.6, "plains share {ratio} not ~0.5");
    }

    #[test]
    fn tracks_only_between_different_styles() {
        let p = FieldProfile::farmland(FieldMix::parse(Some("plains=100")));
        for x in 0..200 {
            for z in 0..200 {
                assert!(!p.cell_at(x, z).is_track);
            }
        }
    }
}
