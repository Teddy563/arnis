//! Configurable land texturing.
//!
//! Splits farmland/grassland into a weighted mix of five styles — coarse dirt, plains
//! grass, flower plains, tilled farmland, and mossy overgrowth — laid out as rectangular
//! **parcels** (like real plots from above), with dirt-track boundaries and a fine
//! internal sub-noise so each style reads as varied ground.
//!
//! Farm parcels additionally each grow **one crop** (wheat / potato / carrot / beetroot /
//! sunflower / pumpkin / fallow), weighted by [`FarmCrops`] — real monoculture plots that
//! form a crop patchwork. Plots carry interior character: worn coarse-dirt spots, a
//! mid-plot path on large parcels, sunflower rows on dirt, pumpkins on a grass/coarse
//! mosaic.
//!
//! Everything is a pure function of `(x, z)` → identical across tile seams. An inactive
//! (stock) profile reproduces the original surface exactly (byte-identical).

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

/// One farm-plot crop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FarmCrop {
    Wheat,
    Potato,
    Carrot,
    Beetroot,
    Sunflower,
    Pumpkin,
    Fallow,
}

/// A resolved cell: style, surface block, per-plot crop, and track flag. Decoration
/// keys off the surface (e.g. sunflower rows = the coarse-dirt rows).
#[derive(Clone, Copy)]
pub struct FieldCell {
    pub cat: FieldCategory,
    pub crop: Option<FarmCrop>,
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

/// Relative shares of the seven farm-plot crops.
#[derive(Clone, Copy)]
pub struct FarmCrops {
    weights: [u16; 7],
}

const CROP_ORDER: [FarmCrop; 7] = [
    FarmCrop::Wheat,
    FarmCrop::Potato,
    FarmCrop::Carrot,
    FarmCrop::Beetroot,
    FarmCrop::Sunflower,
    FarmCrop::Pumpkin,
    FarmCrop::Fallow,
];

impl FarmCrops {
    /// The default "combined" patchwork: wheat-led with the rest sprinkled in.
    pub const fn combined() -> Self {
        FarmCrops { weights: [40, 15, 15, 8, 12, 5, 5] }
    }

    /// Parse `wheat=40,potato=15,carrot=15,beetroot=8,sunflower=12,pumpkin=5,fallow=5`.
    /// `None`/empty/all-zero → [`FarmCrops::combined`].
    pub fn parse(spec: Option<&str>) -> Self {
        let Some(s) = spec.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::combined();
        };
        let mut w = [0u16; 7];
        for tok in s.split(',') {
            if let Some((k, v)) = tok.split_once('=') {
                let val: u16 = v.trim().parse().unwrap_or(0);
                let idx = match k.trim().to_ascii_lowercase().as_str() {
                    "wheat" => 0,
                    "potato" => 1,
                    "carrot" => 2,
                    "beetroot" => 3,
                    "sunflower" => 4,
                    "pumpkin" => 5,
                    "fallow" => 6,
                    _ => continue,
                };
                w[idx] = val;
            }
        }
        if w.iter().map(|&v| v as u32).sum::<u32>() == 0 {
            return Self::combined();
        }
        FarmCrops { weights: w }
    }

    fn pick(&self, px: i32, pz: i32) -> FarmCrop {
        let total: u64 = self.weights.iter().map(|&v| v as u64).sum();
        if total == 0 {
            return FarmCrop::Wheat;
        }
        // Distinct stream from the category roll so crop and style don't correlate.
        let mut roll = coord_hash(px ^ 0x0000_C0FE, pz.wrapping_mul(13)) % total;
        for (i, &w) in self.weights.iter().enumerate() {
            if roll < w as u64 {
                return CROP_ORDER[i];
            }
            roll -= w as u64;
        }
        FarmCrop::Wheat
    }
}

/// A land-kind texture: a mix plus parcel-size band, track probability, and crops.
#[derive(Clone, Copy)]
pub struct FieldProfile {
    mix: FieldMix,
    crops: FarmCrops,
    sizes: [i32; 3],
    track_pct: u64,
    salt: i32,
}

const MACRO: i32 = 160;
const WARP: f64 = 4.0;
const WARP_SCALE: i32 = 24;
const SUB_SCALE: i32 = 6;

/// Surface for a farm plot cell, including its interior character (worn spots,
/// mid-plot path, sunflower rows, pumpkin mosaic).
#[allow(clippy::too_many_arguments)]
fn farm_surface(crop: FarmCrop, x: i32, z: i32, lx: i32, lz: i32, ps: i32, px: i32, pz: i32) -> Block {
    let n = (value_noise_01(x, z, SUB_SCALE) * 1000.0) as i32;
    // Large parcels get a worn mid-plot working path on ~45% of plots.
    let has_mid_path = ps >= 30 && coord_hash(px ^ 0x0000_11C7, pz ^ 0x0000_33B1) % 100 < 45;
    if has_mid_path && lx == ps / 2 && !matches!(crop, FarmCrop::Sunflower) {
        return COARSE_DIRT;
    }
    match crop {
        FarmCrop::Wheat | FarmCrop::Potato | FarmCrop::Carrot | FarmCrop::Beetroot => {
            // Tilled plot with occasional worn coarse spots.
            if n < 40 {
                COARSE_DIRT
            } else {
                FARMLAND
            }
        }
        FarmCrop::Sunflower => {
            // Planted rows on coarse dirt, packed mud between rows (stays bare — plain
            // dirt would slowly regrow grass in-game), grassy patches creeping in.
            if n < 160 {
                GRASS_BLOCK
            } else if lz.rem_euclid(2) == 0 {
                COARSE_DIRT
            } else {
                PACKED_MUD
            }
        }
        FarmCrop::Pumpkin => {
            // Pumpkin patch: grass/coarse mosaic, not tilled.
            if n < 420 {
                COARSE_DIRT
            } else {
                GRASS_BLOCK
            }
        }
        FarmCrop::Fallow => {
            // Resting field: tilled ground breaking up into coarse patches.
            if n < 250 {
                COARSE_DIRT
            } else {
                FARMLAND
            }
        }
    }
}

/// Per-category surface for non-farm styles.
fn surface_block(cat: FieldCategory, x: i32, z: i32) -> Block {
    let n = (value_noise_01(x, z, SUB_SCALE) * 1000.0) as i32;
    match cat {
        FieldCategory::Coarse => {
            // Bare, disturbed ground. Packed mud (not plain dirt) for the barest
            // patches so they can't regrow grass in-game, with mud lows, rooted-dirt
            // texture, and a little grass peeking through.
            if n < 210 {
                PACKED_MUD
            } else if n < 260 {
                MUD
            } else if n < 320 {
                ROOTED_DIRT
            } else if n < 380 {
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

    /// Built-in meadow/grassland mix.
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
    /// Tilled-farmland texture: tight plots, frequent tracks, real crop plots.
    pub fn farmland(mix: FieldMix, crops: FarmCrops) -> Self {
        FieldProfile { mix, crops, sizes: [18, 30, 46], track_pct: 45, salt: 0 }
    }

    /// Meadow/grassland texture: large loose plots, few tracks.
    pub fn grass() -> Self {
        FieldProfile {
            mix: FieldMix::grass_auto(),
            crops: FarmCrops::combined(),
            sizes: [40, 80, 140],
            track_pct: 18,
            salt: 0x0000_2B57,
        }
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
    #[allow(dead_code)] // exercised by tests; kept as the profile's public probe API
    pub fn category_at(&self, x: i32, z: i32) -> FieldCategory {
        let (px, pz, ..) = self.parcel_at(x, z);
        self.category_for_parcel(px, pz)
    }

    /// Crop of the farm parcel containing `(x, z)` (None off farm parcels).
    #[allow(dead_code)] // exercised by tests; kept as the profile's public probe API
    pub fn crop_at(&self, x: i32, z: i32) -> Option<FarmCrop> {
        let (px, pz, ..) = self.parcel_at(x, z);
        if self.category_for_parcel(px, pz) == FieldCategory::Farm {
            Some(self.crops.pick(px, pz))
        } else {
            None
        }
    }

    /// Full resolution of a cell: style + crop + surface + track flag.
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
        let crop = if cat == FieldCategory::Farm {
            Some(self.crops.pick(px, pz))
        } else {
            None
        };
        let surface = if is_track {
            DIRT_PATH
        } else if let Some(c) = crop {
            farm_surface(c, x, z, lx, lz, ps, px, pz)
        } else {
            surface_block(cat, x, z)
        };
        FieldCell { cat, crop, surface, is_track }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_farmland_is_all_farm() {
        let p = FieldProfile::farmland(FieldMix::parse(None), FarmCrops::parse(None));
        assert!(!p.is_active());
        // NOTE: with a stock mix callers never invoke cell_at (field_cell is None),
        // so stock output stays byte-identical regardless of crop weighting.
        for x in -40..40 {
            for z in -40..40 {
                assert_eq!(p.category_at(x, z), FieldCategory::Farm);
            }
        }
    }

    #[test]
    fn empty_and_zero_fall_back_to_stock() {
        assert!(FieldMix::parse(Some("")).is_default());
        assert!(FieldMix::parse(Some("plains=0,farm=0")).is_default());
    }

    #[test]
    fn farm_parcels_are_monoculture_and_diverse() {
        let p = FieldProfile::farmland(FieldMix::parse(Some("farm=100,plains=1")), FarmCrops::combined());
        // Monoculture: a cell and its immediate neighbour agree on the crop unless a
        // parcel boundary sits between them — so over a straight walk, crop changes
        // must be far rarer than cells (parcels are >= 18 wide).
        let mut changes = 0;
        let mut prev = p.cell_at(0, 500).crop;
        for x in 1..2000 {
            let c = p.cell_at(x, 500).crop;
            if c != prev {
                changes += 1;
                prev = c;
            }
        }
        assert!(changes < 2000 / 12, "crop changes {changes} too frequent for parcels");
        // Diversity: all 7 crops appear over a wide area.
        let mut seen = std::collections::HashSet::new();
        for x in (0..8000).step_by(11) {
            for z in (0..8000).step_by(11) {
                if let Some(c) = p.crop_at(x, z) {
                    seen.insert(format!("{c:?}"));
                }
            }
        }
        assert!(seen.len() == 7, "all crops should appear, saw {seen:?}");
    }

    #[test]
    fn crop_shares_follow_weights() {
        let p = FieldProfile::farmland(
            FieldMix::parse(Some("farm=100,plains=1")),
            FarmCrops::parse(Some("wheat=50,potato=50")),
        );
        let (mut wheat, mut potato, mut other) = (0u32, 0u32, 0u32);
        for x in (0..9000).step_by(17) {
            for z in (0..9000).step_by(17) {
                match p.crop_at(x, z) {
                    Some(FarmCrop::Wheat) => wheat += 1,
                    Some(FarmCrop::Potato) => potato += 1,
                    Some(_) => other += 1,
                    None => {}
                }
            }
        }
        assert_eq!(other, 0, "only weighted crops appear");
        let ratio = wheat as f64 / (wheat + potato) as f64;
        assert!(ratio > 0.38 && ratio < 0.62, "wheat share {ratio} not ~0.5");
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
        let p = FieldProfile::farmland(FieldMix::parse(Some("plains=50,farm=50")), FarmCrops::combined());
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
        let p = FieldProfile::farmland(FieldMix::parse(Some("plains=100")), FarmCrops::combined());
        for x in 0..200 {
            for z in 0..200 {
                assert!(!p.cell_at(x, z).is_track);
            }
        }
    }
}
