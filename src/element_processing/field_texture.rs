//! Configurable farmland texturing.
//!
//! Splits OSM `landuse=farmland` into a weighted mix of five patch categories —
//! coarse dirt, plains grass, flower plains, tilled farmland, and mossy overgrowth.
//! The category for a cell is a pure function of `(x, z)` via a per-patch uniform
//! hash, so it is identical across tile seams and each category's area fraction
//! matches its weight. When `--field-mix` is omitted the mix is `farm=100`, which
//! keeps every farmland cell tilled → byte-identical to stock arnis.

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

/// Edge length (blocks) of one patch. ~7 m plots read like real field parcels and
/// keep each category spatially coherent instead of salt-and-pepper.
const PATCH: i32 = 7;

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

    /// Category of the patch containing `(x, z)`. A uniform per-patch hash makes each
    /// category cover ≈ its weight share, with the whole patch a single category.
    pub fn category_at(&self, x: i32, z: i32) -> FieldCategory {
        let total = self.coarse as u64
            + self.plains as u64
            + self.flower as u64
            + self.farm as u64
            + self.moss as u64;
        if total == 0 {
            return FieldCategory::Farm;
        }
        let bx = x.div_euclid(PATCH);
        let bz = z.div_euclid(PATCH);
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
        // Sample distinct patches (step by PATCH) so each draw is an independent patch.
        for x in (0..700).step_by(7) {
            for z in (0..700).step_by(7) {
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
