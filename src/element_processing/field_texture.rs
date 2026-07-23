//! Configurable farmland texturing.
//!
//! Splits OSM `landuse=farmland` into a weighted mix of five patch categories —
//! coarse dirt, plains grass, flower plains, tilled farmland, and mossy overgrowth.
//! The category for a cell is a pure function of `(x, z)` via a per-patch uniform
//! hash, so it is identical across tile seams and each category's area fraction
//! matches its weight. When `--field-mix` is omitted the mix is `farm=100`, which
//! keeps every farmland cell tilled → byte-identical to stock arnis.

use crate::ground_generation::value_noise_01;
use crate::land_cover::coord_hash;

/// One farmland patch style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldCategory {
    /// Bare, disturbed ground: coarse dirt with the odd dead bush.
    Coarse,
    /// Open plains grass.
    Plains,
    /// Grass sprinkled with wildflowers.
    Flower,
    /// Stock tilled farmland with crops (unchanged behaviour).
    Farm,
    /// Overgrown mossy patch.
    Moss,
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

/// Blob size (blocks). One category fills a whole ~24-block blob, so the map reads as
/// coherent fields of texture rather than per-block static.
const BLOB: i32 = 24;
/// Max block offset applied to the sample point before quantising to a blob, warping
/// the otherwise-square blob edges into organic wandering boundaries.
const WARP: f64 = 14.0;
/// Lattice period of the warp noise.
const WARP_SCALE: i32 = 40;

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
    /// Order and subset are free; unknown keys are ignored. `None`, empty, or an
    /// all-zero spec falls back to [`FieldMix::stock`] so a default (or misconfigured)
    /// run stays byte-identical.
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

    /// Category for the blob containing `(x, z)`. The sample point is domain-warped and
    /// quantised to a large blob, then a uniform per-blob hash picks the category — so
    /// each category forms big coherent blobs (not per-block static) while still covering
    /// ≈ its weight share of the area.
    pub fn category_at(&self, x: i32, z: i32) -> FieldCategory {
        let total = self.coarse as u64
            + self.plains as u64
            + self.flower as u64
            + self.farm as u64
            + self.moss as u64;
        if total == 0 {
            return FieldCategory::Farm;
        }
        // Warp the point with two decorrelated noise fields so blob edges wander.
        let wx = value_noise_01(x + 1000, z - 500, WARP_SCALE);
        let wz = value_noise_01(x - 700, z + 1300, WARP_SCALE);
        let sx = x + ((wx - 0.5) * 2.0 * WARP).round() as i32;
        let sz = z + ((wz - 0.5) * 2.0 * WARP).round() as i32;
        let bx = sx.div_euclid(BLOB);
        let bz = sz.div_euclid(BLOB);
        // Salt keeps this stream distinct from the prop/scatter coord_hash uses.
        let mut roll = coord_hash(bx, bz ^ 0x5F35_6495) % total;
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_farm() {
        let m = FieldMix::parse(None);
        assert!(m.is_default());
        for x in -20..20 {
            for z in -20..20 {
                assert_eq!(m.category_at(x, z), FieldCategory::Farm);
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
        // Sample a wide area (step across blobs) so the law of large numbers applies.
        for x in (0..3000).step_by(11) {
            for z in (0..3000).step_by(11) {
                match m.category_at(x, z) {
                    FieldCategory::Plains => plains += 1,
                    FieldCategory::Farm => farm += 1,
                    _ => other += 1,
                }
            }
        }
        assert_eq!(other, 0, "no weight assigned to other categories");
        let ratio = plains as f64 / (plains + farm) as f64;
        assert!(ratio > 0.4 && ratio < 0.6, "plains share {ratio} not ~0.5");
    }
}
