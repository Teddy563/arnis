use crate::args::Args;
use crate::coordinate_system::{
    cartesian::{XZBBox, XZPoint},
    geographic::LLBBox,
};
use crate::elevation::compute_grid_dims;
use crate::elevation_data::{fetch_elevation_data, ElevationData};
use crate::land_cover::{self, LandCoverData};
use crate::osm_parser::ProcessedElement;
#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};
use colored::Colorize;
use image::{Rgb, RgbImage};

/// Parameters describing the inverse-rotation needed to check whether a world
/// coordinate falls inside the original (pre-rotation) bounding box.
#[derive(Clone)]
pub struct RotationMask {
    /// Center of rotation (world coordinates)
    pub cx: f64,
    pub cz: f64,
    /// sin/cos of the *negative* angle (inverse rotation)
    pub neg_sin: f64,
    pub cos: f64,
    /// Original axis-aligned bounding box before rotation
    pub orig_min_x: i32,
    pub orig_max_x: i32,
    pub orig_min_z: i32,
    pub orig_max_z: i32,
}

/// Represents terrain data, land cover classification, and elevation settings
#[derive(Clone)]
pub struct Ground {
    pub elevation_enabled: bool,
    ground_level: i32,
    elevation_data: Option<ElevationData>,
    land_cover: Option<LandCoverData>,
    /// When set, coordinates outside the rotated original bbox are skipped.
    rotation_mask: Option<RotationMask>,
    /// Minecraft Y at/above which terrain is snow-capped; `i32::MAX` disables it.
    snow_threshold_y: i32,
    /// Köppen climate at the bbox center; the single-world / non-tiled fallback value.
    climate: crate::climate::Climate,
    /// Master-origin affine (origin_lat, origin_lng, scale) for turning an absolute world (x, z)
    /// back into lat/lng, so climate can be sampled per-position in tile mode (no per-cell seam).
    /// `None` off-Meld (no master origin) -> `climate_at` returns the cached `climate`.
    koppen_affine: Option<(f64, f64, f64)>,
}

/// Climatic snow line in metres by absolute latitude, piecewise-linear through
/// the cited anchors: equator 4500, subtropics (25 deg) 5700, mid-latitudes
/// (46 deg) 3000, poles 0. Source: Wikipedia "Snow line".
fn snow_line_meters(lat_deg: f64) -> f64 {
    let a = lat_deg.abs().min(90.0);
    if a <= 25.0 {
        4500.0 + (5700.0 - 4500.0) * (a / 25.0)
    } else if a <= 46.0 {
        5700.0 + (3000.0 - 5700.0) * ((a - 25.0) / (46.0 - 25.0))
    } else {
        (3000.0 * (1.0 - (a - 46.0) / (90.0 - 46.0))).max(0.0)
    }
}

/// Minecraft Y threshold for the snow line at this latitude, inverting the
/// affine metre->Y scaling. Returns `i32::MAX` (never) / `i32::MIN` (always)
/// for the flat-terrain extremes.
fn snow_threshold_for(ed: &ElevationData, lat_deg: f64, ground_level: i32) -> i32 {
    let snowline = snow_line_meters(lat_deg);
    if ed.blocks_per_meter <= 0.0 {
        return if ed.min_height_m >= snowline {
            i32::MIN
        } else {
            i32::MAX
        };
    }
    (ground_level as f64 + (snowline - ed.min_height_m) * ed.blocks_per_meter).round() as i32
}

/// How snow caps are placed on terrain.
#[derive(Clone, Copy, Debug)]
pub enum SnowMode {
    /// No snow anywhere.
    Off,
    /// Real climatic snow line by latitude (the geographically-honest default).
    Realistic,
    /// Snow on the top N% of the world's height range (always gives a cap on the tallest terrain,
    /// independent of latitude).
    Peaks,
    /// Snow at/above a fixed Minecraft Y the user picks.
    Manual,
}

/// Snow placement config, resolved from the CLI flags.
#[derive(Clone, Copy, Debug)]
pub struct SnowConfig {
    pub mode: SnowMode,
    pub percent: f64,
    pub manual_y: i32,
}

impl SnowConfig {
    pub fn from_args(mode: &str, percent: f64, manual_y: i32) -> SnowConfig {
        let mode = match mode.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => SnowMode::Off,
            "peaks" | "percent" | "top" => SnowMode::Peaks,
            "manual" | "y" | "fixed" => SnowMode::Manual,
            _ => SnowMode::Realistic,
        };
        SnowConfig {
            mode,
            percent,
            manual_y,
        }
    }
}

/// Resolve the snow-cap Y threshold for the chosen mode. `ground_level` is the world floor Y; the
/// elevation affine (`min_height_m`/`max_height_m`/`blocks_per_meter`) gives the highest terrain Y
/// for the "peaks" percentile. The same global affine is shared by every Meld tile, so the snowline
/// is identical across cells (seam-safe).
fn compute_snow_threshold(
    ed: &ElevationData,
    lat_deg: f64,
    ground_level: i32,
    cfg: SnowConfig,
) -> i32 {
    match cfg.mode {
        SnowMode::Off => i32::MAX,
        SnowMode::Realistic => snow_threshold_for(ed, lat_deg, ground_level),
        SnowMode::Manual => cfg.manual_y,
        SnowMode::Peaks => {
            // Only cap genuine peaks. On flat/rolling terrain (little vertical relief),
            // "top N%" would snow slightly-higher lowland — e.g. flat farmland plains —
            // which reads as random snow speckle. Require real relief before any snow.
            const MIN_RELIEF_M: f64 = 150.0;
            if ed.max_height_m - ed.min_height_m < MIN_RELIEF_M {
                return i32::MAX;
            }
            let max_y =
                ground_level as f64 + (ed.max_height_m - ed.min_height_m) * ed.blocks_per_meter;
            let min_y = ground_level as f64;
            let range = (max_y - min_y).max(0.0);
            let pct = cfg.percent.clamp(0.0, 100.0) / 100.0;
            (max_y - pct * range).round() as i32
        }
    }
}

/// How much room the datum reserves under the terrain for the deepest water carve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaterCarveClearance {
    /// Measure it from this tile's own land cover. Correct for a single world, WRONG for
    /// a tiled one: neighbouring tiles with different water content pick different datums.
    Measured,
    /// Reserve exactly this many blocks in every tile.
    Fixed(i32),
}

impl WaterCarveClearance {
    /// Resolve `--water-carve-clearance`: unset = measure, `max` = the engine's worst
    /// case (exact, because the carve depth is bounded), or an explicit block count.
    pub fn from_arg(spec: Option<&str>) -> Result<Self, String> {
        match spec.map(str::trim) {
            None | Some("") => Ok(Self::Measured),
            Some(s) if s.eq_ignore_ascii_case("max") => {
                Ok(Self::Fixed(crate::water_depth::MAX_WATER_DEPTH))
            }
            Some(s) if s.eq_ignore_ascii_case("auto") || s.eq_ignore_ascii_case("measure") => {
                Ok(Self::Measured)
            }
            Some(s) => s
                .parse::<i32>()
                .map(|n| Self::Fixed(n.max(0)))
                .map_err(|_| {
                    format!(
                    "--water-carve-clearance expects 'max', 'auto', or a block count; got '{s}'"
                )
                }),
        }
    }
}

impl Ground {
    pub fn new_flat(ground_level: i32) -> Self {
        Self {
            climate: crate::climate::Climate::Temperate,
            koppen_affine: None,
            elevation_enabled: false,
            ground_level,
            elevation_data: None,
            land_cover: None,
            rotation_mask: None,
            snow_threshold_y: i32::MAX,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_enabled(
        bbox: &LLBBox,
        scale: f64,
        ground_level: i32,
        fetch_land_cover: bool,
        disable_height_limit: bool,
        extended_max_y: i32,
        elevation_min: Option<f64>,
        elevation_max: Option<f64>,
        aws_only: bool,
        regional_only: bool,
        master_origin_lat: Option<f64>,
        master_origin_lng: Option<f64>,
        benchmark: bool,
        vertical_exaggeration: f64,
        snow: SnowConfig,
        carve_clearance: WaterCarveClearance,
    ) -> Self {
        let mut bench = crate::bench::Bench::new(benchmark);
        // Fetch land cover FIRST so we can feed it into the elevation
        // post-processing pipeline for land-cover-aware artifact repair.
        // The elevation grid is built from the same (bbox, scale, origin) so both
        // grids share dimensions (both use compute_grid_dims).
        let (world_w, world_h, grid_w, grid_h) =
            compute_grid_dims(bbox, scale, master_origin_lat, master_origin_lng);
        let mut land_cover = if fetch_land_cover {
            let lc = land_cover::fetch_land_cover_data(bbox, grid_w, grid_h);
            if lc.is_some() {
                println!("Land cover data loaded successfully");
            } else {
                eprintln!("Warning: Land cover data unavailable, using default ground blocks");
            }
            lc
        } else {
            None
        };
        bench.mark("elev_landcover_fetch");

        // Raise the datum so the deepest water carve still has rock under it.
        //
        // SEAM-CRITICAL: measuring the depth from THIS tile's land cover makes the datum
        // a function of how much water the tile happens to contain, so a coastal tile
        // sits ~5 blocks above its inland neighbour and the shared border becomes a
        // Y-cliff. A tiled run therefore passes a fixed clearance and every tile reserves
        // the same room. `Max` is exact rather than speculative: the carve depth is
        // bounded by MAX_WATER_DEPTH.
        let water_floor = match carve_clearance {
            WaterCarveClearance::Fixed(blocks) => {
                ground_level.max(crate::world_editor::MIN_Y + blocks + 2)
            }
            WaterCarveClearance::Measured => match &land_cover {
                Some(lc) => {
                    let max_depth = crate::water_depth::estimate_max_carve_depth(
                        &lc.grid, world_w, world_h, scale,
                    );
                    ground_level.max(crate::world_editor::MIN_Y + max_depth + 2)
                }
                None => ground_level,
            },
        };

        match fetch_elevation_data(
            bbox,
            scale,
            water_floor,
            disable_height_limit,
            extended_max_y,
            land_cover.as_mut(),
            elevation_min,
            elevation_max,
            aws_only,
            regional_only,
            master_origin_lat,
            master_origin_lng,
            benchmark,
            vertical_exaggeration,
        ) {
            Ok(elevation_data) => {
                let lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
                let snow_threshold_y =
                    compute_snow_threshold(&elevation_data, lat, water_floor, snow);
                Self {
                    climate: crate::climate::Climate::classify(bbox),
                    koppen_affine: master_origin_lat
                        .zip(master_origin_lng)
                        .map(|(la, ln)| (la, ln, scale)),
                    elevation_enabled: true,
                    ground_level: water_floor,
                    elevation_data: Some(elevation_data),
                    land_cover,
                    rotation_mask: None,
                    snow_threshold_y,
                }
            }
            Err(e) => {
                // Strict regional mode: flat-ground degradation would silently produce a
                // wrong world — abort the run instead so the orchestrator retries the cell.
                if regional_only {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
                eprintln!("Failed to fetch elevation data: {}", e);
                #[cfg(feature = "gui")]
                {
                    let short: String = e.to_string().chars().take(200).collect();
                    send_log(
                        LogLevel::Warning,
                        &format!("Elevation unavailable, using flat ground ({short})"),
                    );
                }
                // Graceful fallback: disable elevation and keep provided ground_level.
                // Land cover we already fetched is discarded since it has no
                // elevation grid to align against.
                Self {
                    climate: crate::climate::Climate::classify(bbox),
                    koppen_affine: master_origin_lat
                        .zip(master_origin_lng)
                        .map(|(la, ln)| (la, ln, scale)),
                    elevation_enabled: false,
                    ground_level,
                    elevation_data: None,
                    land_cover: None,
                    rotation_mask: None,
                    snow_threshold_y: i32::MAX,
                }
            }
        }
    }

    /// Minecraft Y at/above which terrain is snow-capped (`i32::MAX` = never,
    /// `i32::MIN` = always, e.g. a flat plateau above the snow line).
    #[inline(always)]
    pub fn snow_threshold_y(&self) -> i32 {
        self.snow_threshold_y
    }

    #[inline(always)]
    /// Köppen climate at an ABSOLUTE world (x, z). In tile mode (master origin set) this inverts
    /// the exact master-origin affine `CoordTransformer::transform_point` uses, so every cell maps
    /// a shared block to bit-identical lat/lng and picks the same Köppen class -> no per-cell seam.
    /// Off-Meld (no master origin) it returns the cached bbox-centre `climate` -> byte-identical to
    /// before. Cheap: one clamped array index into the bundled 0.1deg grid.
    pub fn climate_at(&self, x: i32, z: i32) -> crate::climate::Climate {
        let Some((origin_lat, origin_lng, scale)) = self.koppen_affine else {
            return self.climate;
        };
        // Must match transformation.rs METERS_PER_DEG_LAT and the master-origin branch exactly.
        const MPD_LAT: f64 = 111_320.0;
        let mpd_lon = MPD_LAT * origin_lat.to_radians().cos();
        let lng = origin_lng + (x as f64 + 0.5) / (mpd_lon * scale);
        let lat = origin_lat - (z as f64 + 0.5) / (MPD_LAT * scale);
        crate::climate::Climate::at(lat, lng)
    }

    /// Minecraft Y at/below which terrain counts as the bottom `fraction` of this
    /// world's terrain span, or `None` without elevation data.
    ///
    /// Under Meld's elevation lock (`--elevation-min/--elevation-max`) the affine is
    /// GLOBAL, so every cell derives the same threshold — the same property the
    /// snowline relies on, and what makes a seam-safe lowland band possible.
    #[inline]
    pub fn low_band_threshold_y(&self, fraction: f64) -> Option<i32> {
        let ed = self.elevation_data.as_ref()?;
        if !ed.blocks_per_meter.is_finite() || ed.blocks_per_meter <= 0.0 {
            return None;
        }
        let span = ((ed.max_height_m - ed.min_height_m) * ed.blocks_per_meter).max(0.0);
        Some((self.ground_level as f64 + span * fraction.clamp(0.0, 1.0)).round() as i32)
    }

    /// The real-world elevation range this world was built from, in metres, together with
    /// the blocks-per-metre the elevation pass settled on. `None` without elevation data.
    ///
    /// This is the input the height profile is fitted from, and under Meld's elevation
    /// lock it is global, so every cell fits the same geometry.
    #[inline]
    pub fn elevation_range_m(&self) -> Option<(f64, f64, f64)> {
        let ed = self.elevation_data.as_ref()?;
        Some((ed.min_height_m, ed.max_height_m, ed.blocks_per_meter))
    }

    /// Y that the lowest terrain sits at (after any water-depth floor adjustment).
    #[inline]
    pub fn ground_level(&self) -> i32 {
        self.ground_level
    }

    /// Returns whether land cover data is available
    #[inline(always)]
    pub fn has_land_cover(&self) -> bool {
        self.land_cover.is_some()
    }

    /// Force LC_WATER for every cell inside an OSM water polygon or waterway.
    pub fn apply_osm_water_override(&mut self, elements: &[ProcessedElement], xzbbox: &XZBBox) {
        let Some(lc) = self.land_cover.as_mut() else {
            return;
        };
        let Some(data) = self.elevation_data.as_ref() else {
            return;
        };
        crate::land_cover_osm_water_override::apply_osm_water_override(
            lc,
            &data.heights,
            data.world_width,
            data.world_height,
            elements,
            xzbbox,
        );
    }

    /// Reclassify cells under OSM-tagged bridges to the surrounding class.
    pub fn apply_bridge_land_cover_repair(
        &mut self,
        elements: &[ProcessedElement],
        xzbbox: &XZBBox,
        scale: f64,
    ) {
        let Some(lc) = self.land_cover.as_mut() else {
            return;
        };
        let Some(data) = self.elevation_data.as_ref() else {
            return;
        };
        crate::land_cover_bridge_repair::apply_bridge_land_cover_repair(
            lc,
            data.world_width,
            data.world_height,
            elements,
            xzbbox,
            scale,
        );
    }

    /// Local block bbox (min_x, min_z, max_x, max_z) covering all LC_WATER cells,
    /// derived from the land-cover grid; None if no land cover or no water.
    pub fn lc_water_block_bounds(&self) -> Option<(i32, i32, i32, i32)> {
        let (lc, data) = match (&self.land_cover, &self.elevation_data) {
            (Some(lc), Some(data)) => (lc, data),
            _ => return None,
        };
        let (mut gx0, mut gz0, mut gx1, mut gz1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        let mut any = false;
        for (z, row) in lc.grid.iter().enumerate() {
            for (x, &c) in row.iter().enumerate() {
                if c == land_cover::LC_WATER {
                    gx0 = gx0.min(x);
                    gx1 = gx1.max(x);
                    gz0 = gz0.min(z);
                    gz1 = gz1.max(z);
                    any = true;
                }
            }
        }
        if !any {
            return None;
        }
        let (x0, x1) =
            crate::water_depth::grid_span_to_block_span(gx0, gx1, data.world_width, lc.width);
        let (z0, z1) =
            crate::water_depth::grid_span_to_block_span(gz0, gz1, data.world_height, lc.height);
        Some((x0, z0, x1, z1))
    }

    /// Returns the ESA WorldCover land cover class at the given coordinates.
    /// Returns 0 if land cover data is not available.
    #[inline(always)]
    pub fn cover_class(&self, coord: XZPoint) -> u8 {
        if let (Some(ref lc), Some(ref data)) = (&self.land_cover, &self.elevation_data) {
            let x_ratio = (coord.x as f64 / (data.world_width - 1).max(1) as f64).clamp(0.0, 1.0);
            let z_ratio = (coord.z as f64 / (data.world_height - 1).max(1) as f64).clamp(0.0, 1.0);
            let x = ((x_ratio * (lc.width - 1) as f64).round() as usize).min(lc.width - 1);
            let z = ((z_ratio * (lc.height - 1) as f64).round() as usize).min(lc.height - 1);
            lc.grid[z][x]
        } else {
            0
        }
    }

    /// Returns the water distance-to-shore value at the given coordinates.
    /// 0 = non-water, 1 = shore, 2+ = progressively deeper water.
    #[inline(always)]
    pub fn water_distance(&self, coord: XZPoint) -> u8 {
        if let (Some(ref lc), Some(ref data)) = (&self.land_cover, &self.elevation_data) {
            let x_ratio = (coord.x as f64 / (data.world_width - 1).max(1) as f64).clamp(0.0, 1.0);
            let z_ratio = (coord.z as f64 / (data.world_height - 1).max(1) as f64).clamp(0.0, 1.0);
            let x = ((x_ratio * (lc.width - 1) as f64).round() as usize).min(lc.width - 1);
            let z = ((z_ratio * (lc.height - 1) as f64).round() as usize).min(lc.height - 1);
            lc.water_distance[z][x]
        } else {
            0
        }
    }

    /// Returns a continuous 0.0–1.0 value indicating how "watery" a block is,
    /// using bilinear interpolation of the water classification grid.
    ///
    /// Nearest-neighbor grid lookups (`cover_class`) create rectangular water
    /// edges when the grid is coarser than block resolution.  Bilinear
    /// interpolation produces a smooth gradient across grid cell boundaries,
    /// allowing noise-based thresholding to create organic shorelines.
    #[inline(always)]
    pub fn water_blend(&self, coord: XZPoint) -> f64 {
        if let (Some(ref lc), Some(ref data)) = (&self.land_cover, &self.elevation_data) {
            // Continuous grid coordinates (no rounding — that's the key difference
            // from cover_class which uses .round())
            let fx = (coord.x as f64 / (data.world_width - 1).max(1) as f64).clamp(0.0, 1.0)
                * (lc.width - 1) as f64;
            let fz = (coord.z as f64 / (data.world_height - 1).max(1) as f64).clamp(0.0, 1.0)
                * (lc.height - 1) as f64;

            let x0 = (fx.floor() as usize).min(lc.width - 1);
            let x1 = (x0 + 1).min(lc.width - 1);
            let z0 = (fz.floor() as usize).min(lc.height - 1);
            let z1 = (z0 + 1).min(lc.height - 1);

            let tx = fx - fx.floor();
            let tz = fz - fz.floor();

            // Sample pre-smoothed water-ness at the 4 surrounding grid cells.
            // The grid was Gaussian-blurred from the binary LC_WATER mask so
            // that even at integer block positions (1-to-1 grid-to-world
            // mapping, where tx == tz == 0 below) the sampled value is
            // continuous — the renderer's hard `> 0.5` threshold then traces
            // a clean curved shoreline contour instead of the raw ESA 10 m
            // rectangular grid edge.
            // Widen f32 storage to f64 for the bilinear arithmetic. This
            // doesn't recover the ~10⁻⁷ precision lost at storage, but it
            // prevents extra rounding from accumulating in the four
            // multiply-adds + the threshold comparison downstream.
            let w00 = lc.water_blend_grid[z0][x0] as f64;
            let w10 = lc.water_blend_grid[z0][x1] as f64;
            let w01 = lc.water_blend_grid[z1][x0] as f64;
            let w11 = lc.water_blend_grid[z1][x1] as f64;

            // Bilinear interpolation
            let top = w00 * (1.0 - tx) + w10 * tx;
            let bottom = w01 * (1.0 - tx) + w11 * tx;
            top * (1.0 - tz) + bottom * tz
        } else {
            0.0
        }
    }

    /// Computes terrain slope at the given coordinates.
    ///
    /// Slope is the difference between the maximum and minimum elevation of
    /// 4 cardinal neighbors sampled at a step distance. Higher values indicate
    /// steeper terrain.
    ///
    /// Returns 0 if elevation data is not available.
    #[inline(always)]
    pub fn slope(&self, coord: XZPoint) -> i32 {
        if !self.elevation_enabled {
            return 0;
        }

        const STEP: i32 = 4;
        let east = self.level(XZPoint::new(coord.x + STEP, coord.z));
        let west = self.level(XZPoint::new(coord.x - STEP, coord.z));
        let north = self.level(XZPoint::new(coord.x, coord.z - STEP));
        let south = self.level(XZPoint::new(coord.x, coord.z + STEP));

        let max_val = east.max(west).max(north).max(south);
        let min_val = east.min(west).min(north).min(south);
        // Saturate: pathological CLI input (e.g. very negative ground_level)
        // can push max - min past i32::MAX.
        max_val.saturating_sub(min_val)
    }

    /// Returns the ground level at the given coordinates
    #[inline(always)]
    pub fn level(&self, coord: XZPoint) -> i32 {
        if !self.elevation_enabled || self.elevation_data.is_none() {
            return self.ground_level;
        }

        let data: &ElevationData = self.elevation_data.as_ref().unwrap();
        let (x_ratio, z_ratio) = self.get_data_coordinates(coord, data);
        self.interpolate_height(x_ratio, z_ratio, data)
    }

    /// Returns the appropriate Y level for water placement.
    /// On steep terrain, snaps to the local minimum within a small radius
    /// to compensate for spatial misalignment between water classification
    /// data (OSM/ESA) and the elevation DEM.
    pub fn water_level(&self, coord: XZPoint) -> i32 {
        let center = self.level(coord);
        if !self.elevation_enabled {
            return center;
        }
        // Check if terrain is steep here; if flat, no snapping needed
        let slope = self.slope(coord);
        if slope <= 2 {
            return center;
        }
        // On steep terrain, snap to the local minimum within SNAP_RADIUS to
        // correct small DEM-vs-water misalignment.
        const SNAP_RADIUS: i32 = 3;
        let mut min_y = center;
        for r in 1..=SNAP_RADIUS {
            for &(dx, dz) in &[
                (-r, 0),
                (r, 0),
                (0, -r),
                (0, r),
                (-r, -r),
                (-r, r),
                (r, -r),
                (r, r),
            ] {
                let neighbor = self.level(XZPoint::new(coord.x + dx, coord.z + dz));
                min_y = min_y.min(neighbor);
            }
        }
        // A drop larger than the snap radius is a real cliff/falls, not
        // misalignment; snapping across it terraces the waterfront into a step.
        // saturating_sub guards against overflow on pathological elevations.
        if center.saturating_sub(min_y) > SNAP_RADIUS {
            return center;
        }
        min_y
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn min_level<I: Iterator<Item = XZPoint>>(&self, coords: I) -> Option<i32> {
        if !self.elevation_enabled {
            return Some(self.ground_level);
        }
        coords.map(|c: XZPoint| self.level(c)).min()
    }

    #[allow(unused)]
    #[inline(always)]
    pub fn max_level<I: Iterator<Item = XZPoint>>(&self, coords: I) -> Option<i32> {
        if !self.elevation_enabled {
            return Some(self.ground_level);
        }
        coords.map(|c: XZPoint| self.level(c)).max()
    }

    /// Converts game coordinates to elevation data coordinates (0.0 to 1.0 ratio)
    #[inline(always)]
    fn get_data_coordinates(&self, coord: XZPoint, data: &ElevationData) -> (f64, f64) {
        let x_ratio: f64 = coord.x as f64 / (data.world_width - 1).max(1) as f64;
        let z_ratio: f64 = coord.z as f64 / (data.world_height - 1).max(1) as f64;
        (x_ratio.clamp(0.0, 1.0), z_ratio.clamp(0.0, 1.0))
    }

    /// Bilinearly interpolates height value from the elevation grid
    #[inline(always)]
    fn interpolate_height(&self, x_ratio: f64, z_ratio: f64, data: &ElevationData) -> i32 {
        let fx = x_ratio * (data.width - 1) as f64;
        let fz = z_ratio * (data.height - 1) as f64;
        let x0 = fx.floor() as usize;
        let z0 = fz.floor() as usize;
        let x1 = (x0 + 1).min(data.width - 1);
        let z1 = (z0 + 1).min(data.height - 1);
        let dx = fx - x0 as f64;
        let dz = fz - z0 as f64;
        // Widen f32 storage to f64 for the bilinear arithmetic. The real
        // property we rely on: across the Minecraft Y range (roughly −64 up
        // through a few thousand even with --disable-height-limit), f32's
        // mantissa gives ~10⁻⁷ precision per stored cell, which is far
        // smaller than the 0.5-block half-width used by `round()` below.
        // So for any value that isn't pathologically close to a half-integer
        // boundary, the final `result.round() as i32` matches the f64 path.
        let v00 = data.heights[z0][x0] as f64;
        let v10 = data.heights[z0][x1] as f64;
        let v01 = data.heights[z1][x0] as f64;
        let v11 = data.heights[z1][x1] as f64;
        let lerp_top = v00 + (v10 - v00) * dx;
        let lerp_bot = v01 + (v11 - v01) * dx;
        let result = lerp_top + (lerp_bot - lerp_top) * dz;
        result.round() as i32
    }

    /// Replace the elevation grid with new rotated/transformed data.
    /// Used by the rotation operator to update elevation after rotating.
    pub fn set_elevation_data(
        &mut self,
        heights: Vec<Vec<f64>>,
        grid_width: usize,
        grid_height: usize,
        world_width: usize,
        world_height: usize,
    ) {
        if let Some(ref mut data) = self.elevation_data {
            // Rotation operators build a fresh f64 work grid; downcast here to
            // match `ElevationData::heights`'s f32 storage layout.
            data.heights = heights
                .into_iter()
                .map(|row| row.into_iter().map(|v| v as f32).collect())
                .collect();
            data.width = grid_width;
            data.height = grid_height;
            data.world_width = world_width;
            data.world_height = world_height;
        }
    }

    /// Replace the land-cover grids with new rotated/transformed data.
    /// Used by the rotation operator to keep land cover aligned with elevation.
    pub fn set_land_cover_data(
        &mut self,
        grid: Vec<Vec<u8>>,
        water_distance: Vec<Vec<u8>>,
        width: usize,
        height: usize,
    ) {
        if let Some(ref mut lc) = self.land_cover {
            lc.grid = grid;
            lc.water_distance = water_distance;
            lc.width = width;
            lc.height = height;
            // The water-blend mask was derived from the pre-rotation grid —
            // refresh it from the rotated grid so the shoreline softening
            // stays aligned with the new classification.
            lc.refresh_water_blend_grid();
        }
    }

    /// Store rotation parameters so we can mask out-of-bounds blocks later.
    pub fn set_rotation_mask(&mut self, mask: RotationMask) {
        self.rotation_mask = Some(mask);
    }

    /// Returns `true` if the coordinate is inside the rotated original bbox.
    /// When no rotation was applied, always returns `true`.
    #[inline(always)]
    pub fn is_in_rotated_bounds(&self, x: i32, z: i32) -> bool {
        let mask = match self.rotation_mask {
            Some(ref m) => m,
            None => return true,
        };
        // Inverse-rotate (x, z) back to original space
        let dx = x as f64 - mask.cx;
        let dz = z as f64 - mask.cz;
        let orig_x = dx * mask.cos + dz * mask.neg_sin + mask.cx;
        let orig_z = -dx * mask.neg_sin + dz * mask.cos + mask.cz;
        // Allow a tiny tolerance so points that land infinitesimally outside the
        // integer bbox due to floating-point rounding are still considered inside.
        const EPSILON: f64 = 1.0e-9;
        orig_x >= mask.orig_min_x as f64 - EPSILON
            && orig_x <= mask.orig_max_x as f64 + EPSILON
            && orig_z >= mask.orig_min_z as f64 - EPSILON
            && orig_z <= mask.orig_max_z as f64 + EPSILON
    }

    pub fn save_land_cover_debug_image(&self, filename: &str) {
        let Some(ref lc) = self.land_cover else {
            return;
        };
        if lc.height == 0 || lc.width == 0 {
            return;
        }
        let mut img: image::ImageBuffer<Rgb<u8>, Vec<u8>> =
            RgbImage::new(lc.width as u32, lc.height as u32);
        for (y, row) in lc.grid.iter().enumerate() {
            for (x, &class) in row.iter().enumerate() {
                let color = match class {
                    land_cover::LC_TREE_COVER => Rgb([0x00, 0x6e, 0x00]),
                    land_cover::LC_SHRUBLAND => Rgb([0xff, 0xbb, 0x22]),
                    land_cover::LC_GRASSLAND => Rgb([0xff, 0xff, 0x4c]),
                    land_cover::LC_CROPLAND => Rgb([0xf0, 0x96, 0xff]),
                    land_cover::LC_BUILT_UP => Rgb([0xfa, 0x00, 0x00]),
                    land_cover::LC_BARE => Rgb([0xb4, 0xb4, 0xb4]),
                    land_cover::LC_SNOW_ICE => Rgb([0xf0, 0xf0, 0xf0]),
                    land_cover::LC_WATER => Rgb([0x00, 0x64, 0xc8]),
                    land_cover::LC_WETLAND => Rgb([0x00, 0x96, 0xa0]),
                    land_cover::LC_MANGROVES => Rgb([0x00, 0xcf, 0x75]),
                    land_cover::LC_MOSS => Rgb([0xfa, 0xe6, 0xa0]),
                    _ => Rgb([0x00, 0x00, 0x00]),
                };
                img.put_pixel(x as u32, y as u32, color);
            }
        }
        let filename: String = if !filename.ends_with(".png") {
            format!("{filename}.png")
        } else {
            filename.to_string()
        };
        if let Err(e) = img.save(&filename) {
            eprintln!("Failed to save land cover debug image: {e}");
        }
    }

    fn save_debug_image(&self, filename: &str) {
        let heights = &self
            .elevation_data
            .as_ref()
            .expect("Elevation data not available")
            .heights;
        if heights.is_empty() || heights[0].is_empty() {
            return;
        }

        let height: usize = heights.len();
        let width: usize = heights[0].len();
        let mut img: image::ImageBuffer<Rgb<u8>, Vec<u8>> =
            RgbImage::new(width as u32, height as u32);

        let mut min_height: f32 = f32::MAX;
        let mut max_height: f32 = f32::MIN;

        for row in heights {
            for &h in row {
                if h.is_finite() {
                    min_height = min_height.min(h);
                    max_height = max_height.max(h);
                }
            }
        }

        let range = max_height - min_height;
        for (y, row) in heights.iter().enumerate() {
            for (x, &h) in row.iter().enumerate() {
                let normalized: u8 = if range > 0.0 {
                    (((h - min_height) / range) * 255.0) as u8
                } else {
                    128
                };
                img.put_pixel(
                    x as u32,
                    y as u32,
                    Rgb([normalized, normalized, normalized]),
                );
            }
        }

        // Ensure filename has .png extension
        let filename: String = if !filename.ends_with(".png") {
            format!("{filename}.png")
        } else {
            filename.to_string()
        };

        if let Err(e) = img.save(&filename) {
            eprintln!("Failed to save debug image: {e}");
        }
    }
}

pub fn generate_ground_data(args: &Args) -> Ground {
    if args.terrain {
        println!("{} Fetching elevation...", "[3/7]".bold());
        let ground = Ground::new_enabled(
            &args.bbox,
            args.scale,
            args.ground_level,
            args.land_cover,
            args.disable_height_limit,
            extended_max_y_for(args),
            args.elevation_min,
            args.elevation_max,
            args.aws_only_elevation,
            args.regional_elevation_only,
            args.master_origin_lat,
            args.master_origin_lng,
            args.benchmark,
            args.vertical_exaggeration,
            SnowConfig::from_args(&args.snow_mode, args.snow_percent, args.snow_y),
            match WaterCarveClearance::from_arg(args.water_carve_clearance.as_deref()) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{} {}", "Error:".red().bold(), e);
                    std::process::exit(1);
                }
            },
        );
        if args.debug {
            ground.save_debug_image("elevation_debug");
            ground.save_land_cover_debug_image("landcover_debug");
        }
        return ground;
    }
    Ground::new_flat(args.ground_level)
}

/// Per-format build-height cap when the user opts into extended build height:
/// 2031 for the Java datapack, 512 for the Bedrock behavior pack.
pub(crate) fn extended_max_y_for(args: &Args) -> i32 {
    if args.bedrock {
        512
    } else {
        2031
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::cartesian::XZPoint;
    use crate::elevation_data::ElevationData;

    fn ground_with(heights: Vec<Vec<f32>>) -> Ground {
        let h = heights.len();
        let w = heights[0].len();
        Ground {
            climate: crate::climate::Climate::Temperate,
            koppen_affine: None,
            elevation_enabled: true,
            ground_level: 0,
            elevation_data: Some(ElevationData {
                heights,
                width: w,
                height: h,
                world_width: w,
                world_height: h,
                min_height_m: 0.0,
                max_height_m: 0.0,
                blocks_per_meter: 1.0,
            }),
            land_cover: None,
            rotation_mask: None,
            snow_threshold_y: i32::MAX,
        }
    }

    // Water snaps to the local floor over small DEM steps, but not across a real cliff.
    #[test]
    fn water_level_snaps_small_steps_not_cliffs() {
        // Flat terrain: no snap, returns the cell's own level.
        let flat = ground_with(vec![vec![5.0; 16]; 16]);
        assert_eq!(flat.water_level(XZPoint::new(8, 8)), 5);

        // 3-block step: snaps down to the nearby floor.
        let step = ground_with(
            (0..16)
                .map(|_| (0..16).map(|x| if x <= 7 { 10.0 } else { 7.0 }).collect())
                .collect(),
        );
        assert_eq!(step.water_level(XZPoint::new(7, 8)), 7);

        // Real cliff (30-block drop): keeps its own level, no terracing.
        let cliff = ground_with(
            (0..16)
                .map(|_| (0..16).map(|x| if x <= 7 { 30.0 } else { 0.0 }).collect())
                .collect(),
        );
        assert_eq!(cliff.water_level(XZPoint::new(7, 8)), 30);
    }

    #[test]
    fn snow_line_follows_latitude() {
        assert!((snow_line_meters(0.0) - 4500.0).abs() < 1.0);
        assert!((snow_line_meters(25.0) - 5700.0).abs() < 1.0);
        assert!((snow_line_meters(46.0) - 3000.0).abs() < 1.0);
        assert!(snow_line_meters(90.0).abs() < 1.0);
        // Symmetric across the equator.
        assert_eq!(snow_line_meters(-46.0), snow_line_meters(46.0));
    }

    #[test]
    fn snow_threshold_inverts_the_scale() {
        let ed = |min_m: f64, bpm: f64| ElevationData {
            heights: vec![vec![0.0; 2]; 2],
            width: 2,
            height: 2,
            world_width: 2,
            world_height: 2,
            min_height_m: min_m,
            max_height_m: min_m,
            blocks_per_meter: bpm,
        };
        // 46 deg snow line is 3000 m; at 0.1 block/m from min 0 m, ground 64 => Y 364.
        assert_eq!(snow_threshold_for(&ed(0.0, 0.1), 46.0, 64), 364);
        // Flat terrain: never below the line, always above it.
        assert_eq!(snow_threshold_for(&ed(100.0, 0.0), 46.0, 64), i32::MAX);
        assert_eq!(snow_threshold_for(&ed(4000.0, 0.0), 46.0, 64), i32::MIN);
    }

    #[test]
    fn snow_peaks_caps_the_top_percent() {
        // min 0 m -> Y0, range 1000 m at 0.1 block/m -> peak Y100. Top 10% -> snow above Y90.
        let ed = ElevationData {
            heights: vec![vec![0.0; 2]; 2],
            width: 2,
            height: 2,
            world_width: 2,
            world_height: 2,
            min_height_m: 0.0,
            max_height_m: 1000.0,
            blocks_per_meter: 0.1,
        };
        let cfg = SnowConfig::from_args("peaks", 10.0, 0);
        assert_eq!(compute_snow_threshold(&ed, 46.0, 0, cfg), 90);
        // off + manual.
        assert_eq!(
            compute_snow_threshold(&ed, 46.0, 0, SnowConfig::from_args("off", 0.0, 0)),
            i32::MAX
        );
        assert_eq!(
            compute_snow_threshold(&ed, 46.0, 0, SnowConfig::from_args("manual", 0.0, 55)),
            55
        );
    }
}
