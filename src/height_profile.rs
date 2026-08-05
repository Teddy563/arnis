//! The single source of truth for a world's vertical geometry.
//!
//! One [`HeightProfile`] defines where a world's floor and ceiling sit, what real-world
//! elevation maps to which Y, and which Minecraft version the result targets. Exactly
//! three things are allowed to consume it: the datapack writer, the chunk writer, and the
//! UI. Anything that computes its own `min_y`, `height` or datum offset is a bug — the
//! failure mode is tiles that don't line up, and it only surfaces hundreds of chunks in.
//!
//! ## Engine invariants
//!
//! These come from `BlockPos` packing Y into 12 signed bits and from the Anvil chunk
//! format, not from configuration. [`HeightProfile::validate`] enforces every one of them
//! and no constructor may return a profile that fails it:
//!
//! | Constraint | Value |
//! |---|---|
//! | `min_y` | -2032 ..= 2031, multiple of 16 |
//! | `height` | 16 ..= 4064, multiple of 16 |
//! | `min_y + height` | <= 2032 (highest buildable Y is 2031) |
//! | section index | -128 ..= 127 (signed byte) |
//!
//! Nothing raises these — no datapack, plugin or server flag. Only a Cubic Chunks-class
//! rewrite of the chunk format, which breaks vanilla clients. When a region needs more
//! than fits, the honest answer is vertical compression, reported loudly.

use std::fmt;

/// Hard engine limits. Not tunable.
pub const ABS_MIN_Y: i32 = -2032;
/// Highest buildable Y.
pub const ABS_MAX_Y: i32 = 2031;
/// `min_y + height` may not exceed this.
pub const ABS_TOP: i32 = 2032;
pub const MIN_HEIGHT: i32 = 16;
pub const MAX_HEIGHT: i32 = 4064;
/// Anvil stores a section's Y index in a signed byte.
pub const MIN_SECTION_INDEX: i32 = -128;
pub const MAX_SECTION_INDEX: i32 = 127;

/// Vanilla 1.18+ geometry: the default when nothing asks for more.
pub const VANILLA_MIN_Y: i32 = -64;
pub const VANILLA_HEIGHT: i32 = 384;

/// Why a profile was rejected. Every variant is a refusal, never a clamp: a silent clamp
/// here becomes a user regenerating hundreds of gigabytes before noticing the mountains
/// are flat-topped.
#[derive(Debug, Clone, PartialEq)]
pub enum HeightError {
    MinYOutOfRange(i32),
    MinYNotAligned(i32),
    HeightOutOfRange(i32),
    HeightNotAligned(i32),
    TopExceeded {
        min_y: i32,
        height: i32,
    },
    SectionIndexOutOfRange(i32),
    VScaleNotFinite(f64),
    /// The target Minecraft version predates extended height (1.17).
    VersionTooOld {
        version: String,
        needed: i32,
        available: i32,
    },
}

impl fmt::Display for HeightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MinYOutOfRange(v) => write!(
                f,
                "min_y {v} is outside the engine range {ABS_MIN_Y}..={ABS_MAX_Y}. \
                 Y is packed into 12 signed bits in BlockPos; nothing raises this."
            ),
            Self::MinYNotAligned(v) => {
                write!(
                    f,
                    "min_y {v} is not a multiple of 16 (chunk sections are 16 blocks tall)"
                )
            }
            Self::HeightOutOfRange(v) => {
                write!(f, "height {v} is outside {MIN_HEIGHT}..={MAX_HEIGHT}")
            }
            Self::HeightNotAligned(v) => {
                write!(f, "height {v} is not a multiple of 16")
            }
            Self::TopExceeded { min_y, height } => write!(
                f,
                "min_y {min_y} + height {height} = {} exceeds {ABS_TOP}; \
                 the highest buildable Y is {ABS_MAX_Y}",
                min_y + height
            ),
            Self::SectionIndexOutOfRange(v) => write!(
                f,
                "section index {v} is outside {MIN_SECTION_INDEX}..={MAX_SECTION_INDEX} \
                 (Anvil stores it in a signed byte)"
            ),
            Self::VScaleNotFinite(v) => write!(f, "v_scale {v} must be finite and > 0"),
            Self::VersionTooOld {
                version,
                needed,
                available,
            } => write!(
                f,
                "Minecraft {version} has no extended build height — that arrived in 1.17. \
                 This region needs {needed} blocks of vertical range and {available} are \
                 available. Either target 1.18+, or accept vertical compression into the \
                 0..255 range."
            ),
        }
    }
}

impl std::error::Error for HeightError {}

/// A world's vertical geometry. Immutable once a world exists: it is written to
/// `meld.height.json` at creation and read back on every later regeneration, because a
/// world's chunks were serialised against these exact numbers.
#[derive(Debug, Clone, PartialEq)]
pub struct HeightProfile {
    /// Lowest buildable Y. Multiple of 16.
    pub min_y: i32,
    /// Total vertical range in blocks. Multiple of 16.
    pub height: i32,
    /// The Y that real-world elevation 0 m maps to.
    pub datum_y: i32,
    /// Blocks per metre of real elevation. < 1.0 means the terrain was compressed.
    pub v_scale: f64,
    /// Target Minecraft version this geometry was computed for.
    pub mc_version: String,
}

impl HeightProfile {
    /// Vanilla 1.18+ geometry (-64..319). The default for every world that does not ask
    /// for more, and byte-compatible with what the generator did before profiles existed.
    pub fn vanilla(mc_version: impl Into<String>, datum_y: i32, v_scale: f64) -> Self {
        HeightProfile {
            min_y: VANILLA_MIN_Y,
            height: VANILLA_HEIGHT,
            datum_y,
            v_scale,
            mc_version: mc_version.into(),
        }
    }

    /// Highest buildable Y in this world.
    #[inline]
    pub fn max_y(&self) -> i32 {
        self.min_y + self.height - 1
    }

    /// Lowest section index (`yPos` of the bottom section).
    #[inline]
    pub fn min_section_y(&self) -> i32 {
        self.min_y.div_euclid(16)
    }

    /// Highest section index.
    #[inline]
    pub fn max_section_y(&self) -> i32 {
        self.max_y().div_euclid(16)
    }

    /// Bits needed per heightmap entry: `ceil(log2(height + 1))`, never below vanilla's 9.
    /// Deriving this is not optional — a 4064-tall world needs 12, and hardcoding 9
    /// corrupts the heightmap of every extended world.
    #[inline]
    pub fn heightmap_bits(&self) -> u32 {
        let needed = 32 - ((self.height as u32).saturating_add(1)).leading_zeros();
        needed.max(9)
    }

    /// True when this profile is exactly vanilla geometry (so no datapack is required).
    #[inline]
    pub fn is_vanilla(&self) -> bool {
        self.min_y == VANILLA_MIN_Y && self.height == VANILLA_HEIGHT
    }

    /// Map a real-world elevation in metres to a Y in this world.
    #[inline]
    pub fn elevation_to_y(&self, metres: f64) -> i32 {
        (metres * self.v_scale).round() as i32 + self.datum_y
    }

    /// Inverse of [`Self::elevation_to_y`], for reporting.
    #[inline]
    pub fn y_to_elevation(&self, y: i32) -> f64 {
        if self.v_scale <= 0.0 {
            return 0.0;
        }
        (y - self.datum_y) as f64 / self.v_scale
    }

    /// Enforce every engine invariant. Called by every constructor; call it again after
    /// deserialising a sidecar, because a hand-edited file is untrusted input.
    pub fn validate(&self) -> Result<(), HeightError> {
        if !(ABS_MIN_Y..=ABS_MAX_Y).contains(&self.min_y) {
            return Err(HeightError::MinYOutOfRange(self.min_y));
        }
        if self.min_y % 16 != 0 {
            return Err(HeightError::MinYNotAligned(self.min_y));
        }
        if !(MIN_HEIGHT..=MAX_HEIGHT).contains(&self.height) {
            return Err(HeightError::HeightOutOfRange(self.height));
        }
        if self.height % 16 != 0 {
            return Err(HeightError::HeightNotAligned(self.height));
        }
        if self.min_y + self.height > ABS_TOP {
            return Err(HeightError::TopExceeded {
                min_y: self.min_y,
                height: self.height,
            });
        }
        for idx in [self.min_section_y(), self.max_section_y()] {
            if !(MIN_SECTION_INDEX..=MAX_SECTION_INDEX).contains(&idx) {
                return Err(HeightError::SectionIndexOutOfRange(idx));
            }
        }
        if !self.v_scale.is_finite() || self.v_scale <= 0.0 {
            return Err(HeightError::VScaleNotFinite(self.v_scale));
        }
        Ok(())
    }
}

/// What a world is being asked to hold, in real-world metres, plus the room the user
/// wants kept free at each end. Headroom and underroom are user-facing knobs on purpose —
/// burying them is how worlds end up with trees clipping the build limit.
#[derive(Debug, Clone, Copy)]
pub struct FitRequest {
    /// Lowest real elevation in the region (metres).
    pub floor_m: f64,
    /// Highest real elevation in the region (metres).
    pub peak_m: f64,
    /// Blocks per metre the user asked for (1.0 = 1 m per block).
    pub v_scale: f64,
    /// Blocks kept free above the highest terrain, for trees and buildings.
    pub headroom: i32,
    /// Blocks kept free below the lowest terrain, for caves and water carving.
    pub underroom: i32,
    /// Y that elevation `floor_m` should sit at once fitted.
    pub floor_y: i32,
}

/// The result of fitting, including whether the terrain had to be squashed to fit.
#[derive(Debug, Clone)]
pub struct Fitted {
    pub profile: HeightProfile,
    /// The `DataVersion` chunks must be stamped with for this target. Resolved from the
    /// verified version table, never from a call site.
    pub data_version: i32,
    /// The version row this profile was resolved against. The datapack writer reads its
    /// schema from here, so the pack shape and the geometry can never come from
    /// different versions.
    pub caps: &'static crate::mc_version::VersionCaps,
    /// 1.0 = the requested v_scale was honoured. 2.5 = the terrain was squashed 2.5:1.
    /// Anything above 1.0 MUST be surfaced to the user — silent rescaling is the single
    /// worst failure mode in this subsystem, because the world looks fine and isn't.
    pub compression: f64,
}

impl Fitted {
    #[inline]
    pub fn is_compressed(&self) -> bool {
        self.compression > 1.000_001
    }

    /// A one-line report for logs and the UI. Say it once per world, not once per cell.
    pub fn report(&self) -> String {
        let p = &self.profile;
        let ceiling_m = p.y_to_elevation(p.max_y());
        let sea_level_y = p.elevation_to_y(0.0);
        let detail = format!(
            " Sea level Y={sea_level_y}; holds real elevation to ~{ceiling_m:.0} m; {}-bit heightmaps; DataVersion {}.",
            p.heightmap_bits(),
            self.data_version
        );
        if self.is_compressed() {
            format!(
                "Height: Y {}..{} ({} blocks) for {} — terrain COMPRESSED {:.2}:1 \
                 ({:.2} blocks per metre instead of the requested scale); vertical \
                 distances in this world are not to scale.",
                p.min_y,
                p.max_y(),
                p.height,
                p.mc_version,
                self.compression,
                p.v_scale
            ) + &detail
        } else {
            format!(
                "Height: Y {}..{} ({} blocks) for {} — 1:1 vertical scale at {:.2} \
                 blocks per metre.",
                p.min_y,
                p.max_y(),
                p.height,
                p.mc_version,
                p.v_scale
            ) + &detail
        }
    }
}

/// Fit the smallest legal world that holds this terrain.
///
/// Rules, in order (see the brief's §4):
/// 1. smallest legal height, never the maximum by default — every extra 16 blocks is
///    another section per column in lighting, heightmaps and region files;
/// 2. `headroom` above the peak and `underroom` below the floor are reserved explicitly;
/// 3. if it will not fit even at maximum, `v_scale` is compressed and the ratio is
///    reported, never applied silently;
/// 4. rounding to multiples of 16 happens once, at the end, outward — never mid-calculation.
///
/// `caps` gates the whole thing: a version without extended height may only produce
/// vanilla geometry, and is refused (not clamped) when the terrain needs more.
pub fn fit(
    req: &FitRequest,
    caps: &'static crate::mc_version::VersionCaps,
) -> Result<Fitted, HeightError> {
    if !req.v_scale.is_finite() || req.v_scale <= 0.0 {
        return Err(HeightError::VScaleNotFinite(req.v_scale));
    }
    let span_m = (req.peak_m - req.floor_m).max(0.0);
    let headroom = req.headroom.max(0);
    let underroom = req.underroom.max(0);

    // Blocks of terrain at the requested scale, plus the reserved room at each end.
    let wanted = (span_m * req.v_scale).ceil() as i64 + headroom as i64 + underroom as i64 + 1;

    // The ceiling this version allows.
    let cap_height = if caps.extended_height {
        MAX_HEIGHT as i64
    } else {
        VANILLA_HEIGHT as i64
    };

    let (v_scale, compression) = if wanted <= cap_height {
        (req.v_scale, 1.0)
    } else {
        // Squash only the terrain; the reserved room is not negotiable.
        let room_for_terrain = (cap_height - headroom as i64 - underroom as i64 - 1).max(1) as f64;
        if span_m <= 0.0 {
            (req.v_scale, 1.0)
        } else {
            let squashed = room_for_terrain / span_m;
            if !caps.extended_height {
                // Refuse rather than quietly shear a mountain range off at the vanilla cap.
                return Err(HeightError::VersionTooOld {
                    version: caps.id.clone(),
                    needed: wanted.min(i32::MAX as i64) as i32,
                    available: cap_height as i32,
                });
            }
            (squashed, req.v_scale / squashed)
        }
    };

    // Terrain extent in blocks at the scale we settled on.
    let terrain_blocks = (span_m * v_scale).ceil() as i32;

    // Place the band inside the engine's box. The caller's `floor_y` is a PREFERENCE: if
    // the terrain plus its headroom would push past the engine ceiling, the whole band
    // slides down rather than having its peaks sheared off. Sliding is lossless; shearing
    // is not.
    let highest_usable_floor = ABS_MAX_Y - headroom - terrain_blocks;
    let floor_y = req.floor_y.min(highest_usable_floor);

    // Round outward, once, at the end.
    //
    // The band must also COVER the vanilla range. The writer still emits the vanilla
    // column of sections (-64..319) for every chunk, so a dimension declared narrower
    // than that would ship chunks with sections outside its own world — the client sees
    // blocks above a ceiling that does not exist. Widening here is free: when the terrain
    // needs nothing beyond vanilla the result IS vanilla geometry, and `is_vanilla()`
    // then correctly reports that no datapack is needed at all.
    let mut min_y = floor16(floor_y - underroom).clamp(ABS_MIN_Y, VANILLA_MIN_Y);
    let top_y = (floor_y + terrain_blocks + headroom).max(VANILLA_MIN_Y + VANILLA_HEIGHT - 1);
    let mut height = ceil16(top_y - min_y + 1).clamp(MIN_HEIGHT, MAX_HEIGHT);
    // Both ends are now aligned; make sure the pair still fits the box, trimming the
    // floor first (the ceiling is where the terrain actually is).
    if min_y + height > ABS_TOP {
        min_y = floor16(ABS_TOP - height).max(ABS_MIN_Y);
        height = height.min(ABS_TOP - min_y);
    }

    let profile = HeightProfile {
        min_y,
        height,
        // datum: elevation `floor_m` maps to `floor_y`, so 0 m maps here. It may legally
        // sit outside the world — a world that starts at 800 m has its 0 m origin below
        // its own floor, which is just the affine's origin, not a block position.
        datum_y: floor_y - (req.floor_m * v_scale).round() as i32,
        v_scale,
        mc_version: caps.id.clone(),
    };
    profile.validate()?;
    Ok(Fitted {
        profile,
        data_version: caps
            .data_version
            .unwrap_or_else(crate::mc_version::default_data_version),
        caps,
        compression,
    })
}

/// Resolve the one profile for this run: the single place a world's vertical geometry is
/// decided.
///
/// Without `--disable-height-limit` this returns vanilla geometry, so the generator
/// behaves exactly as it did before profiles existed. With it, the world is fitted to the
/// terrain the elevation pass actually produced — under Meld's elevation lock that range
/// is global, so every cell of a tiled world fits identical geometry.
pub fn for_run(args: &crate::args::Args, ground: &crate::ground::Ground) -> Result<Fitted, String> {
    // A named version must be one we have verified constants for; an unknown one is
    // refused rather than approximated.
    let caps = match args.mc_version.as_deref() {
        Some(v) => crate::mc_version::capabilities(v)
            .ok_or_else(|| crate::mc_version::unknown_version_message(v))?,
        None => crate::mc_version::default_caps(),
    };

    // Answer the request the user actually made first: asking for extended height on a
    // release that has none deserves the version-floor message, not a chunk-layout one.
    if args.disable_height_limit && !caps.extended_height {
        return Err(format!(
            "--disable-height-limit was requested but Minecraft {} predates extended \
             build height (1.17). Target 1.18+ for a taller world, or drop the flag and \
             accept the vanilla range.",
            caps.id
        ));
    }

    // Pre-1.18 chunks are a `Level` compound with int-array biomes — a different writer,
    // not a variation on this one. Refuse rather than emit 1.18+ chunks under a pre-1.18
    // DataVersion, which produces a world that loads and then misbehaves.
    if caps.chunk_layout != crate::mc_version::ChunkLayout::Flat {
        return Err(format!(
            "Minecraft {} uses the pre-1.18 chunk layout (a Level compound with \
             int-array biomes), which this writer does not implement. Target 1.18 or later.",
            caps.id
        ));
    }

    let datum = ground.ground_level();
    let (floor_m, peak_m, blocks_per_m) = ground.elevation_range_m().unwrap_or((0.0, 0.0, 1.0));
    let v_scale = if blocks_per_m.is_finite() && blocks_per_m > 0.0 {
        blocks_per_m
    } else {
        1.0
    };

    if !args.disable_height_limit {
        // Vanilla geometry: unchanged behaviour, and no datapack.
        return Ok(Fitted {
            profile: HeightProfile::vanilla(caps.id.clone(), datum, v_scale),
            data_version: crate::mc_version::data_version_for(args.mc_version.as_deref())?,
            caps,
            compression: 1.0,
        });
    }

    let fitted = fit(
        &FitRequest {
            floor_m,
            peak_m,
            v_scale,
            headroom: args.height_headroom,
            underroom: args.height_underroom,
            floor_y: datum,
        },
        caps,
    )
    .map_err(|e| e.to_string())?;

    apply_explicit_bounds(fitted, args.min_y, args.max_y)
}

/// Apply an explicit `--min-y` / `--max-y` on top of the fitted geometry.
///
/// Explicit values REPLACE the fitted ones, but they are checked, never clamped: a floor
/// or ceiling that would cut into the terrain is refused with what it would have cost.
/// Silently honouring it would shear the peaks off a mountain range and say nothing.
pub fn apply_explicit_bounds(
    fitted: Fitted,
    min_y: Option<i32>,
    max_y: Option<i32>,
) -> Result<Fitted, String> {
    if min_y.is_none() && max_y.is_none() {
        return Ok(fitted);
    }
    let fit_lo = fitted.profile.min_y;
    let fit_hi = fitted.profile.max_y();
    let lo = min_y.unwrap_or(fit_lo);
    let hi = max_y.unwrap_or(fit_hi);

    if lo % 16 != 0 {
        return Err(format!(
            "--min-y {lo} is not a multiple of 16. Chunk sections are 16 blocks tall, so \
             a world floor has to land on one; try {}.",
            floor16(lo)
        ));
    }
    if hi < lo {
        return Err(format!("--max-y {hi} is below --min-y {lo}."));
    }
    // The terrain is where it is; a ceiling under it or a floor over it loses blocks.
    if hi < fit_hi {
        return Err(format!(
            "--max-y {hi} is below the {fit_hi} this terrain needs (its peak plus the \
             headroom). Lower --height-headroom, reduce the vertical scale, or raise \
             --max-y; refusing rather than shearing the peaks off."
        ));
    }
    if lo > fit_lo {
        return Err(format!(
            "--min-y {lo} is above the {fit_lo} this terrain needs (its floor minus the \
             underroom). Lower --height-underroom or lower --min-y; refusing rather than \
             cutting the ground away."
        ));
    }

    let height = ceil16(hi - lo + 1);
    let profile = HeightProfile {
        min_y: lo,
        height,
        ..fitted.profile
    };
    profile
        .validate()
        .map_err(|e| format!("--min-y {lo} / --max-y {hi} do not describe a legal world: {e}"))?;
    Ok(Fitted { profile, ..fitted })
}

/// Round down to a multiple of 16 (towards negative infinity, so it works below 0).
#[inline]
pub fn floor16(v: i32) -> i32 {
    v.div_euclid(16) * 16
}

/// Round up to a multiple of 16.
#[inline]
pub fn ceil16(v: i32) -> i32 {
    floor16(v + 15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vanilla_profile_is_valid_and_matches_the_old_constants() {
        let p = HeightProfile::vanilla("1.21.9", -56, 1.0);
        p.validate().unwrap();
        assert_eq!(p.min_y, -64);
        assert_eq!(p.max_y(), 319);
        assert_eq!(p.min_section_y(), -4);
        assert_eq!(p.max_section_y(), 19);
        assert!(p.is_vanilla());
    }

    #[test]
    fn heightmap_bits_are_derived_not_fixed() {
        // Vanilla 384 fits in 9 bits, and 9 is also the floor.
        assert_eq!(HeightProfile::vanilla("1.21.9", 0, 1.0).heightmap_bits(), 9);
        for (height, bits) in [
            (16, 9),
            (384, 9),
            (512, 10),
            (1024, 11),
            (2048, 12),
            (4064, 12),
        ] {
            let p = HeightProfile {
                min_y: -64,
                height,
                datum_y: 0,
                v_scale: 1.0,
                mc_version: "1.21.9".into(),
            };
            assert_eq!(p.heightmap_bits(), bits, "height {height}");
        }
    }

    #[test]
    fn the_full_legal_box_validates() {
        let p = HeightProfile {
            min_y: ABS_MIN_Y,
            height: MAX_HEIGHT,
            datum_y: 0,
            v_scale: 1.0,
            mc_version: "1.21.9".into(),
        };
        p.validate().unwrap();
        assert_eq!(p.max_y(), ABS_MAX_Y);
        assert_eq!(p.min_section_y(), -127);
        assert_eq!(p.max_section_y(), 126);
    }

    #[test]
    fn rejects_every_invariant_violation() {
        let base = HeightProfile::vanilla("1.21.9", 0, 1.0);

        let mut p = base.clone();
        p.min_y = -2048; // below the 12-bit floor
        assert!(matches!(p.validate(), Err(HeightError::MinYOutOfRange(_))));

        let mut p = base.clone();
        p.min_y = -60; // not a multiple of 16
        assert!(matches!(p.validate(), Err(HeightError::MinYNotAligned(_))));

        let mut p = base.clone();
        p.height = 4080; // over the cap
        assert!(matches!(
            p.validate(),
            Err(HeightError::HeightOutOfRange(_))
        ));

        let mut p = base.clone();
        p.height = 100; // not a multiple of 16
        assert!(matches!(
            p.validate(),
            Err(HeightError::HeightNotAligned(_))
        ));

        let mut p = base.clone();
        p.min_y = 2016;
        p.height = 64; // top would be 2080
        assert!(matches!(p.validate(), Err(HeightError::TopExceeded { .. })));

        // NOTE: datum_y is deliberately NOT range-checked. It is the origin of the
        // elevation affine, not a block position, and a world whose terrain starts high
        // above sea level legitimately has its 0 m origin below its own floor.

        let mut p = base.clone();
        p.v_scale = 0.0;
        assert!(matches!(p.validate(), Err(HeightError::VScaleNotFinite(_))));
        p.v_scale = f64::NAN;
        assert!(matches!(p.validate(), Err(HeightError::VScaleNotFinite(_))));
    }

    #[test]
    fn elevation_mapping_round_trips() {
        let p = HeightProfile {
            min_y: -64,
            height: 384,
            datum_y: -56,
            v_scale: 0.5,
            mc_version: "1.21.9".into(),
        };
        assert_eq!(p.elevation_to_y(0.0), -56);
        assert_eq!(p.elevation_to_y(100.0), -6);
        assert_eq!(p.elevation_to_y(-20.0), -66); // below the world: the caller must clamp
        assert!((p.y_to_elevation(-6) - 100.0).abs() < 1e-9);
    }

    fn caps(id: &str) -> &'static crate::mc_version::VersionCaps {
        crate::mc_version::capabilities(id).expect("test version must exist in the table")
    }

    fn req(floor_m: f64, peak_m: f64, v_scale: f64) -> FitRequest {
        FitRequest {
            floor_m,
            peak_m,
            v_scale,
            headroom: 32,
            underroom: 16,
            floor_y: -48,
        }
    }

    /// The declared world must always contain the vanilla column the writer emits — a
    /// dimension narrower than its own chunks puts blocks outside the world.
    #[test]
    fn the_fitted_world_always_covers_the_vanilla_band() {
        for (floor_m, peak_m, scale, floor_y) in [
            (0.0, 20.0, 1.0, -48),
            (0.0, 2544.0, 0.05, -56), // Romania at 1:20 — fits inside vanilla
            (0.0, 2544.0, 1.0, -56),  // same range at 1:1 — genuinely needs a tall world
            (-30.0, 5.0, 1.0, 60),
        ] {
            let mut r = req(floor_m, peak_m, scale);
            r.floor_y = floor_y;
            let f = fit(&r, caps("26.1.2")).unwrap();
            assert!(
                f.profile.min_y <= VANILLA_MIN_Y,
                "floor {} must reach the vanilla floor",
                f.profile.min_y
            );
            assert!(
                f.profile.max_y() >= VANILLA_MIN_Y + VANILLA_HEIGHT - 1,
                "ceiling {} must reach the vanilla ceiling",
                f.profile.max_y()
            );
            f.profile.validate().unwrap();
        }
    }

    /// Terrain whose reserved room also fits inside vanilla must produce VANILLA
    /// geometry, so no datapack is written and the world carries no
    /// experimental-features prompt for nothing.
    #[test]
    fn terrain_that_fits_vanilla_needs_no_datapack() {
        // Romania's full 0..2544 m at 1:20 is ~127 blocks of relief. With the terrain
        // floor at Y -56, 8 blocks of underroom reach exactly the vanilla floor.
        let mut r = req(0.0, 2544.0, 0.05);
        r.floor_y = -56;
        r.underroom = 8;
        let f = fit(&r, caps("26.1.2")).unwrap();
        assert!(
            f.profile.is_vanilla(),
            "expected vanilla geometry, got Y {}..{}",
            f.profile.min_y,
            f.profile.max_y()
        );
    }

    /// Rule 1 of the fitting brief: the smallest legal height, never the maximum by
    /// default. The floor of "smallest" is the vanilla band, which the writer's own
    /// sections require — below that there is nothing to shrink into.
    #[test]
    fn a_flat_region_gets_a_small_world_not_the_maximum() {
        let f = fit(&req(0.0, 20.0, 1.0), caps("26.1.2")).unwrap();
        f.profile.validate().unwrap();
        assert!(!f.is_compressed());
        assert!(
            f.profile.height <= VANILLA_HEIGHT + 32,
            "20 m of relief should not produce a {}-block world",
            f.profile.height
        );
        assert!(
            f.profile.height < MAX_HEIGHT,
            "must not default to the maximum"
        );
    }

    /// A range that genuinely exceeds vanilla must produce a tall world — the whole point.
    #[test]
    fn real_relief_at_one_to_one_produces_a_tall_world() {
        // 0..2544 m at 1:1 is 2544 blocks of terrain: far beyond vanilla's 384.
        let mut r = req(0.0, 2544.0, 1.0);
        r.floor_y = -56;
        let f = fit(&r, caps("26.1.2")).unwrap();
        assert!(!f.profile.is_vanilla());
        assert!(
            f.profile.height >= 2544,
            "expected room for 2544 blocks of terrain, got {}",
            f.profile.height
        );
        assert!(
            !f.is_compressed(),
            "2544 blocks fits in 4064 without squashing"
        );
        f.profile.validate().unwrap();
    }

    #[test]
    fn reserved_room_is_actually_reserved() {
        let r = req(0.0, 100.0, 1.0);
        let f = fit(&r, caps("26.1.2")).unwrap();
        let terrain_top = r.floor_y + 100;
        assert!(
            f.profile.max_y() >= terrain_top + r.headroom,
            "peak at {terrain_top} + {} headroom must fit under max_y {}",
            r.headroom,
            f.profile.max_y()
        );
        assert!(
            f.profile.min_y <= r.floor_y - r.underroom,
            "floor {} - {} underroom must sit above min_y {}",
            r.floor_y,
            r.underroom,
            f.profile.min_y
        );
    }

    #[test]
    fn everything_is_16_aligned_and_legal() {
        for (floor_m, peak_m, scale) in [
            (0.0, 8.0, 1.0),
            (-400.0, 2544.0, 1.0),
            (0.0, 1000.0, 0.5),
            (120.0, 121.0, 4.0),
        ] {
            let f = fit(&req(floor_m, peak_m, scale), caps("26.1.2")).unwrap();
            assert_eq!(f.profile.min_y % 16, 0, "{floor_m}..{peak_m}");
            assert_eq!(f.profile.height % 16, 0, "{floor_m}..{peak_m}");
            f.profile.validate().unwrap();
        }
    }

    #[test]
    fn terrain_that_cannot_fit_is_compressed_and_reported() {
        // 9000 m of relief at 1 block/m needs 9000 blocks; only 4064 exist.
        let f = fit(&req(0.0, 9000.0, 1.0), caps("26.1.2")).unwrap();
        assert!(
            f.is_compressed(),
            "must report compression, never apply it silently"
        );
        assert!(
            f.compression > 2.0,
            "compression {} looks wrong",
            f.compression
        );
        assert!(f.profile.v_scale < 1.0);
        assert!(f.report().contains("COMPRESSED"));
        f.profile.validate().unwrap();
    }

    #[test]
    fn a_pre_117_target_refuses_instead_of_shearing_the_peaks() {
        let err = fit(&req(0.0, 2000.0, 1.0), caps("1.16.5")).unwrap_err();
        match err {
            HeightError::VersionTooOld {
                ref version,
                needed,
                available,
            } => {
                assert_eq!(version, "1.16.5");
                assert!(needed > available);
            }
            other => panic!("expected VersionTooOld, got {other:?}"),
        }
        // The message must state the floor and the way out.
        let text = err.to_string();
        assert!(text.contains("1.17"));
        assert!(text.contains("compression"));
    }

    #[test]
    fn a_pre_117_target_still_works_when_the_terrain_fits() {
        let f = fit(&req(0.0, 100.0, 1.0), caps("1.16.5")).unwrap();
        f.profile.validate().unwrap();
        assert!(!f.is_compressed());
    }

    #[test]
    fn the_datum_maps_elevation_back_to_the_requested_floor() {
        let r = req(150.0, 900.0, 1.0);
        let f = fit(&r, caps("26.1.2")).unwrap();
        assert_eq!(f.profile.elevation_to_y(150.0), r.floor_y);
        assert_eq!(f.profile.elevation_to_y(900.0), r.floor_y + 750);
    }

    fn fitted_for_tests() -> Fitted {
        let mut r = req(0.0, 100.0, 1.0);
        r.floor_y = -48;
        fit(&r, caps("26.1.2")).unwrap()
    }

    #[test]
    fn explicit_bounds_widen_the_world() {
        let f = fitted_for_tests();
        let (lo, hi) = (f.profile.min_y, f.profile.max_y());
        let out = apply_explicit_bounds(f, Some(-512), Some(1023)).unwrap();
        assert_eq!(out.profile.min_y, -512);
        assert_eq!(out.profile.max_y(), 1023);
        assert!(
            -512 < lo && 1023 > hi,
            "the test should actually be widening"
        );
        out.profile.validate().unwrap();
    }

    #[test]
    fn explicit_bounds_that_would_cut_the_terrain_are_refused() {
        let f = fitted_for_tests();
        let needed_hi = f.profile.max_y();
        let err = apply_explicit_bounds(f.clone(), None, Some(needed_hi - 16)).unwrap_err();
        assert!(err.contains("below the"), "{err}");
        assert!(err.contains("shearing the peaks off"), "{err}");

        let needed_lo = f.profile.min_y;
        let err = apply_explicit_bounds(f, Some(needed_lo + 16), None).unwrap_err();
        assert!(err.contains("cutting the ground away"), "{err}");
    }

    #[test]
    fn explicit_bounds_must_be_aligned_and_ordered() {
        let f = fitted_for_tests();
        let err = apply_explicit_bounds(f.clone(), Some(-100), None).unwrap_err();
        assert!(err.contains("multiple of 16"), "{err}");
        let err = apply_explicit_bounds(f, Some(-64), Some(-128)).unwrap_err();
        assert!(err.contains("below --min-y"), "{err}");
    }

    #[test]
    fn explicit_bounds_past_the_engine_limit_are_refused() {
        let f = fitted_for_tests();
        let err = apply_explicit_bounds(f, Some(-2048), None).unwrap_err();
        assert!(err.contains("do not describe a legal world"), "{err}");
    }

    #[test]
    fn no_explicit_bounds_changes_nothing() {
        let f = fitted_for_tests();
        let before = f.profile.clone();
        let out = apply_explicit_bounds(f, None, None).unwrap();
        assert_eq!(out.profile, before);
    }

    #[test]
    fn rounding_helpers_work_below_zero() {
        assert_eq!(floor16(0), 0);
        assert_eq!(floor16(15), 0);
        assert_eq!(floor16(-1), -16);
        assert_eq!(floor16(-16), -16);
        assert_eq!(floor16(-17), -32);
        assert_eq!(ceil16(1), 16);
        assert_eq!(ceil16(16), 16);
        assert_eq!(ceil16(-15), 0);
        assert_eq!(ceil16(-16), -16);
    }
}
