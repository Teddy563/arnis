//! Farmland parcels: one crop per plot, road-aligned field systems, style presets.
//!
//! `landuse=farmland` renders by default as one uniform sheet of mixed crops. Real
//! farmland is a patchwork of separate plots, each worked on its own and each growing
//! one thing. With `--field-mix` (or `--farm-crops`) this module splits farmland into
//! rectangular **parcels** separated by dirt tracks, with a fine sub-noise so each
//! parcel reads as varied ground. Every farm parcel grows exactly **one crop** (wheat,
//! potato, carrot, beetroot, sunflower, pumpkin or fallow) at one growth stage. Plots
//! carry interior character: worn coarse-dirt spots, a mid-plot working path on large
//! parcels, sunflower rows on dirt, a pumpkin patch on a grass and coarse mosaic.
//!
//! Parcel grids sit in 192-block orientation domains, rotated to the dominant nearby
//! road where there is one (see `road_bearings`) and to one of six hashed angles where
//! there is not, so plots do not all snap to the world axes.
//!
//! Layout is a pure function of `(x, z)`: no per-run state, no RNG, so tiles, One World
//! work units and separate runs of overlapping areas agree. Decoration uses the
//! element's own RNG like the rest of `landuse`.

use crate::block_definitions::*;
use crate::ground_generation::value_noise_01;
use crate::land_cover::coord_hash;
use crate::world_editor::WorldEditor;
use rand::Rng;
use std::sync::{Arc, OnceLock};

/// Farmland parcel options. Off unless asked for, so the GUI runs on
/// `FieldArgs::default()`.
#[derive(clap::Args, Debug, Clone)]
pub struct FieldArgs {
    /// Lay farmland out as parcels, one crop per plot. A preset: smallholding (many
    /// small plots, full crop variety), patchwork (balanced mixed farmland), prairie
    /// (large wheat-led fields) or pasture (grass and wildflowers with a few crop
    /// plots); or a share list over coarse, plains, flower, farm and moss, e.g.
    /// `farm=75,coarse=10,plains=8,moss=5,flower=2`. classic, or leaving the flag out,
    /// keeps the uniform crop sheet. Parcels cost about 15% more time and 1.5x the peak
    /// memory on farmland-heavy areas.
    #[arg(long, value_name = "PRESET|LIST", value_parser = FieldMix::parse)]
    pub field_mix: Option<FieldMix>,

    /// Crop shares for farm parcels over wheat, potato, carrot, beetroot, sunflower,
    /// pumpkin and fallow, e.g. `wheat=60,sunflower=20,fallow=20`. Each parcel grows one.
    /// Without --field-mix this lays farmland out as crop plots only.
    #[arg(long, value_name = "LIST", value_parser = FarmCrops::parse)]
    pub farm_crops: Option<FarmCrops>,

    /// Parcel size in percent of the preset's (25 to 400). Sizes are metres, so they
    /// also follow --scale.
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(25..=400))]
    pub field_scale: u16,
}

impl Default for FieldArgs {
    fn default() -> Self {
        Self {
            field_mix: None,
            farm_crops: None,
            field_scale: 100,
        }
    }
}

/// One parcel style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldCategory {
    Coarse,
    Plains,
    Flower,
    Farm,
    Moss,
}

const CATEGORY_ORDER: [FieldCategory; 5] = [
    FieldCategory::Coarse,
    FieldCategory::Plains,
    FieldCategory::Flower,
    FieldCategory::Farm,
    FieldCategory::Moss,
];
const CATEGORY_KEYS: [&str; 5] = ["coarse", "plains", "flower", "farm", "moss"];

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

const CROP_ORDER: [FarmCrop; 7] = [
    FarmCrop::Wheat,
    FarmCrop::Potato,
    FarmCrop::Carrot,
    FarmCrop::Beetroot,
    FarmCrop::Sunflower,
    FarmCrop::Pumpkin,
    FarmCrop::Fallow,
];
const CROP_KEYS: [&str; 7] = [
    "wheat",
    "potato",
    "carrot",
    "beetroot",
    "sunflower",
    "pumpkin",
    "fallow",
];

/// Parse a `key=weight` list into `N` weights in `keys` order. Unknown keys, bad
/// numbers and an all-zero list are errors, so a typo cannot silently change a world.
fn parse_weights<const N: usize>(s: &str, keys: &[&str; N]) -> Result<[u16; N], String> {
    let mut w = [0u16; N];
    for tok in s.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let (k, v) = tok
            .split_once('=')
            .ok_or_else(|| format!("{tok}: expected key=weight"))?;
        let k = k.trim().to_ascii_lowercase();
        let i = keys
            .iter()
            .position(|&key| key == k)
            .ok_or_else(|| format!("{k}: expected one of {}", keys.join(", ")))?;
        w[i] = v
            .trim()
            .parse()
            .map_err(|_| format!("{tok}: weight must be 0 to 65535"))?;
    }
    if w.iter().all(|&v| v == 0) {
        return Err(format!("{s}: give at least one non-zero weight"));
    }
    Ok(w)
}

/// Relative shares of the seven farm-plot crops, in `CROP_ORDER`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FarmCrops([u16; 7]);

impl FarmCrops {
    pub fn parse(s: &str) -> Result<Self, String> {
        parse_weights(s, &CROP_KEYS).map(FarmCrops)
    }

    fn pick(&self, px: i32, pz: i32) -> FarmCrop {
        let total: u64 = self.0.iter().map(|&v| v as u64).sum();
        // Distinct stream from the category roll so crop and style do not correlate.
        let mut roll = coord_hash(px ^ 0x0000_C0FE, pz.wrapping_mul(13)) % total.max(1);
        for (i, &w) in self.0.iter().enumerate() {
            if roll < w as u64 {
                return CROP_ORDER[i];
            }
            roll -= w as u64;
        }
        FarmCrop::Wheat
    }
}

/// A farmland style: category shares, parcel-size band (metres), track probability
/// between differing parcels, and crop shares. All-zero shares is classic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FieldMix {
    shares: [u16; 5],
    sizes: [i32; 3],
    track_pct: u64,
    crops: FarmCrops,
}

impl FieldMix {
    const CLASSIC: Self = Self::preset([0; 5], [18, 30, 46], 0, [100, 0, 0, 0, 0, 0, 0]);
    const SMALLHOLDING: Self = Self::preset(
        [12, 10, 4, 70, 4],
        [10, 16, 24],
        60,
        [22, 18, 18, 14, 12, 10, 6],
    );
    const PATCHWORK: Self = Self::preset(
        [10, 8, 2, 75, 5],
        [18, 30, 46],
        45,
        [40, 15, 15, 8, 12, 5, 5],
    );
    const PRAIRIE: Self =
        Self::preset([6, 6, 1, 85, 2], [46, 78, 120], 22, [62, 8, 6, 4, 12, 2, 6]);
    const PASTURE: Self = Self::preset(
        [6, 58, 24, 6, 6],
        [40, 80, 140],
        18,
        [45, 10, 10, 5, 20, 5, 5],
    );
    /// `--farm-crops` alone: patchwork plots, all of them crops.
    const FARM_ONLY: Self = Self::preset(
        [0, 0, 0, 100, 0],
        [18, 30, 46],
        45,
        [40, 15, 15, 8, 12, 5, 5],
    );

    const fn preset(shares: [u16; 5], sizes: [i32; 3], track_pct: u64, crops: [u16; 7]) -> Self {
        FieldMix {
            shares,
            sizes,
            track_pct,
            crops: FarmCrops(crops),
        }
    }

    /// A preset name, or a share list on patchwork's parcel sizes and crops.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "classic" => Ok(Self::CLASSIC),
            "smallholding" => Ok(Self::SMALLHOLDING),
            "patchwork" => Ok(Self::PATCHWORK),
            "prairie" => Ok(Self::PRAIRIE),
            "pasture" => Ok(Self::PASTURE),
            _ => Ok(FieldMix {
                shares: parse_weights(s, &CATEGORY_KEYS)?,
                ..Self::PATCHWORK
            }),
        }
    }
}

/// Writes `key=weight` pairs that `parse_weights` reads back to the same weights.
fn write_weights(f: &mut std::fmt::Formatter, weights: &[u16], keys: &[&str]) -> std::fmt::Result {
    for (i, (k, w)) in keys.iter().zip(weights).enumerate() {
        write!(f, "{}{k}={w}", if i == 0 { "" } else { "," })?;
    }
    Ok(())
}

/// The `--farm-crops` value for these shares, as a piece's command line needs it.
impl std::fmt::Display for FarmCrops {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write_weights(f, &self.0, &CROP_KEYS)
    }
}

/// The `--field-mix` value that parses back to this mix: a preset's name, else
/// the share list (a list always sits on patchwork's sizes and crops).
impl std::fmt::Display for FieldMix {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let presets = [
            (Self::CLASSIC, "classic"),
            (Self::SMALLHOLDING, "smallholding"),
            (Self::PATCHWORK, "patchwork"),
            (Self::PRAIRIE, "prairie"),
            (Self::PASTURE, "pasture"),
        ];
        match presets.iter().find(|(mix, _)| mix == self) {
            Some((_, name)) => f.write_str(name),
            None => write_weights(f, &self.shares, &CATEGORY_KEYS),
        }
    }
}

/// A resolved cell: style, surface block, per-plot crop, growth level, and track flag.
/// Decoration keys off the surface (for example sunflower rows are the coarse-dirt rows).
#[derive(Clone, Copy)]
pub struct FieldCell {
    pub cat: FieldCategory,
    pub crop: Option<FarmCrop>,
    /// 0..=7 growth level, uniform within a farm parcel (a field is planted at once).
    /// Mapped per crop when placed, since beetroot only reaches age 3.
    pub crop_age: u8,
    /// Stable per-parcel seed; flower plots derive their 2-3 species subset from it.
    pub species_seed: u32,
    pub surface: Block,
    pub is_track: bool,
}

/// The farmland texture a run asked for.
#[derive(Clone, Copy)]
pub struct FieldProfile {
    mix: FieldMix,
    /// Blocks per preset metre: `--scale` times `--field-scale`.
    size_factor: f64,
}

/// A resolved parcel reference: grid id, dimensions, cell-local coordinates and the
/// orientation-domain salt that keeps neighbouring domains' parcels independent.
#[derive(Clone, Copy)]
struct ParcelRef {
    px: i32,
    pz: i32,
    w: i32,
    l: i32,
    lx: i32,
    lz: i32,
    dsalt: i32,
    /// This block sits on the line where two orientation domains meet.
    on_domain_edge: bool,
}

/// Orientation-domain edge length in blocks.
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

/// Fallback field-system orientations, used where no road is near enough to define one.
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

/// Surface for a farm plot cell, including its interior character: noise-driven worn
/// spots, mid-plot working path, sunflower rows, pumpkin mosaic.
fn farm_surface(crop: FarmCrop, x: i32, z: i32, p: &ParcelRef) -> Block {
    let n = (value_noise_01(x, z, SUB_SCALE) * 1000.0) as i32;
    // Large parcels get a worn mid-plot working path on roughly 45% of plots.
    let has_mid_path =
        p.w >= 30 && coord_hash(p.px ^ 0x0000_11C7, (p.pz ^ 0x0000_33B1) ^ p.dsalt) % 100 < 45;
    if has_mid_path && p.lx == p.w / 2 && crop != FarmCrop::Sunflower {
        return COARSE_DIRT;
    }
    match crop {
        FarmCrop::Wheat | FarmCrop::Potato | FarmCrop::Carrot | FarmCrop::Beetroot => {
            if n < 38 {
                COARSE_DIRT
            } else if n < 52 {
                ROOTED_DIRT
            } else {
                FARMLAND
            }
        }
        // Planted rows on coarse dirt, packed mud between them; grass creeps in at the
        // low end of the noise.
        FarmCrop::Sunflower => {
            if n < 160 {
                GRASS_BLOCK
            } else if p.lz.rem_euclid(2) == 0 {
                COARSE_DIRT
            } else {
                PACKED_MUD
            }
        }
        FarmCrop::Pumpkin => {
            if n < 420 {
                COARSE_DIRT
            } else {
                GRASS_BLOCK
            }
        }
        // Resting field: bare worked ground being reclaimed by grass. No farmland,
        // since bare farmland with nothing on it reverts to dirt in-game.
        FarmCrop::Fallow => {
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

/// Per-category surface for the non-farm styles.
fn surface_block(cat: FieldCategory, x: i32, z: i32) -> Block {
    let n = (value_noise_01(x, z, SUB_SCALE) * 1000.0) as i32;
    match cat {
        FieldCategory::Coarse => {
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

impl FieldProfile {
    /// The profile `--field-mix` / `--farm-crops` ask for, or None for the uniform
    /// crop sheet (flags absent or classic), in which case callers skip the field pass.
    pub fn from_args(args: &FieldArgs, map_scale: f64) -> Option<Self> {
        let mut mix = match (args.field_mix, args.farm_crops) {
            (Some(mix), _) => mix,
            (None, Some(_)) => FieldMix::FARM_ONLY,
            (None, None) => return None,
        };
        if mix.shares == [0; 5] {
            return None;
        }
        if let Some(crops) = args.farm_crops {
            mix.crops = crops;
        }
        let map_scale = if map_scale.is_finite() && map_scale > 0.0 {
            map_scale
        } else {
            1.0
        };
        Some(FieldProfile {
            mix,
            size_factor: map_scale * args.field_scale as f64 / 100.0,
        })
    }

    fn category_for_parcel(&self, px: i32, pz: i32, dsalt: i32) -> FieldCategory {
        let total: u64 = self.mix.shares.iter().map(|&v| v as u64).sum();
        let mut roll = coord_hash(px, (pz ^ 0x5F35_6495) ^ dsalt) % total.max(1);
        for (&share, cat) in self.mix.shares.iter().zip(CATEGORY_ORDER) {
            if roll < share as u64 {
                return cat;
            }
            roll -= share as u64;
        }
        FieldCategory::Farm
    }

    /// Resolve the parcel containing `(x, z)`.
    ///
    /// Each MACRO-sized orientation domain takes its rotation from the dominant nearby
    /// road, or from a hash where no road is close, and hashes to a layout method: long
    /// strips either way, or blocky plots.
    fn parcel_at(&self, x: i32, z: i32) -> ParcelRef {
        let wx = value_noise_01(x + 1000, z - 500, WARP_SCALE);
        let wz = value_noise_01(x - 700, z + 1300, WARP_SCALE);
        let sx = x + ((wx - 0.5) * 2.0 * WARP).round() as i32;
        let sz = z + ((wz - 0.5) * 2.0 * WARP).round() as i32;
        // Orientation domain. The lookup point wanders by up to 28 blocks on a coarse
        // noise, so domain borders meander instead of cutting along grid lines.
        let mwx = value_noise_01(x - 4000, z + 2000, 64);
        let mwz = value_noise_01(x + 5000, z - 3000, 64);
        let dx = x + ((mwx - 0.5) * 56.0).round() as i32;
        let dz = z + ((mwz - 0.5) * 56.0).round() as i32;
        let mx = dx.div_euclid(MACRO);
        let mz = dz.div_euclid(MACRO);
        let dh = coord_hash(mx ^ 0x0000_51ED, mz.wrapping_mul(7));
        let dsalt = (dh as i32) ^ (mx.wrapping_mul(0x1F12_3BB5)) ^ (mz.wrapping_mul(0x0077_F0ED));
        let base = (self.mix.sizes[(dh % 3) as usize] as f64 * self.size_factor).round();
        let base = (base as i32).clamp(6, 400);
        // Layout method: strips one way, strips the other, or blocky plots.
        let (w, l) = match (dh >> 8) % 10 {
            0..=2 => ((base * 2 / 5).max(8), (base * 12 / 5).max(16)),
            3..=5 => ((base * 12 / 5).max(16), (base * 2 / 5).max(8)),
            _ => (base, base),
        };
        let (sin_t, cos_t) =
            crate::road_bearings::bearing_at(mx * MACRO + MACRO / 2, mz * MACRO + MACRO / 2)
                .unwrap_or(ANGLES[((dh >> 16) % 6) as usize]);
        let (fx, fz) = (sx as f64, sz as f64);
        let rx = (fx * cos_t + fz * sin_t).round() as i32;
        let rz = (-fx * sin_t + fz * cos_t).round() as i32;
        // Where two domains meet a plot spanning the line gets cut; mark the line so
        // `cell_at` can lay a headland track along it.
        let (edge_x, edge_z) = (dx.rem_euclid(MACRO), dz.rem_euclid(MACRO));
        let on_domain_edge = edge_x <= DOMAIN_EDGE
            || edge_x >= MACRO - 1 - DOMAIN_EDGE
            || edge_z <= DOMAIN_EDGE
            || edge_z >= MACRO - 1 - DOMAIN_EDGE;
        ParcelRef {
            px: rx.div_euclid(w),
            pz: rz.div_euclid(l),
            w,
            l,
            lx: rx.rem_euclid(w),
            lz: rz.rem_euclid(l),
            dsalt,
            on_domain_edge,
        }
    }

    /// Full resolution of a cell: style, crop, surface, and track flag.
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
                && coord_hash(p.px ^ nx, ((p.pz ^ nz) ^ 0x0000_7A11) ^ p.dsalt) % 100
                    < self.mix.track_pct
            {
                is_track = true;
            }
        }
        if p.on_domain_edge
            && coord_hash(p.dsalt ^ 0x0000_D0E9, p.dsalt.wrapping_mul(31)) % 100 < DOMAIN_TRACK_PCT
        {
            is_track = true;
        }
        let crop = (cat == FieldCategory::Farm).then(|| self.mix.crops.pick(p.px, p.pz ^ p.dsalt));
        // Growth level, uniform per field since a field is planted together; mostly
        // ripe with a few younger fields.
        let crop_age = match coord_hash(p.px ^ 0x0000_A9E3, p.pz.wrapping_mul(29) ^ p.dsalt) % 10 {
            _ if crop.is_none() => 0,
            0..=5 => 7,
            6 => 6,
            7 => 5,
            8 => 4,
            _ => 2,
        };
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

/// Flower species a flower parcel draws from; each parcel uses a 2-3 species subset.
const FIELD_FLOWERS: [Block; 8] = [
    RED_FLOWER,
    YELLOW_FLOWER,
    ALLIUM,
    CORNFLOWER,
    LILY_OF_THE_VALLEY,
    ORANGE_TULIP,
    PINK_TULIP,
    OXEYE_DAISY,
];

/// Highest `age` any vanilla crop uses (wheat/carrots/potatoes; beetroot stops at 3).
const MAX_CROP_AGE: u8 = 7;

/// The `age` NBT compound for each crop age, built once. A whole field shares one of
/// eight compounds, so placing a crop is a refcount bump; building one per block cost
/// roughly 3.8 GB of peak RSS on a farmland-dense 3.3 x 4.7 km bbox.
fn crop_age_props(age: u8) -> Arc<fastnbt::Value> {
    static AGES: OnceLock<Vec<Arc<fastnbt::Value>>> = OnceLock::new();
    let table = AGES.get_or_init(|| {
        (0..=MAX_CROP_AGE)
            .map(|a| {
                let props = std::collections::HashMap::from([(
                    "age".to_string(),
                    fastnbt::Value::String(a.to_string()),
                )]);
                Arc::new(fastnbt::Value::Compound(props))
            })
            .collect()
    });
    Arc::clone(&table[age.min(MAX_CROP_AGE) as usize])
}

/// Place a crop at the plot's growth level (0..=7), mapped to the crop's own max age.
fn place_crop(editor: &mut WorldEditor, base: Block, growth: u8, max_age: u8, x: i32, z: i32) {
    let age = (growth as u32 * max_age as u32 / 7) as u8;
    let bwp = BlockWithProperties::from_arc(base, Some(crop_age_props(age)));
    let ay = editor.get_absolute_y(x, 1, z);
    editor.set_block_with_properties_absolute(bwp, x, ay, z, None, None);
}

fn place_tall(editor: &mut WorldEditor, lower: Block, upper: Block, x: i32, z: i32) {
    editor.set_block(lower, x, 1, z, None, None);
    editor.set_block(upper, x, 2, z, None, None);
}

/// Ground cover on grass. Style 0 is plain grass, 1 adds occasional tall grass, 2 is
/// the fuller mix with ferns.
fn place_grass_cover(editor: &mut WorldEditor, x: i32, z: i32, rng: &mut impl Rng, style: u8) {
    match style {
        0 => editor.set_block(GRASS, x, 1, z, None, None),
        1 if rng.random_range(0..22) == 0 => {
            place_tall(editor, TALL_GRASS_BOTTOM, TALL_GRASS_TOP, x, z)
        }
        1 => editor.set_block(GRASS, x, 1, z, None, None),
        _ => match rng.random_range(0..24) {
            0..=1 => editor.set_block(FERN, x, 1, z, None, None),
            2 => place_tall(editor, LARGE_FERN_LOWER, LARGE_FERN_UPPER, x, z),
            3 => place_tall(editor, TALL_GRASS_BOTTOM, TALL_GRASS_TOP, x, z),
            _ => editor.set_block(GRASS, x, 1, z, None, None),
        },
    }
}

/// Pick from the parcel's 2-3 flower species. Each slot reads a different bit window
/// of the seed, so the subset is varied rather than collapsing onto one species.
fn parcel_flower(cell: &FieldCell, rng: &mut impl Rng) -> Block {
    let slot = rng.random_range(0..2 + cell.species_seed % 2);
    FIELD_FLOWERS[((cell.species_seed >> (5 * slot + 3)) % FIELD_FLOWERS.len() as u32) as usize]
}

/// Decorate one farmland cell according to its parcel style. Tracks stay clear.
pub fn decorate(editor: &mut WorldEditor, cell: &FieldCell, x: i32, z: i32, rng: &mut impl Rng) {
    if cell.is_track {
        return;
    }
    let on = |editor: &WorldEditor, blocks: &[Block]| editor.check_for_block(x, 0, z, Some(blocks));
    match cell.cat {
        FieldCategory::Farm => decorate_farm_plot(editor, cell, x, z, rng),
        FieldCategory::Plains => {
            if on(editor, &[GRASS_BLOCK]) {
                let style = ((cell.species_seed >> 16) % 3) as u8;
                match rng.random_range(0..1000) {
                    0..=780 => place_grass_cover(editor, x, z, rng, style),
                    781..=795 => {
                        let f = parcel_flower(cell, rng);
                        editor.set_block(f, x, 1, z, None, None);
                    }
                    796..=799 => place_tall(editor, SUNFLOWER_LOWER, SUNFLOWER_UPPER, x, z),
                    _ => {}
                }
            } else if on(editor, &[COARSE_DIRT]) && rng.random_range(0..100) < 30 {
                editor.set_block(GRASS, x, 1, z, None, None);
            }
        }
        FieldCategory::Flower => {
            if on(editor, &[GRASS_BLOCK]) {
                match rng.random_range(0..1000) {
                    0..=105 => {
                        let f = parcel_flower(cell, rng);
                        editor.set_block(f, x, 1, z, None, None);
                    }
                    106..=117 => place_tall(editor, SUNFLOWER_LOWER, SUNFLOWER_UPPER, x, z),
                    118..=580 => place_grass_cover(editor, x, z, rng, 1),
                    _ => {}
                }
            }
        }
        FieldCategory::Coarse => {
            if on(editor, &[COARSE_DIRT, ROOTED_DIRT]) {
                match rng.random_range(0..100) {
                    0..=11 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    12..=17 => editor.set_block(FERN, x, 1, z, None, None),
                    18..=24 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            } else if on(editor, &[GRASS_BLOCK]) {
                match rng.random_range(0..100) {
                    0..=5 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    6..=28 => place_grass_cover(editor, x, z, rng, 2),
                    _ => {}
                }
            }
        }
        FieldCategory::Moss => {
            if on(editor, &[MOSS_BLOCK, GRASS_BLOCK]) {
                match rng.random_range(0..100) {
                    0..=3 => editor.set_block(AZALEA, x, 1, z, None, None),
                    4..=30 => editor.set_block(MOSS_CARPET, x, 1, z, None, None),
                    31..=40 => place_grass_cover(editor, x, z, rng, 2),
                    41..=44 => {
                        let f = FIELD_FLOWERS[rng.random_range(0..FIELD_FLOWERS.len())];
                        editor.set_block(f, x, 1, z, None, None);
                    }
                    _ => {}
                }
            } else if on(editor, &[COARSE_DIRT, ROOTED_DIRT]) {
                match rng.random_range(0..100) {
                    0..=5 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    6..=12 => editor.set_block(FERN, x, 1, z, None, None),
                    _ => {}
                }
            }
        }
    }
}

/// Grow a farm plot's single crop. Tilled plots keep the enclosure-gated irrigation
/// dots, sunflower plots plant their dirt rows, pumpkin patches fruit sparsely, fallow
/// fields carry stubble, and wheat and fallow plots get the odd hay bale.
fn decorate_farm_plot(
    editor: &mut WorldEditor,
    cell: &FieldCell,
    x: i32,
    z: i32,
    rng: &mut impl Rng,
) {
    let Some(crop) = cell.crop else { return };
    let on = |editor: &WorldEditor, blocks: &[Block]| editor.check_for_block(x, 0, z, Some(blocks));
    match crop {
        FarmCrop::Wheat | FarmCrop::Potato | FarmCrop::Carrot | FarmCrop::Beetroot => {
            if x % 9 == 0 && z % 9 == 0 && editor.water_source_is_enclosed(x, z) {
                editor.set_block(WATER, x, 0, z, Some(&[FARMLAND]), None);
            } else if on(editor, &[FARMLAND]) {
                let (mut block, mut max_age) = match crop {
                    FarmCrop::Wheat => (WHEAT, 7),
                    FarmCrop::Potato => (POTATOES, 7),
                    FarmCrop::Carrot => (CARROTS, 7),
                    _ => (BEETROOTS, 3),
                };
                // Stray-seed pockets where another crop took root, noise-clustered so
                // they read as patches rather than single scattered blocks.
                if value_noise_01(x + 321, z - 777, 4) < 0.022 {
                    let alt = [WHEAT, POTATOES, CARROTS, BEETROOTS]
                        [((cell.species_seed >> 9) % 4) as usize];
                    max_age = if alt == BEETROOTS { 3 } else { 7 };
                    block = alt;
                }
                // Most of the field sits at its own stage, with younger spots mixed in.
                let mut growth = cell.crop_age;
                if rng.random_range(0..5) == 0 {
                    growth = growth.saturating_sub(1 + rng.random_range(0..2));
                }
                place_crop(editor, block, growth, max_age, x, z);
            } else if on(editor, &[COARSE_DIRT, ROOTED_DIRT]) {
                match rng.random_range(0..100) {
                    0..=2 if crop == FarmCrop::Wheat => {
                        editor.set_block(HAY_BALE, x, 1, z, None, Some(&[SPONGE]));
                    }
                    3..=8 => place_tall(editor, SUNFLOWER_LOWER, SUNFLOWER_UPPER, x, z),
                    9..=13 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    14..=20 => editor.set_block(GRASS, x, 1, z, None, None),
                    _ => {}
                }
            }
        }
        FarmCrop::Sunflower => {
            if on(editor, &[COARSE_DIRT]) {
                if rng.random_range(0..100) < 85 {
                    place_tall(editor, SUNFLOWER_LOWER, SUNFLOWER_UPPER, x, z);
                }
            } else if on(editor, &[GRASS_BLOCK]) && rng.random_range(0..100) < 25 {
                editor.set_block(GRASS, x, 1, z, None, None);
            }
        }
        FarmCrop::Pumpkin => {
            if on(editor, &[GRASS_BLOCK]) {
                match rng.random_range(0..100) {
                    0..=5 => editor.set_block(PUMPKIN, x, 1, z, None, None),
                    6..=25 => editor.set_block(GRASS, x, 1, z, None, None),
                    26 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    _ => {}
                }
            } else if on(editor, &[COARSE_DIRT]) && rng.random_range(0..100) < 8 {
                editor.set_block(GRASS, x, 1, z, None, None);
            }
        }
        FarmCrop::Fallow => {
            if on(editor, &[COARSE_DIRT, ROOTED_DIRT]) {
                match rng.random_range(0..200) {
                    0..=1 => editor.set_block(HAY_BALE, x, 1, z, None, Some(&[SPONGE])),
                    2..=11 => editor.set_block(DEAD_BUSH, x, 1, z, None, None),
                    12..=29 => editor.set_block(GRASS, x, 1, z, None, None),
                    30..=33 => editor.set_block(FERN, x, 1, z, None, None),
                    _ => {}
                }
            } else if on(editor, &[GRASS_BLOCK]) && rng.random_range(0..100) < 20 {
                place_grass_cover(editor, x, z, rng, 2);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn profile(mix: &str) -> FieldProfile {
        let args = FieldArgs {
            field_mix: Some(FieldMix::parse(mix).unwrap()),
            ..FieldArgs::default()
        };
        FieldProfile::from_args(&args, 1.0).unwrap()
    }

    fn category_at(p: &FieldProfile, x: i32, z: i32) -> FieldCategory {
        let r = p.parcel_at(x, z);
        p.category_for_parcel(r.px, r.pz, r.dsalt)
    }

    /// Flags absent or classic: no profile, so the field pass is skipped and the
    /// surface is byte-identical to the uniform crop sheet.
    #[test]
    fn absent_or_classic_is_inactive() {
        let off = FieldArgs::default();
        assert!(FieldProfile::from_args(&off, 1.0).is_none());
        let classic = FieldArgs {
            field_mix: Some(FieldMix::parse("Classic").unwrap()),
            ..FieldArgs::default()
        };
        assert!(FieldProfile::from_args(&classic, 1.0).is_none());
        for preset in ["smallholding", "patchwork", "prairie", "pasture", "farm=1"] {
            assert!(FieldProfile::from_args(
                &FieldArgs {
                    field_mix: Some(FieldMix::parse(preset).unwrap()),
                    ..FieldArgs::default()
                },
                1.0
            )
            .is_some());
        }
    }

    /// --farm-crops alone lays out crop plots only, growing just the listed crops.
    #[test]
    fn farm_crops_alone_is_all_crop_plots() {
        let args = FieldArgs {
            farm_crops: Some(FarmCrops::parse("pumpkin=1, fallow=1").unwrap()),
            ..FieldArgs::default()
        };
        let p = FieldProfile::from_args(&args, 1.0).unwrap();
        for x in (0..3000).step_by(23) {
            for z in (0..3000).step_by(29) {
                let c = p.cell_at(x, z);
                assert_eq!(c.cat, FieldCategory::Farm);
                assert!(matches!(c.crop, Some(FarmCrop::Pumpkin | FarmCrop::Fallow)));
            }
        }
    }

    #[test]
    fn lists_reject_typos_and_all_zero() {
        assert!(FieldMix::parse("farm=50,grass=10").is_err());
        assert!(FieldMix::parse("farm=0").is_err());
        assert!(FieldMix::parse("farm").is_err());
        assert!(FarmCrops::parse("wheat=x").is_err());
        assert!(FarmCrops::parse("corn=5").is_err());
        let m = FieldMix::parse("FARM=3, moss=1").unwrap();
        assert_eq!(m.shares, [0, 0, 0, 3, 1]);
        assert_eq!(m.sizes, FieldMix::PATCHWORK.sizes);
    }

    #[test]
    fn farm_parcels_are_monoculture_and_diverse() {
        let p = profile("patchwork");
        // Crop changes along a straight walk must be far rarer than cells.
        let mut changes = 0;
        let mut prev = p.cell_at(0, 500).crop;
        for x in 1..2000 {
            let c = p.cell_at(x, 500).crop;
            if c != prev {
                changes += 1;
                prev = c;
            }
        }
        assert!(
            changes < 2000 / 5,
            "crop changes {changes} too frequent for parcels"
        );
        let mut seen = HashSet::new();
        for x in (0..8000).step_by(11) {
            for z in (0..8000).step_by(11) {
                if let Some(c) = p.cell_at(x, z).crop {
                    seen.insert(format!("{c:?}"));
                }
            }
        }
        assert_eq!(seen.len(), 7, "all crops should appear, saw {seen:?}");
    }

    /// Prairie is wheat-led, smallholding spreads its crops.
    #[test]
    fn crop_shares_follow_preset_weights() {
        fn wheat_share(mix: &str) -> f64 {
            let p = profile(mix);
            let (mut wheat, mut total) = (0u32, 0u32);
            for x in (0..9000).step_by(17) {
                for z in (0..9000).step_by(17) {
                    let r = p.parcel_at(x, z);
                    if p.category_for_parcel(r.px, r.pz, r.dsalt) == FieldCategory::Farm {
                        total += 1;
                        wheat += (p.mix.crops.pick(r.px, r.pz ^ r.dsalt) == FarmCrop::Wheat) as u32;
                    }
                }
            }
            wheat as f64 / total as f64
        }
        let prairie = wheat_share("prairie");
        let small = wheat_share("smallholding");
        assert!(
            prairie > 0.5,
            "prairie wheat share {prairie} should dominate"
        );
        assert!(small < 0.35, "smallholding wheat share {small} too high");
    }

    /// Category shares track the mix: pasture is grassy, prairie ~85% farm.
    #[test]
    fn style_shares_roughly_match_the_mix() {
        let share = |p: &FieldProfile, cats: &[FieldCategory]| {
            let (mut hit, mut n) = (0, 0);
            for x in (0..6000).step_by(13) {
                for z in (0..6000).step_by(13) {
                    hit += cats.contains(&category_at(p, x, z)) as u32;
                    n += 1;
                }
            }
            hit as f64 / n as f64
        };
        let farm = share(&profile("prairie"), &[FieldCategory::Farm]);
        assert!(farm > 0.75 && farm < 0.95, "farm share {farm} not ~0.85");
        let grassy = share(
            &profile("pasture"),
            &[FieldCategory::Plains, FieldCategory::Flower],
        );
        assert!(grassy > 0.7, "pasture grassy share {grassy}");
    }

    /// Tracks appear only where two styles or two field systems meet.
    #[test]
    fn tracks_only_on_parcel_or_domain_edges() {
        let p = profile("prairie");
        let mut tracks = 0;
        for x in 0..400 {
            for z in 0..400 {
                if p.cell_at(x, z).is_track {
                    let r = p.parcel_at(x, z);
                    let boundary = r.lx == 0 || r.lz == 0 || r.lx == r.w - 1 || r.lz == r.l - 1;
                    assert!(r.on_domain_edge || boundary, "stray track at ({x},{z})");
                    tracks += 1;
                }
            }
        }
        assert!(tracks > 0 && tracks < 400 * 400 / 5, "{tracks} tracks");
    }

    /// A parcel (grid index within its orientation domain) grows one crop at one age.
    #[test]
    fn crop_is_uniform_across_a_whole_parcel() {
        let p = profile("patchwork");
        let mut seen: HashMap<(i32, i32, i32), (Option<FarmCrop>, u8)> = HashMap::new();
        for x in -300..300 {
            for z in -300..300 {
                let r = p.parcel_at(x, z);
                let c = p.cell_at(x, z);
                let prev = *seen
                    .entry((r.px, r.pz, r.dsalt))
                    .or_insert((c.crop, c.crop_age));
                assert_eq!(
                    prev,
                    (c.crop, c.crop_age),
                    "parcel ({},{}) split",
                    r.px,
                    r.pz
                );
            }
        }
    }

    /// Switching preset changes plot size and mix, not which way the field system runs.
    #[test]
    fn presets_share_the_orientation_field() {
        let (a, b) = (profile("patchwork"), profile("pasture"));
        for x in (-500..500).step_by(37) {
            for z in (-500..500).step_by(41) {
                assert_eq!(a.parcel_at(x, z).dsalt, b.parcel_at(x, z).dsalt);
            }
        }
    }

    /// Parcel sizes are metres: doubling --scale or --field-scale doubles them in blocks.
    #[test]
    fn parcels_follow_scale_and_field_scale() {
        let mix = Some(FieldMix::PATCHWORK);
        let at = |field_scale, map_scale| {
            let args = FieldArgs {
                field_mix: mix,
                field_scale,
                ..FieldArgs::default()
            };
            FieldProfile::from_args(&args, map_scale)
                .unwrap()
                .parcel_at(1234, 5678)
                .w
        };
        let w = at(100, 1.0);
        for doubled in [at(200, 1.0), at(100, 2.0)] {
            assert!((doubled - 2 * w).abs() <= 2, "{doubled} is not ~2x {w}");
        }
    }

    /// Crop placement shares one compound per age, and an out-of-range age clamps.
    #[test]
    fn crop_age_props_are_interned() {
        assert!(Arc::ptr_eq(&crop_age_props(3), &crop_age_props(3)));
        assert!(Arc::ptr_eq(
            &crop_age_props(200),
            &crop_age_props(MAX_CROP_AGE)
        ));
        for age in 0..=MAX_CROP_AGE {
            let fastnbt::Value::Compound(map) = crop_age_props(age).as_ref().clone() else {
                panic!("crop properties must be a compound");
            };
            assert_eq!(
                map.get("age"),
                Some(&fastnbt::Value::String(age.to_string()))
            );
        }
    }
}
