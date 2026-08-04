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

/// A resolved cell: style, surface block, per-plot crop, growth level, and track flag.
/// Decoration keys off the surface (e.g. sunflower rows = the coarse-dirt rows).
#[derive(Clone, Copy)]
pub struct FieldCell {
    pub cat: FieldCategory,
    pub crop: Option<FarmCrop>,
    /// 0..=7 growth level, uniform within a farm parcel (a field is planted at once);
    /// mapped per crop when placed (beetroot uses 0..=3). Meaningful only for farm cells.
    pub crop_age: u8,
    /// Stable per-parcel seed; flower plots derive their 2-3 species subset from it.
    pub species_seed: u32,
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
        FarmCrops {
            weights: [40, 15, 15, 8, 12, 5, 5],
        }
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
    /// Pattern zoom: parcel dimensions scale by this percent (100 = default).
    scale_pct: u16,
    /// Map scale (blocks per metre). Parcel sizes are defined at the 0.05 (1:20)
    /// reference, so real-world parcel size stays constant across map scales.
    map_scale: f64,
}

/// A resolved parcel reference: id, dimensions, cell-local coords, the
/// orientation-domain salt that keeps neighbouring domains' parcels independent, and
/// the parcel's own world centre (terrain steering is sampled there so a plot never
/// splits its crop down the middle).
#[derive(Clone, Copy)]
struct ParcelRef {
    px: i32,
    pz: i32,
    w: i32,
    l: i32,
    lx: i32,
    lz: i32,
    dsalt: i32,
    cx: i32,
    cz: i32,
    /// This block sits on the line where two orientation domains meet.
    on_domain_edge: bool,
}

/// World seed mixed into every parcel hash, so the field layout is bound to the world
/// (`--seed`, which Meld sets identically for every cell — the layout stays a pure
/// function of world coordinates and remains identical across tile seams).
#[inline]
fn seed_salt() -> i32 {
    let s = crate::ground_generation::current_noise_seed();
    (s ^ (s >> 32)) as i32
}

const MACRO: i32 = 192;
const WARP: f64 = 4.0;
const WARP_SCALE: i32 = 24;
const SUB_SCALE: i32 = 6;
/// Half-width (blocks) of the boundary between two orientation domains. The warp that
/// makes the border meander also stretches and compresses it, so a 1-block line would
/// break up; 1 here gives a 2-3 block track that survives the distortion.
const DOMAIN_EDGE: i32 = 1;
/// Share of orientation-domain borders that carry a headland track. Below 100 so the
/// boundary network keeps some gaps instead of reading as a lattice.
const DOMAIN_TRACK_PCT: u64 = 72;

/// Field-system orientation domains: each macro cell picks one of these angles, so
/// parcel grids sit at multiple angles like real farmland (not one world-aligned grid).
const ANGLES: [(f64, f64); 6] = [
    // (sin, cos) for 0, 15, 30, 45, -15, -30 degrees
    (0.0, 1.0),
    (0.258_819, 0.965_926),
    (0.5, 0.866_025),
    (
        std::f64::consts::FRAC_1_SQRT_2,
        std::f64::consts::FRAC_1_SQRT_2,
    ),
    (-0.258_819, 0.965_926),
    (-0.5, 0.866_025),
];

/// Surface for a farm plot cell, including its interior character (noise-driven worn
/// spots, mid-plot path, sunflower rows, pumpkin mosaic).
fn farm_surface(crop: FarmCrop, x: i32, z: i32, p: &ParcelRef) -> Block {
    let n = (value_noise_01(x, z, SUB_SCALE) * 1000.0) as i32;
    // Large parcels get a worn mid-plot working path on ~45% of plots.
    let has_mid_path =
        p.w >= 30 && coord_hash(p.px ^ 0x0000_11C7, (p.pz ^ 0x0000_33B1) ^ p.dsalt) % 100 < 45;
    if has_mid_path && p.lx == p.w / 2 && !matches!(crop, FarmCrop::Sunflower) {
        return COARSE_DIRT;
    }
    match crop {
        FarmCrop::Wheat | FarmCrop::Potato | FarmCrop::Carrot | FarmCrop::Beetroot => {
            // Tilled plot with noise-driven worn spots (coarse + rooted dirt).
            if n < 38 {
                COARSE_DIRT
            } else if n < 52 {
                ROOTED_DIRT
            } else {
                FARMLAND
            }
        }
        FarmCrop::Sunflower => {
            // Planted rows on coarse dirt, packed mud between rows (stays bare — plain
            // dirt would slowly regrow grass in-game), grassy patches creeping in.
            if n < 160 {
                GRASS_BLOCK
            } else if p.lz.rem_euclid(2) == 0 {
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
            // Resting field: worked bare ground reclaimed by grass. NO farmland here —
            // bare (crop-less) farmland reverts to dirt in-game, so fallow uses the
            // locked bare palette instead and never decays.
            if n < 330 {
                COARSE_DIRT
            } else if n < 450 {
                ROOTED_DIRT
            } else if n < 540 {
                PACKED_MUD
            } else if n < 700 {
                GRASS_BLOCK
            } else {
                COARSE_DIRT
            }
        }
    }
}

/// Per-category surface for non-farm styles.
fn surface_block(cat: FieldCategory, x: i32, z: i32) -> Block {
    let n = (value_noise_01(x, z, SUB_SCALE) * 1000.0) as i32;
    match cat {
        FieldCategory::Coarse => {
            // Bare, disturbed ground: coarse dirt + grass + packed mud + rooted dirt,
            // plus locked dirt-path patches. No plain mud, and none of these regrow
            // grass in-game, so the patch stays disturbed.
            if n < 160 {
                PACKED_MUD
            } else if n < 250 {
                ROOTED_DIRT
            } else if n < 300 {
                DIRT_PATH
            } else if n < 380 {
                GRASS_BLOCK
            } else {
                COARSE_DIRT
            }
        }
        FieldCategory::Moss => {
            if n < 300 {
                GRASS_BLOCK
            } else if n < 360 {
                COARSE_DIRT
            } else if n < 400 {
                ROOTED_DIRT
            } else {
                MOSS_BLOCK
            }
        }
        // Vanilla-plains look: grass with sparse coarse-dirt patches breaking it up.
        FieldCategory::Plains => {
            if n < 35 {
                COARSE_DIRT
            } else {
                GRASS_BLOCK
            }
        }
        FieldCategory::Flower => GRASS_BLOCK,
        FieldCategory::Farm => FARMLAND,
    }
}

impl FieldMix {
    /// Stock behaviour: all-farmland.
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

    /// Built-in meadow/grassland mix.
    pub const fn grass_auto() -> Self {
        FieldMix {
            coarse: 6,
            plains: 64,
            flower: 22,
            farm: 0,
            moss: 8,
            default: false,
        }
    }

    /// Parse a `name=pct` list. `None`/empty/all-zero → [`FieldMix::stock`].
    pub fn parse(spec: Option<&str>) -> Self {
        let Some(s) = spec.map(str::trim).filter(|s| !s.is_empty()) else {
            return Self::stock();
        };
        let (mut coarse, mut plains, mut flower, mut farm, mut moss) =
            (0u16, 0u16, 0u16, 0u16, 0u16);
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

    pub fn is_default(&self) -> bool {
        self.default
    }

    fn total(&self) -> u64 {
        self.coarse as u64
            + self.plains as u64
            + self.flower as u64
            + self.farm as u64
            + self.moss as u64
    }
}

impl FieldProfile {
    /// Tilled-farmland texture: tight plots, frequent tracks, real crop plots.
    pub fn farmland(mix: FieldMix, crops: FarmCrops) -> Self {
        FieldProfile {
            mix,
            crops,
            sizes: [18, 30, 46],
            track_pct: 45,
            salt: 0,
            scale_pct: 100,
            map_scale: 0.05,
        }
    }

    /// Meadow/grassland texture: large loose plots, few tracks.
    #[allow(dead_code)] // used by tests; production callers go through grass_with
    pub fn grass() -> Self {
        Self::grass_with(FieldMix::grass_auto())
    }

    /// Grassland profile with a user-supplied mix (`--grass-mix`); a stock/absent mix
    /// falls back to the built-in grassy blend so grassland never turns all-farm.
    pub fn grass_with(mix: FieldMix) -> Self {
        let mix = if mix.is_default() {
            FieldMix::grass_auto()
        } else {
            mix
        };
        FieldProfile {
            mix,
            crops: FarmCrops::combined(),
            sizes: [40, 80, 140],
            track_pct: 18,
            salt: 0x0000_2B57,
            scale_pct: 100,
            map_scale: 0.05,
        }
    }

    /// Pattern zoom: scale all parcel dimensions by `pct` percent (clamped 25..=400).
    pub fn with_scale(mut self, pct: u16) -> Self {
        self.scale_pct = pct.clamp(25, 400);
        self
    }

    /// Bind the profile to the map scale so parcels keep their REAL-WORLD size: the
    /// base sizes are defined at the 0.05 (1:20) reference, so a 1:10 map gets
    /// parcels twice as many blocks across (same metres of land).
    pub fn with_map_scale(mut self, scale: f64) -> Self {
        if scale.is_finite() && scale > 0.0 {
            self.map_scale = scale;
        }
        self
    }

    /// True when this profile actually changes the surface (mix is non-stock).
    pub fn is_active(&self) -> bool {
        !self.mix.is_default()
    }

    fn category_for_parcel(&self, px: i32, pz: i32, dsalt: i32) -> FieldCategory {
        let total = self.mix.total();
        if total == 0 {
            return FieldCategory::Farm;
        }
        let mut roll = coord_hash(px, ((pz ^ 0x5F35_6495) ^ self.salt) ^ dsalt) % total;
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

    /// Resolve the parcel containing `(x, z)`.
    ///
    /// The map is divided into MACRO-sized orientation domains; each domain hashes to
    /// a rotation angle and a layout method — long strips (either orientation) or
    /// blocky plots — so field grids sit at multiple angles and mixed shapes, like real
    /// agricultural land. Everything stays a pure function of `(x, z)`.
    fn parcel_at(&self, x: i32, z: i32) -> ParcelRef {
        let s = self.salt;
        let seed = seed_salt();
        let wx = value_noise_01(x + 1000 + s, z - 500 - s, WARP_SCALE);
        let wz = value_noise_01(x - 700 - s, z + 1300 + s, WARP_SCALE);
        let sx = x + ((wx - 0.5) * 2.0 * WARP).round() as i32;
        let sz = z + ((wz - 0.5) * 2.0 * WARP).round() as i32;
        // Orientation domain. The lookup point is warped by a coarse noise (~28-block
        // wander on the 192 grid) so domain borders — where the angle and layout
        // change — meander organically instead of cutting along straight grid lines.
        //
        // Deliberately SHARED between profiles: no profile salt and no profile warp
        // enters the domain lookup, so farmland, grassland and untagged land sit on ONE
        // rotation/layout field. Their parcels then line up in direction and domain
        // border across a land-class boundary (they still differ in plot size and style
        // mix), instead of two unrelated grids hard-cutting where the classes meet.
        let mwx = value_noise_01(x - 4000, z + 2000, 64);
        let mwz = value_noise_01(x + 5000, z - 3000, 64);
        let dx = x + ((mwx - 0.5) * 56.0).round() as i32;
        let dz = z + ((mwz - 0.5) * 56.0).round() as i32;
        let mx = dx.div_euclid(MACRO);
        let mz = dz.div_euclid(MACRO);
        // The world seed enters here and flows into `dsalt`, hence into every parcel
        // hash below — layout, crops, growth and species are bound to the world.
        let dh = coord_hash(mx ^ 0x0000_51ED ^ seed, mz.wrapping_mul(7) ^ seed);
        let dsalt = (dh as i32) ^ (mx.wrapping_mul(0x1F12_3BB5)) ^ (mz.wrapping_mul(0x0077_F0ED));
        // Base parcel size (scaled by the pattern zoom).
        let scale_factor = self.map_scale / 0.05;
        let base = (self.sizes[(dh % 3) as usize] as f64 * scale_factor * self.scale_pct as f64
            / 100.0)
            .round() as i64;
        let base = (base as i32).clamp(6, 400);
        // Layout method: strips one way, strips the other, or blocky plots.
        let (w, l) = match (dh >> 8) % 10 {
            0..=2 => ((base * 2 / 5).max(8), (base * 12 / 5).max(16)),
            3..=5 => ((base * 12 / 5).max(16), (base * 2 / 5).max(8)),
            _ => (base, base),
        };
        // Domain rotation: align to the dominant nearby road (real fields are laid out
        // off their access roads); hashed angle where no road is near.
        let (sin_t, cos_t) =
            crate::road_bearings::bearing_at(mx * MACRO + MACRO / 2, mz * MACRO + MACRO / 2)
                .unwrap_or(ANGLES[((dh >> 16) % 6) as usize]);
        let fx = sx as f64;
        let fz = sz as f64;
        let rx = (fx * cos_t + fz * sin_t).round() as i32;
        let rz = (-fx * sin_t + fz * cos_t).round() as i32;
        let (px, pz) = (rx.div_euclid(w), rz.div_euclid(l));
        // Parcel centre, rotated back into world space: the one point every block of a
        // parcel agrees on, so terrain-driven steering resolves per PLOT, not per block.
        let (crx, crz) = ((px * w + w / 2) as f64, (pz * l + l / 2) as f64);
        // Where two orientation domains meet, the field system changes angle and a plot
        // that spans the line gets cut. Real farmland separates neighbouring field
        // systems with a track or headland, so mark the line and let `cell_at` lay a
        // dirt track along it — the cut then reads as a field boundary rather than a
        // chopped plot. Free to detect: it is the macro grid line we already crossed,
        // in the SAME warped space, so it meanders exactly like the domain border does.
        let edge_x = dx.rem_euclid(MACRO);
        let edge_z = dz.rem_euclid(MACRO);
        let on_domain_edge = edge_x <= DOMAIN_EDGE
            || edge_x >= MACRO - 1 - DOMAIN_EDGE
            || edge_z <= DOMAIN_EDGE
            || edge_z >= MACRO - 1 - DOMAIN_EDGE;
        ParcelRef {
            px,
            pz,
            w,
            l,
            lx: rx.rem_euclid(w),
            lz: rz.rem_euclid(l),
            dsalt,
            cx: (crx * cos_t - crz * sin_t).round() as i32,
            cz: (crx * sin_t + crz * cos_t).round() as i32,
            on_domain_edge,
        }
    }

    /// Category for the parcel containing `(x, z)`.
    #[allow(dead_code)] // exercised by tests; kept as the profile's public probe API
    pub fn category_at(&self, x: i32, z: i32) -> FieldCategory {
        let p = self.parcel_at(x, z);
        self.category_for_parcel(p.px, p.pz, p.dsalt)
    }

    /// Crop of the farm parcel containing `(x, z)` (None off farm parcels).
    #[allow(dead_code)] // exercised by tests; kept as the profile's public probe API
    pub fn crop_at(&self, x: i32, z: i32) -> Option<FarmCrop> {
        let p = self.parcel_at(x, z);
        if self.category_for_parcel(p.px, p.pz, p.dsalt) == FieldCategory::Farm {
            Some(self.crops.pick(p.px, p.pz ^ p.dsalt))
        } else {
            None
        }
    }

    /// Full resolution of a cell: style + crop + surface + track flag.
    pub fn cell_at(&self, x: i32, z: i32) -> FieldCell {
        let p = self.parcel_at(x, z);
        let cat = self.category_for_parcel(p.px, p.pz, p.dsalt);
        let mut is_track = false;
        if p.lx == 0 || p.lz == 0 || p.lx == p.w - 1 || p.lz == p.l - 1 {
            let (nx, nz) = if p.lx == 0 {
                (p.px - 1, p.pz)
            } else if p.lx == p.w - 1 {
                (p.px + 1, p.pz)
            } else if p.lz == 0 {
                (p.px, p.pz - 1)
            } else {
                (p.px, p.pz + 1)
            };
            if self.category_for_parcel(nx, nz, p.dsalt) != cat
                && coord_hash(
                    p.px ^ nx,
                    (((p.pz ^ nz) ^ 0x0000_7A11) ^ self.salt) ^ p.dsalt,
                ) % 100
                    < self.track_pct
            {
                is_track = true;
            }
        }
        // Headland track between two field systems. Most domain borders get one (real
        // farmland separates differently-oriented systems with a working track); the
        // rest just meet, so the network doesn't read as a perfect grid.
        if p.on_domain_edge
            && coord_hash(p.dsalt ^ 0x0000_D0E9, p.dsalt.wrapping_mul(31)) % 100 < DOMAIN_TRACK_PCT
        {
            is_track = true;
        }
        let crop = if cat == FieldCategory::Farm {
            Some(self.crops.pick(p.px, p.pz ^ p.dsalt))
        } else {
            None
        };
        // Terrain steering: sunflower fields cluster in the low, open plains and are
        // demoted on the higher ground (parcel-hash consistent, so a field stays one
        // crop; the lowland grid is coarse, so big low areas fill with sunflowers).
        // Sampled at the parcel CENTRE, not at this block: a plot lying across a
        // lowland border used to grow sunflowers in one half and wheat in the other,
        // which read as a plot cut in two.
        let crop = crop.map(|c| {
            let low = crate::lowland::is_lowland(p.cx, p.cz);
            if low
                && c != FarmCrop::Sunflower
                && coord_hash(p.px ^ 0x0051_F10A, p.pz ^ p.dsalt) % 100 < 22
            {
                FarmCrop::Sunflower
            } else if !low
                && c == FarmCrop::Sunflower
                && coord_hash(p.px ^ 0x0051_F10B, p.pz ^ p.dsalt) % 100 < 60
            {
                FarmCrop::Wheat
            } else {
                c
            }
        });
        // Growth level uniform per field (planted together), weighted toward ripe with a
        // few immature fields — so neighbouring plots read as different growth stages.
        let crop_age = if crop.is_some() {
            match coord_hash(p.px ^ 0x0000_A9E3, p.pz.wrapping_mul(29) ^ p.dsalt) % 10 {
                0..=5 => 7,
                6 => 6,
                7 => 5,
                8 => 4,
                _ => 2,
            }
        } else {
            0
        };
        // Per-parcel species seed (flower plots pick a small species subset from it).
        let species_seed = coord_hash(p.px ^ 0x0000_F10E, p.pz.wrapping_mul(53) ^ p.dsalt) as u32;
        let surface = if is_track {
            DIRT_PATH
        } else if let Some(c) = crop {
            farm_surface(c, x, z, &p)
        } else {
            surface_block(cat, x, z)
        };
        FieldCell {
            cat,
            crop,
            crop_age,
            species_seed,
            surface,
            is_track,
        }
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
        let p = FieldProfile::farmland(
            FieldMix::parse(Some("farm=100,plains=1")),
            FarmCrops::combined(),
        );
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
        // Strips can be as narrow as 8 blocks and boundaries sit at angles, so allow
        // more frequent changes than the blocky-only layout — but still far below
        // per-cell salt-and-pepper (which would be ~85% changes).
        assert!(
            changes < 2000 / 5,
            "crop changes {changes} too frequent for parcels"
        );
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
        assert!(
            grassy as f64 / n as f64 > 0.7,
            "grass profile should be mostly grassy"
        );
    }

    #[test]
    fn weights_roughly_match_area_share() {
        let p = FieldProfile::farmland(
            FieldMix::parse(Some("plains=50,farm=50")),
            FarmCrops::combined(),
        );
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

    /// Inside one field system a single-style profile has nothing to separate, so the
    /// only tracks are the headlands where two orientation domains meet.
    #[test]
    fn tracks_only_between_different_styles_or_field_systems() {
        let p = FieldProfile::farmland(FieldMix::parse(Some("plains=100")), FarmCrops::combined());
        let mut headlands = 0;
        for x in 0..400 {
            for z in 0..400 {
                if p.cell_at(x, z).is_track {
                    assert!(
                        p.parcel_at(x, z).on_domain_edge,
                        "track at ({x},{z}) is neither a style boundary nor a headland"
                    );
                    headlands += 1;
                }
            }
        }
        // Domains are 192 blocks, so a 400x400 patch must contain some of those borders
        // (and nowhere near all of it can be track).
        assert!(headlands > 0, "no headland tracks at all");
        assert!(
            headlands < 400 * 400 / 10,
            "headlands cover too much ground"
        );
    }

    /// A parcel grows ONE crop everywhere, including where terrain steering applies:
    /// the lowland probe reads the parcel centre, so a plot can never be sunflower on
    /// one side of a lowland border and wheat on the other.
    #[test]
    fn crop_is_uniform_across_a_whole_parcel() {
        let p = FieldProfile::farmland(FieldMix::parse(Some("farm=100")), FarmCrops::combined());
        // Walk a band and check every block of each parcel agrees with its parcel id.
        // A parcel is identified by its grid index AND its orientation domain: two
        // domains (different rotation/layout) can reuse the same grid index, and where
        // they meet the field systems genuinely differ — as adjacent real field systems
        // laid out off different roads do.
        let mut seen: std::collections::HashMap<(i32, i32, i32), Option<FarmCrop>> =
            std::collections::HashMap::new();
        for x in -300..300 {
            for z in -300..300 {
                let r = p.parcel_at(x, z);
                let c = p.cell_at(x, z).crop;
                match seen.entry((r.px, r.pz, r.dsalt)) {
                    std::collections::hash_map::Entry::Occupied(e) => {
                        assert_eq!(*e.get(), c, "parcel ({},{}) grows two crops", r.px, r.pz);
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(c);
                    }
                }
            }
        }
    }

    /// All three land profiles must share one orientation field, so their patterns meld
    /// where farmland meets grassland/untagged instead of hard-cutting: at any point the
    /// parcel grids agree on rotation and on which orientation domain they are in.
    #[test]
    fn profiles_share_the_orientation_field() {
        let farm = FieldProfile::farmland(FieldMix::parse(Some("farm=100")), FarmCrops::combined());
        let grass = FieldProfile::grass();
        for x in (-500..500).step_by(37) {
            for z in (-500..500).step_by(41) {
                assert_eq!(
                    farm.parcel_at(x, z).dsalt,
                    grass.parcel_at(x, z).dsalt,
                    "profiles disagree on the orientation domain at ({x},{z})"
                );
            }
        }
    }
}
