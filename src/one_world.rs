//! One World: one persistent Java world that every generation extends.
//!
//! The world pins a Web Mercator frame (origin and scale in the manifest next
//! to `level.dat`). Every requested area is snapped outward to whole chunks in
//! that frame and written into the existing region files, so areas generated
//! at different times line up block for block.

use crate::args::Args;
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::LLBBox;
use crate::elevation::ElevationAffine;
use crate::projection::{snap_bbox_to_chunks, WebMercatorProjection};
use crate::world_utils::{world_is_locked, SessionLock};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "arnis_one_world.json";
pub const PREVIEW_DIR: &str = "arnis_one_world/previews";
pub const DEFAULT_WORLD_NAME: &str = "Arnis One World";
/// 2: merges write into empty region files. Version 1 worlds still hold
/// region template chunks outside their areas and are repaired once.
/// 3: new worlds have the extended build height and a whole-Earth elevation
/// mapping, which older builds would not apply.
pub const MANIFEST_VERSION: u32 = 3;

/// Geometry kept past the area edge, so an element straddling a seam is
/// built whole on both sides.
pub const CLIP_PAD_BLOCKS: i32 = 64;

/// How far past its own chunks a piece of a job builds, writing only its own:
/// a tree rooted in a neighbour's ground spreads its crown across the seam,
/// and a single run plants it from there. The widest bundled tree reaches 20
/// blocks from its trunk, which snaps up to 6 blocks from the cell asking.
// ponytail: fixed reach; a --tree-pack-dir tree over ~50 blocks wide still loses crown at a seam.
pub const PIECE_HALO_BLOCKS: i32 = 32;
// Whole chunks, so the piece's own chunk snap gives its build rect back exactly.
const _: () = assert!(PIECE_HALO_BLOCKS % 16 == 0);

/// What a piece over `rect` builds: `rect` grown by `PIECE_HALO_BLOCKS`, but
/// never past the job's `selection`, which a single run would not build either.
pub fn piece_build_rect(rect: &XZBBox, selection: &XZBBox) -> XZBBox {
    let h = PIECE_HALO_BLOCKS;
    XZBBox::rect_from_min_max(
        (rect.min_x() - h).max(selection.min_x()),
        (rect.min_z() - h).max(selection.min_z()),
        (rect.max_x() + h).min(selection.max_x()),
        (rect.max_z() + h).min(selection.max_z()),
    )
    .unwrap_or_else(|_| rect.clone())
}

const MAX_ABS_LAT: f64 = 85.0;

/// Rounds to `decimals` places. serde_json reads such short decimals back
/// exactly, but not every f64 it writes, and the first run has to use the same
/// numbers as the runs that load them.
fn stable(v: f64, decimals: i32) -> f64 {
    let f = 10f64.powi(decimals);
    (v * f).round() / f
}

/// Ground data is fetched this far past the area and cropped again, so the
/// smoothing passes (widest: built-up Gaussian, ~90 m) agree across seams, also
/// at geometry kept up to `CLIP_PAD_BLOCKS` past the edge.
pub fn ground_pad_blocks(scale: f64) -> i32 {
    ((100.0 * scale).ceil() as i32).max(96) + CLIP_PAD_BLOCKS
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GeneratedArea {
    pub id: u32,
    /// Unix seconds.
    pub generated_at: u64,
    pub arnis_version: String,
    pub min_x: i32,
    pub min_z: i32,
    pub max_x: i32,
    pub max_z: i32,
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
    pub preview: Option<String>,
}

impl GeneratedArea {
    fn is_inside(&self, other: &XZBBox) -> bool {
        other.min_x() <= self.min_x
            && other.min_z() <= self.min_z
            && other.max_x() >= self.max_x
            && other.max_z() >= self.max_z
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Manifest {
    pub version: u32,
    pub created_with: String,
    /// Unix seconds.
    pub created_at: u64,
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub scale: f64,
    pub ground_level: i32,
    pub terrain: bool,
    pub disable_height_limit: bool,
    pub aws_only_elevation: bool,
    /// Already folded into `elevation`; kept so later areas are told which one applies.
    #[serde(default = "real_height", skip_serializing_if = "is_real_height")]
    pub height_multiplier: f64,
    /// Metre to Y mapping shared by every area. Set at creation; worlds from
    /// before version 3 take it from their first terrain area.
    pub elevation: Option<ElevationAffine>,
    /// `--cave-seed` of the first area; later areas and pieces carve with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cave_seed: Option<u64>,
    /// `--cave-datum-y` of the first area, the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cave_datum_y: Option<i32>,
    /// `--seed` of the first area; later areas and pieces build with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// Top zoom of the Mapterhorn pyramid every area samples, without the AWS fallback.
    /// Set at creation; absent (older or legacy-terrain worlds) each area picks its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation_zoom: Option<u8>,
    pub next_area_id: u32,
    pub areas: Vec<GeneratedArea>,
}

fn real_height() -> f64 {
    1.0
}

fn is_real_height(multiplier: &f64) -> bool {
    *multiplier == 1.0
}

impl Manifest {
    fn new(args: &Args, origin_lat: f64, origin_lon: f64) -> Self {
        let scale = stable(args.scale, 9);
        let height_multiplier = stable(args.height_multiplier, 6);
        Self {
            version: MANIFEST_VERSION,
            created_with: format!("arnis {}", env!("CARGO_PKG_VERSION")),
            created_at: unix_now(),
            origin_lat: stable(origin_lat, 7),
            origin_lon: stable(origin_lon, 7),
            scale,
            ground_level: args.ground_level,
            terrain: args.terrain(),
            disable_height_limit: args.disable_height_limit,
            aws_only_elevation: args.aws_only_elevation,
            height_multiplier,
            // One section up, so water carved at the lowest level stays above the floor.
            elevation: args.terrain().then(|| {
                let mut e = ElevationAffine::whole_earth(
                    scale * height_multiplier,
                    crate::ground::min_ground_level_for(args) + 16,
                    crate::ground::extended_max_y_for(args),
                );
                if let Some(top) = e.soft_top.as_mut() {
                    top.knee_m = stable(top.knee_m, 6);
                    // Down, so Everest stays under the ceiling.
                    top.width_blocks = stable(top.width_blocks - 5e-7, 6);
                }
                e
            }),
            cave_seed: args.cave_seed,
            cave_datum_y: args.cave_datum_y,
            seed: args.seed,
            elevation_zoom: (args.terrain() && !args.aws_only_elevation).then(|| {
                crate::elevation::providers::mapterhorn::frame_zoom(stable(origin_lat, 7), scale)
            }),
            next_area_id: 1,
            areas: Vec::new(),
        }
    }

    pub fn projection(&self) -> WebMercatorProjection {
        WebMercatorProjection::new(self.origin_lat, self.origin_lon, self.scale)
    }

    pub fn path_in(world_dir: &Path) -> PathBuf {
        world_dir.join(MANIFEST_FILE)
    }

    pub fn load(world_dir: &Path) -> Result<Option<Self>, String> {
        let path = Self::path_in(world_dir);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
        let manifest: Manifest = serde_json::from_str(&text)
            .map_err(|e| format!("{} is not a valid One World manifest: {e}", path.display()))?;
        if manifest.version > MANIFEST_VERSION {
            return Err(format!(
                "{} was written by a newer Arnis (manifest version {}); update Arnis to extend this world.",
                path.display(),
                manifest.version
            ));
        }
        manifest
            .validate()
            .map_err(|e| format!("{} is damaged: {e}", path.display()))?;
        Ok(Some(manifest))
    }

    fn validate(&self) -> Result<(), String> {
        if !(self.origin_lat.is_finite() && self.origin_lat.abs() <= MAX_ABS_LAT) {
            return Err(format!("origin latitude {}", self.origin_lat));
        }
        if !(self.origin_lon.is_finite() && self.origin_lon.abs() <= 180.0) {
            return Err(format!("origin longitude {}", self.origin_lon));
        }
        crate::args::validate_scale(self.scale)?;
        crate::args::validate_height_multiplier(self.height_multiplier)?;
        if let Some(e) = &self.elevation {
            let soft_top_ok = e.soft_top.is_none_or(|t| {
                t.knee_m.is_finite() && t.width_blocks.is_finite() && t.width_blocks > 0.0
            });
            if !(e.min_height_m.is_finite()
                && e.blocks_per_meter.is_finite()
                && e.blocks_per_meter >= 0.0
                && soft_top_ok)
            {
                return Err("elevation mapping".to_string());
            }
        }
        if let Some(a) = self
            .areas
            .iter()
            .find(|a| a.min_x > a.max_x || a.min_z > a.max_z)
        {
            return Err(format!("area {} has an empty rectangle", a.id));
        }
        Ok(())
    }

    pub fn save(&self, world_dir: &Path) -> Result<(), String> {
        let path = Self::path_in(world_dir);
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize the One World manifest: {e}"))?;
        crate::world_utils::replace_file_atomically(&path, text.as_bytes())
            .map_err(|e| format!("Failed to write {}: {e}", path.display()))
    }

    pub fn extent(&self) -> Option<XZBBox> {
        let first = self.areas.first()?;
        let mut min_x = first.min_x;
        let mut min_z = first.min_z;
        let mut max_x = first.max_x;
        let mut max_z = first.max_z;
        for a in &self.areas[1..] {
            min_x = min_x.min(a.min_x);
            min_z = min_z.min(a.min_z);
            max_x = max_x.max(a.max_x);
            max_z = max_z.max(a.max_z);
        }
        XZBBox::rect_from_min_max(min_x, min_z, max_x, max_z).ok()
    }
}

/// Chunks inside `rect` that already exist in the world's region files,
/// read from the region headers alone.
pub fn existing_chunks(world_dir: &Path, rect: &XZBBox) -> u64 {
    let (cx0, cz0) = (rect.min_x() >> 4, rect.min_z() >> 4);
    let (cx1, cz1) = (rect.max_x() >> 4, rect.max_z() >> 4);
    let region_dir = crate::world_utils::WorldLayout::of(world_dir)
        .overworld_dir(world_dir)
        .join("region");
    let mut count = 0;
    for rz in (cz0 >> 5)..=(cz1 >> 5) {
        for rx in (cx0 >> 5)..=(cx1 >> 5) {
            let path = region_dir.join(format!("r.{rx}.{rz}.mca"));
            let mut header = [0u8; 4096];
            let read = std::fs::File::open(&path).and_then(|mut f| f.read_exact(&mut header));
            if read.is_err() {
                continue;
            }
            for lz in 0..32 {
                for lx in 0..32 {
                    let (cx, cz) = (rx * 32 + lx, rz * 32 + lz);
                    let i = 4 * (lx + lz * 32) as usize;
                    if (cx0..=cx1).contains(&cx)
                        && (cz0..=cz1).contains(&cz)
                        && header[i..i + 4] != [0; 4]
                    {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}

/// The One World a run lands in, carried in `Args::one_world_run`.
#[derive(Clone, Debug)]
pub struct RunContext {
    pub world_dir: PathBuf,
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub extending: bool,
    pub elevation: Option<ElevationAffine>,
    /// `Manifest::elevation_zoom`.
    pub elevation_zoom: Option<u8>,
    /// Chunks of this area that already exist and are replaced.
    pub replaced_chunks: u64,
    pub area_id: u32,
    /// Set when this run is one piece of a larger job (`--one-world-unit`).
    pub unit: Option<UnitLease>,
}

impl RunContext {
    pub fn preview_path(&self) -> PathBuf {
        match &self.unit {
            Some(unit) => unit.preview.clone(),
            None => self
                .world_dir
                .join(PREVIEW_DIR)
                .join(format!("area-{}.png", self.area_id)),
        }
    }
}

/// Where a job coordinator keeps its jobs, inside the world folder.
pub const JOBS_DIR: &str = "arnis_one_world/jobs";
/// Written by the coordinator while it holds the world; a piece only runs
/// while this still carries its lease's nonce.
pub const COORDINATOR_FILE: &str = "arnis_one_world/jobs/coordinator";

/// What a coordinator hands one piece of a job. The piece writes its own
/// chunks and nothing that belongs to the world as a whole: the manifest,
/// level.dat and the map id counter stay with the coordinator.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UnitLease {
    pub nonce: String,
    pub piece: usize,
    pub area_id: u32,
    /// min_x, min_z, max_x, max_z; the piece's bbox must snap to exactly this.
    pub rect: [i32; 4],
    /// What the piece builds (`piece_build_rect`); it writes only `rect`.
    pub build: [i32; 4],
    /// The job as one run would take the tile-parallel path.
    pub tiled: bool,
    /// Signage map ids of this piece: `first_map_id..map_id_end`.
    pub first_map_id: i32,
    pub map_id_end: i32,
    /// The job's spawn, given to the piece that contains it, which reports
    /// the ground height there.
    pub spawn: Option<[i32; 2]>,
    /// The world's branding frame, given to the piece that contains it:
    /// x, z, and whether the map item sits beside it (map ids 0 and 1).
    pub branding: Option<(i32, i32, bool)>,
    pub preview: PathBuf,
}

/// A resolved run. Holds the world's session lock until dropped.
pub struct Session {
    pub created: bool,
    pub llbbox: LLBBox,
    pub lock: SessionLock,
}

fn compatibility_errors(manifest: &Manifest, args: &Args) -> Vec<String> {
    let mut errors = Vec::new();
    if (manifest.scale - args.scale).abs() > 1e-9 {
        errors.push(format!(
            "world scale {:.2} does not match the world's {:.2}",
            args.scale, manifest.scale
        ));
    }
    if manifest.ground_level != args.ground_level {
        errors.push(format!(
            "ground level {} does not match the world's {}",
            args.ground_level, manifest.ground_level
        ));
    }
    if manifest.terrain != args.terrain() {
        errors.push(format!(
            "the world was generated {} terrain, so this area must be too",
            if manifest.terrain { "with" } else { "without" }
        ));
    }
    errors
}

/// Caves are a pure function of the seed and the datum, so a world keeps its first area's (none
/// means the built-in seed and each run's own floor): a later area or piece with others would
/// not line up with its neighbours underground. `--seed` the same way, for everything else.
fn keep_cave_settings(manifest: &Manifest, args: &mut Args) {
    if args.seed.is_some() && args.seed != manifest.seed {
        let kept = manifest.seed.map_or("none".into(), |s| s.to_string());
        println!("Note: One World keeps the seed it was created with ({kept}).");
    }
    args.seed = manifest.seed;
    if args.cave_seed.is_some() && args.cave_seed != manifest.cave_seed {
        let kept = manifest
            .cave_seed
            .map_or("the built-in one".into(), |s| s.to_string());
        println!("Note: One World keeps the cave seed it was created with ({kept}).");
    }
    args.cave_seed = manifest.cave_seed;
    if args.cave_datum_y.is_some() && args.cave_datum_y != manifest.cave_datum_y {
        let kept = manifest
            .cave_datum_y
            .map_or("none".into(), |y| format!("Y {y}"));
        println!("Note: One World keeps the cave datum it was created with ({kept}).");
    }
    args.cave_datum_y = manifest.cave_datum_y;
}

/// The block rectangle a One World run over `requested` builds, for a preview that writes no
/// world and so takes no lock: the manifest's frame, or the one a new world would get. Also
/// applies the cave settings the world keeps.
pub fn preview_rect(
    world_dir: Option<&Path>,
    requested: &LLBBox,
    args: &mut Args,
) -> Result<XZBBox, String> {
    let manifest = match world_dir {
        Some(dir) => Manifest::load(dir)?,
        None => None,
    };
    let projection = match manifest {
        Some(manifest) => {
            keep_cave_settings(&manifest, args);
            manifest.projection()
        }
        None => Manifest::new(
            args,
            (requested.min().lat() + requested.max().lat()) / 2.0,
            (requested.min().lng() + requested.max().lng()) / 2.0,
        )
        .projection(),
    };
    Ok(snap_bbox_to_chunks(&projection, requested)?.0)
}

/// Why the world's lock is taken. Another run of this executable is named as
/// such, since telling its user to close Minecraft would send them looking
/// for a game that is not running.
fn open_in_minecraft(world_dir: &Path) -> String {
    match other_arnis_process(world_dir) {
        Some(pid) => format!(
            "The One World at {} is in use by another Arnis run (process {pid}). Wait for it to finish, or stop it, and try again.",
            world_dir.display()
        ),
        None => format!(
            "The One World at {} is open in Minecraft. Leave the world (or close the game) and try again.",
            world_dir.display()
        ),
    }
}

/// Written next to the lock by the Arnis run that holds it, so a refused run
/// can tell another Arnis job from Minecraft. Never removed: a holder that is
/// gone, or a pid now used by another program, simply reads as Minecraft.
const OWNER_FILE: &str = "arnis_one_world/owner.pid";

fn record_owner(world_dir: &Path) {
    let path = world_dir.join(OWNER_FILE);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, std::process::id().to_string());
}

/// The live Arnis process recorded as the lock holder, unless it is this one.
fn other_arnis_process(world_dir: &Path) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(world_dir.join(OWNER_FILE))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if pid == std::process::id() {
        return None;
    }
    let name = std::env::current_exe().ok()?.file_name()?.to_owned();
    let mut sys = sysinfo::System::new();
    let target = sysinfo::Pid::from_u32(pid);
    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[target]), true);
    sys.process(target)
        .filter(|p| p.name().eq_ignore_ascii_case(&name))
        .map(|_| pid)
}

/// Opens or creates the One World at `world_dir`, locks it, and points `args`
/// at the world's frame.
pub fn prepare(world_dir: &Path, requested: &LLBBox, args: &mut Args) -> Result<Session, String> {
    if args.bedrock || args.luanti {
        return Err("One World is available for Java Edition worlds only.".to_string());
    }
    if !args.body.is_earth() {
        return Err("One World is available for Earth only.".to_string());
    }
    if args.rotation.abs() > f64::EPSILON {
        return Err(
            "One World keeps the world aligned to real-world coordinates, so rotation must be 0."
                .to_string(),
        );
    }
    if requested.min().lat() < -MAX_ABS_LAT || requested.max().lat() > MAX_ABS_LAT {
        return Err(format!(
            "One World covers latitudes up to {MAX_ABS_LAT} degrees north and south."
        ));
    }

    let fresh_dir = !world_dir.exists();
    if !fresh_dir && Manifest::load(world_dir)?.is_none() {
        let empty = std::fs::read_dir(world_dir)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false);
        if !empty {
            return Err(format!(
                "{} exists but is not a One World (no {}). Choose another world name or delete the folder.",
                world_dir.display(),
                MANIFEST_FILE
            ));
        }
    }
    if world_is_locked(world_dir) {
        return Err(open_in_minecraft(world_dir));
    }
    std::fs::create_dir_all(world_dir)
        .map_err(|e| format!("Failed to create {}: {e}", world_dir.display()))?;
    let lock = SessionLock::acquire(world_dir).map_err(|_| open_in_minecraft(world_dir))?;
    let mut owned = false;
    let resolved = resolve(world_dir, requested, args, lock, &mut owned);
    if resolved.is_err() && fresh_dir {
        // Everything in it is ours once the check under the lock passed.
        // Before that only an empty folder is removed.
        let _ = if owned {
            std::fs::remove_dir_all(world_dir)
        } else {
            std::fs::remove_dir(world_dir)
        };
    }
    if resolved.is_ok() {
        record_owner(world_dir);
    }
    resolved
}

/// The frame `requested` lands in: the world's own, or for a world that does
/// not exist yet the one `prepare` would create. Reads only; takes no lock.
pub fn frame_for(
    world_dir: &Path,
    requested: &LLBBox,
    args: &Args,
) -> Result<WebMercatorProjection, String> {
    Ok(match Manifest::load(world_dir)? {
        Some(manifest) => manifest.projection(),
        None => {
            let (lat, lon) = new_origin(requested, args);
            new_frame(args, lat, lon)
        }
    })
}

/// The frame of a world created with block (0, 0) at `lat, lon`.
pub fn new_frame(args: &Args, lat: f64, lon: f64) -> WebMercatorProjection {
    Manifest::new(args, lat, lon).projection()
}

/// Block (0, 0) of a world created for `requested`: `--origin`, or the
/// request's centre.
fn new_origin(requested: &LLBBox, args: &Args) -> (f64, f64) {
    args.origin.unwrap_or((
        (requested.min().lat() + requested.max().lat()) / 2.0,
        (requested.min().lng() + requested.max().lng()) / 2.0,
    ))
}

/// The part of `prepare` that runs under the world's lock.
/// `owned` is set once the folder is known to hold nothing but this run's files.
fn resolve(
    world_dir: &Path,
    requested: &LLBBox,
    args: &mut Args,
    lock: SessionLock,
    owned: &mut bool,
) -> Result<Session, String> {
    let (mut manifest, created) = match Manifest::load(world_dir)? {
        Some(manifest) => {
            if !world_dir.join("level.dat").is_file() {
                return Err(format!(
                    "{} has a One World manifest but no level.dat; the world seems damaged.",
                    world_dir.display()
                ));
            }
            let errors = compatibility_errors(&manifest, args);
            if !errors.is_empty() {
                return Err(format!(
                    "This area cannot be added to the One World at {}: {}. Change the setting, or use another world name to start a new One World.",
                    world_dir.display(),
                    errors.join("; ")
                ));
            }
            if manifest.disable_height_limit != args.disable_height_limit {
                println!(
                    "Note: One World keeps the build height it was created with ({}).",
                    if manifest.disable_height_limit {
                        "extended"
                    } else {
                        "vanilla"
                    }
                );
                args.disable_height_limit = manifest.disable_height_limit;
            }
            if manifest.height_multiplier != args.height_multiplier {
                println!(
                    "Note: One World keeps the terrain height multiplier it was created with ({}x).",
                    manifest.height_multiplier
                );
                args.height_multiplier = manifest.height_multiplier;
            }
            if let Some((lat, lon)) = args.origin {
                if (stable(lat, 7), stable(lon, 7)) != (manifest.origin_lat, manifest.origin_lon) {
                    println!(
                        "Note: --origin {lat},{lon} is ignored; the One World keeps the origin it was created with ({},{}).",
                        manifest.origin_lat, manifest.origin_lon
                    );
                }
            }
            (manifest, false)
        }
        None => {
            // Checked again under the lock: the folder may have filled up since.
            let foreign = std::fs::read_dir(world_dir)
                .map_err(|e| format!("Failed to read {}: {e}", world_dir.display()))?
                .filter_map(Result::ok)
                .any(|entry| entry.file_name() != "session.lock");
            if foreign {
                return Err(format!(
                    "{} exists but is not a One World (no {}). Choose another world name or delete the folder.",
                    world_dir.display(),
                    MANIFEST_FILE
                ));
            }
            *owned = true;
            // Room for any place on Earth, so the first area does not limit later ones.
            if !args.disable_height_limit {
                println!(
                    "Note: One World uses the extended build height, so any place on Earth fits."
                );
                args.disable_height_limit = true;
            }
            let (origin_lat, origin_lon) = new_origin(requested, args);
            (Manifest::new(args, origin_lat, origin_lon), true)
        }
    };

    args.scale = manifest.scale;
    let (xzbbox, llbbox) = snap_bbox_to_chunks(&manifest.projection(), requested)?;

    if created {
        let name = world_dir
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(DEFAULT_WORLD_NAME)
            .to_string();
        crate::world_utils::write_world_skeleton(world_dir, &name, false)?;
        manifest.save(world_dir)?;
    }

    if manifest.version < 2 {
        let removed = crate::world_editor::java::drop_misplaced_chunks(world_dir).map_err(|e| {
            format!(
                "Failed to repair the One World at {}: {e}",
                world_dir.display()
            )
        })?;
        println!(
            "One World: repaired a world from an earlier build ({removed} stray chunks dropped)."
        );
        manifest.version = 2;
        manifest.save(world_dir)?;
    }

    if manifest.aws_only_elevation != args.aws_only_elevation {
        println!(
            "Note: One World keeps the elevation source it was created with ({}).",
            if manifest.aws_only_elevation {
                "legacy AWS terrain"
            } else {
                "high-resolution terrain"
            }
        );
        args.aws_only_elevation = manifest.aws_only_elevation;
    }
    keep_cave_settings(&manifest, args);

    let replaced = if created {
        0
    } else {
        existing_chunks(world_dir, &xzbbox)
    };
    let extending = !manifest.areas.is_empty();
    let area_id = manifest.next_area_id;

    args.one_world_run = Some(RunContext {
        world_dir: world_dir.to_path_buf(),
        origin_lat: manifest.origin_lat,
        origin_lon: manifest.origin_lon,
        extending,
        elevation: manifest.elevation,
        elevation_zoom: manifest.elevation_zoom,
        replaced_chunks: replaced,
        area_id,
        unit: None,
    });
    point_args_at_world(args, llbbox);

    println!(
        "One World: {} {} at {}",
        if created {
            "created"
        } else if extending {
            "extending"
        } else {
            "using"
        },
        world_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(DEFAULT_WORLD_NAME),
        world_dir.display()
    );
    println!(
        "  area #{area_id}: blocks x {}..={} z {}..={} ({} x {} chunks){}",
        xzbbox.min_x(),
        xzbbox.max_x(),
        xzbbox.min_z(),
        xzbbox.max_z(),
        (xzbbox.max_x() - xzbbox.min_x() + 1) / 16,
        (xzbbox.max_z() - xzbbox.min_z() + 1) / 16,
        if replaced > 0 {
            format!(", {replaced} existing chunks will be replaced")
        } else {
            String::new()
        }
    );
    // Ground scale changes with latitude; far from the origin a request
    // becomes a much larger world than its size on the map suggests.
    let centre_lat = (llbbox.min().lat() + llbbox.max().lat()) / 2.0;
    let density = manifest.origin_lat.to_radians().cos() / centre_lat.to_radians().cos();
    if !(0.8..=1.25).contains(&density) {
        println!(
            "Note: this far from the world's origin one metre is {:.2} blocks instead of {:.2}.",
            args.scale * density,
            args.scale
        );
    }

    Ok(Session {
        created,
        llbbox,
        lock,
    })
}

/// Points `args` at a One World area: its frame, its bbox, and the options
/// One World turns off.
fn point_args_at_world(args: &mut Args, llbbox: LLBBox) {
    args.projection = crate::projection::ProjectionKind::WebMercator;
    args.bbox = Some(llbbox);
    // Voxy rebuilds the LOD database from the regions of one run, and both
    // facade sources replace the world's resource pack on every run.
    if args.voxy_lod {
        println!("Note: the Voxy LOD cache is off in One World mode.");
        args.voxy_lod = false;
    }
    if args.mapillary_facade_mode.places_displays() && args.mapillary_facades_wanted() {
        println!("Note: One World builds Mapillary facades as blocks; photo panels are off.");
        args.mapillary_facade_mode = crate::args::FacadeMode::Blocks;
    }
    if args.building_facades {
        println!("Note: the preset building facades are off in One World mode.");
        args.building_facades = false;
    }
    args.map_preview = true;
}

/// Opens one piece of a coordinator's job (`--one-world-unit <lease>`). The
/// coordinator holds the world's lock for the whole job, so this takes none:
/// it runs only while that lock is held and the coordinator file still
/// carries the lease's nonce, and it writes nothing that belongs to the world
/// as a whole (no manifest, level.dat or map counter).
pub fn prepare_unit(
    world_dir: &Path,
    requested: &LLBBox,
    args: &mut Args,
    lease_path: &Path,
) -> Result<LLBBox, String> {
    let lease: UnitLease = std::fs::read_to_string(lease_path)
        .map_err(|e| format!("Failed to read {}: {e}", lease_path.display()))
        .and_then(|t| serde_json::from_str(&t).map_err(|e| format!("Bad unit lease: {e}")))?;
    let owner = std::fs::read_to_string(world_dir.join(COORDINATOR_FILE)).unwrap_or_default();
    if !world_is_locked(world_dir) || owner.trim() != lease.nonce {
        return Err(format!(
            "{} is no longer held by the job this piece belongs to.",
            world_dir.display()
        ));
    }
    let manifest = Manifest::load(world_dir)?
        .ok_or_else(|| format!("{} is not a One World.", world_dir.display()))?;
    let errors = compatibility_errors(&manifest, args);
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    args.scale = manifest.scale;
    args.disable_height_limit = manifest.disable_height_limit;
    args.height_multiplier = manifest.height_multiplier;
    args.aws_only_elevation = manifest.aws_only_elevation;
    keep_cave_settings(&manifest, args);
    let (xzbbox, _) = snap_bbox_to_chunks(&manifest.projection(), requested)?;
    let rect = xzbbox.to_array();
    if rect != lease.rect {
        return Err(format!(
            "piece {} snaps to {rect:?}, not to its planned {:?}",
            lease.piece, lease.rect
        ));
    }
    println!(
        "One World: piece {} of area #{}: blocks x {}..={} z {}..={}",
        lease.piece, lease.area_id, rect[0], rect[2], rect[1], rect[3]
    );
    let replaced_chunks = existing_chunks(world_dir, &xzbbox);
    let [x0, z0, x1, z1] = lease.build;
    let build = XZBBox::rect_from_min_max(x0, z0, x1, z1)?;
    let llbbox = crate::projection::llbbox_for_rect(&manifest.projection(), &build)?;
    let (snapped, _) = snap_bbox_to_chunks(&manifest.projection(), &llbbox)?;
    let built = snapped.to_array();
    if built != lease.build {
        return Err(format!(
            "piece {} builds {built:?}, not its planned {:?}",
            lease.piece, lease.build
        ));
    }
    args.one_world_run = Some(RunContext {
        world_dir: world_dir.to_path_buf(),
        origin_lat: manifest.origin_lat,
        origin_lon: manifest.origin_lon,
        // The world-wide extras of a first area are the coordinator's.
        extending: true,
        elevation: manifest.elevation,
        elevation_zoom: manifest.elevation_zoom,
        replaced_chunks,
        area_id: lease.area_id,
        unit: Some(lease),
    });
    point_args_at_world(args, llbbox);
    Ok(llbbox)
}

/// Records a finished area and drops the ones it fully covers.
pub fn record_area(
    run: &RunContext,
    llbbox: &LLBBox,
    xzbbox: &XZBBox,
    elevation: Option<ElevationAffine>,
    preview: Option<&Path>,
) -> Result<(), String> {
    let mut manifest = Manifest::load(&run.world_dir)?.ok_or_else(|| {
        format!(
            "{} vanished during generation",
            Manifest::path_in(&run.world_dir).display()
        )
    })?;
    if manifest.elevation.is_none() {
        manifest.elevation = elevation;
    }
    let preview_rel = preview.and_then(|p| {
        p.strip_prefix(&run.world_dir)
            .ok()
            .map(|r| r.to_string_lossy().replace('\\', "/"))
    });
    let (covered, kept): (Vec<GeneratedArea>, Vec<GeneratedArea>) =
        std::mem::take(&mut manifest.areas)
            .into_iter()
            .partition(|a| a.is_inside(xzbbox));
    manifest.areas = kept;
    manifest.areas.push(GeneratedArea {
        id: run.area_id,
        generated_at: unix_now(),
        arnis_version: env!("CARGO_PKG_VERSION").to_string(),
        min_x: xzbbox.min_x(),
        min_z: xzbbox.min_z(),
        max_x: xzbbox.max_x(),
        max_z: xzbbox.max_z(),
        min_lat: llbbox.min().lat(),
        min_lon: llbbox.min().lng(),
        max_lat: llbbox.max().lat(),
        max_lon: llbbox.max().lng(),
        preview: preview_rel.clone(),
    });
    manifest.next_area_id = manifest.next_area_id.max(run.area_id.saturating_add(1));
    manifest.save(&run.world_dir)?;
    for old in covered {
        if let Some(p) = old.preview.filter(|p| Some(p) != preview_rel.as_ref()) {
            if let Some(path) = safe_preview_path(&run.world_dir, &p) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    Ok(())
}

/// `--world-border`: the border around `rect`, the area just built, or for a
/// One World around every area it holds. A failure is a warning: the world
/// itself is written.
pub fn apply_world_border(world_dir: &Path, rect: &XZBBox) {
    let all = Manifest::load(world_dir)
        .ok()
        .flatten()
        .and_then(|m| m.extent());
    let rect = all.as_ref().unwrap_or(rect);
    match crate::world_utils::set_world_border(world_dir, rect) {
        Ok(()) => println!(
            "World border set around {},{} to {},{}.",
            rect.min_x(),
            rect.min_z(),
            rect.max_x(),
            rect.max_z()
        ),
        Err(e) => eprintln!("Warning: Failed to set the world border: {e}"),
    }
}

/// Stores the elevation mapping as soon as the first terrain area has one,
/// so a run that fails later cannot leave chunks behind on another mapping.
pub fn remember_elevation(run: &RunContext, elevation: ElevationAffine) -> Result<(), String> {
    if run.elevation.is_some() {
        return Ok(());
    }
    let Some(mut manifest) = Manifest::load(&run.world_dir)? else {
        return Ok(());
    };
    if manifest.elevation.is_none() {
        manifest.elevation = Some(elevation);
        manifest.save(&run.world_dir)?;
    }
    Ok(())
}

/// Manifests can come with downloaded worlds, so their preview paths are
/// only followed inside the preview folder.
pub fn safe_preview_path(world_dir: &Path, rel: &str) -> Option<PathBuf> {
    let name = rel.strip_prefix(PREVIEW_DIR)?.strip_prefix('/')?;
    let valid = !name.is_empty()
        && name.ends_with(".png")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !name.contains("..");
    valid.then(|| world_dir.join(PREVIEW_DIR).join(name))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn args_for(bbox: &str, extra: &[&str]) -> Args {
        let mut cmd = vec!["arnis", "--output-dir", ".", "--bbox", bbox];
        cmd.extend_from_slice(extra);
        Args::parse_from(cmd)
    }

    const MUNICH: &str = "48.130,11.560,48.145,11.590";

    fn rect(args: &Args, s: &Session) -> XZBBox {
        crate::projection::ProjectionSpec::from_args(args)
            .transformer(&s.llbbox)
            .unwrap()
            .1
    }

    fn record(args: &Args, s: &Session, elevation: Option<ElevationAffine>) {
        let run = args.one_world_run.clone().unwrap();
        record_area(&run, &s.llbbox, &rect(args, s), elevation, None).unwrap();
    }

    #[test]
    fn a_new_world_gets_a_manifest_and_a_chunk_aligned_area() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("My One World");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &[]);
        let session = prepare(&world, &req, &mut args).unwrap();
        assert!(session.created);
        assert!(world.join("level.dat").is_file());
        assert!(!world.join("region").join("r.0.0.mca").exists());
        let manifest = Manifest::load(&world).unwrap().unwrap();
        assert!(manifest.areas.is_empty());
        let area = rect(&args, &session);
        assert_eq!(area.min_x().rem_euclid(16), 0);
        assert_eq!((area.max_z() + 1).rem_euclid(16), 0);
        let run = args.one_world_run.as_ref().unwrap();
        assert_eq!(run.area_id, 1);
        assert!(!run.extending);
        assert_eq!(
            args.projection,
            crate::projection::ProjectionKind::WebMercator
        );
        assert_eq!(args.bbox.unwrap(), session.llbbox);
    }

    #[test]
    fn recorded_areas_survive_and_a_second_run_extends() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let mut args = args_for(MUNICH, &[]);
        let session = prepare(&world, &LLBBox::from_str(MUNICH).unwrap(), &mut args).unwrap();
        let run = args.one_world_run.clone().unwrap();
        let affine = run.elevation.expect("set when the world is created");
        let other = ElevationAffine {
            min_height_m: 500.0,
            ..affine
        };
        record(&args, &session, Some(other));
        drop(session);

        let east = "48.130,11.585,48.145,11.610";
        let mut args2 = args_for(east, &[]);
        let session2 = prepare(&world, &LLBBox::from_str(east).unwrap(), &mut args2).unwrap();
        assert!(!session2.created);
        let run2 = args2.one_world_run.as_ref().unwrap();
        assert_eq!(run2.area_id, 2);
        assert!(run2.extending);
        assert_eq!(run2.elevation, Some(affine));
        assert_eq!(run2.origin_lat, run.origin_lat);
        assert_eq!(run2.origin_lon, run.origin_lon);
        assert_eq!(run2.replaced_chunks, 0, "nothing was written yet");
    }

    /// A new world pins the elevation zoom of its frame, and every later area and piece
    /// reads it; a world without the field (older, or legacy AWS terrain) leaves the choice
    /// to each area as before.
    #[test]
    fn elevation_zoom_is_pinned_at_creation_only() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &[]);
        drop(prepare(&world, &req, &mut args).unwrap());
        let manifest = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(manifest.elevation_zoom, Some(16));
        assert_eq!(args.one_world_run.unwrap().elevation_zoom, Some(16));

        let east = "48.130,11.585,48.145,11.610";
        let mut args = args_for(east, &[]);
        drop(prepare(&world, &LLBBox::from_str(east).unwrap(), &mut args).unwrap());
        assert_eq!(args.one_world_run.unwrap().elevation_zoom, Some(16));

        let mut legacy = manifest.clone();
        legacy.elevation_zoom = None;
        legacy.save(&world).unwrap();
        let text = std::fs::read_to_string(Manifest::path_in(&world)).unwrap();
        assert!(!text.contains("elevation_zoom"));
        let mut args = args_for(east, &[]);
        drop(prepare(&world, &LLBBox::from_str(east).unwrap(), &mut args).unwrap());
        assert_eq!(args.one_world_run.unwrap().elevation_zoom, None);

        let aws = dir.path().join("aws");
        let mut args = args_for(MUNICH, &["--aws-only-elevation"]);
        drop(prepare(&aws, &req, &mut args).unwrap());
        assert_eq!(Manifest::load(&aws).unwrap().unwrap().elevation_zoom, None);
    }

    /// `--world-border` reads back from level.dat: around the area of a plain
    /// world, around every area of a One World, the rest of the border as
    /// the template has it.
    #[test]
    fn the_world_border_holds_every_area() {
        fn border(world: &Path) -> std::collections::HashMap<String, f64> {
            let level = crate::map_item::read_gzip_nbt(&world.join("level.dat")).unwrap();
            let fastnbt::Value::Compound(root) = level else {
                panic!("root");
            };
            let Some(fastnbt::Value::Compound(data)) = root.get("Data") else {
                panic!("Data");
            };
            data.iter()
                .filter_map(|(k, v)| match v {
                    fastnbt::Value::Double(d) if k.starts_with("Border") => Some((k.clone(), *d)),
                    _ => None,
                })
                .collect()
        }
        let dir = tempfile::tempdir().unwrap();
        // A plain world: its own area, 100 x 40 blocks, the longer side as the size.
        let plain = crate::world_utils::create_new_world(dir.path()).unwrap();
        let plain = Path::new(&plain);
        let template = border(plain);
        let area = XZBBox::rect_from_min_max(-50, 10, 49, 49).unwrap();
        apply_world_border(plain, &area);
        let b = border(plain);
        assert_eq!((b["BorderCenterX"], b["BorderCenterZ"]), (0.0, 30.0));
        assert_eq!((b["BorderSize"], b["BorderSizeLerpTarget"]), (100.0, 100.0));
        for k in [
            "BorderDamagePerBlock",
            "BorderSafeZone",
            "BorderWarningBlocks",
            "BorderWarningTime",
        ] {
            assert_eq!(b[k], template[k], "{k} unchanged");
        }
        // A One World with two areas side by side: the union of both.
        let world = dir.path().join("w");
        let mut args = args_for(MUNICH, &[]);
        let session = prepare(&world, &LLBBox::from_str(MUNICH).unwrap(), &mut args).unwrap();
        let first = rect(&args, &session);
        record(&args, &session, None);
        drop(session);
        let east = "48.130,11.585,48.145,11.610";
        let mut args2 = args_for(east, &[]);
        let session2 = prepare(&world, &LLBBox::from_str(east).unwrap(), &mut args2).unwrap();
        let second = rect(&args2, &session2);
        record(&args2, &session2, None);
        drop(session2);
        assert!(
            second.max_x() > first.max_x(),
            "the second area reaches further east"
        );
        apply_world_border(&world, &second);
        let (x0, z0) = (
            first.min_x().min(second.min_x()),
            first.min_z().min(second.min_z()),
        );
        let (x1, z1) = (
            first.max_x().max(second.max_x()),
            first.max_z().max(second.max_z()),
        );
        let b = border(&world);
        assert_eq!(b["BorderCenterX"], f64::from(x0 + x1 + 1) / 2.0);
        assert_eq!(b["BorderCenterZ"], f64::from(z0 + z1 + 1) / 2.0);
        assert_eq!(b["BorderSize"], f64::from((x1 + 1 - x0).max(z1 + 1 - z0)));
    }

    /// `--origin` pins block (0, 0) of a new world; an existing world keeps its own.
    #[test]
    fn origin_pins_a_new_world_only() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &["--origin", "48.1234567891,11.5432109876"]);
        drop(prepare(&world, &req, &mut args).unwrap());
        let manifest = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(
            (manifest.origin_lat, manifest.origin_lon),
            (48.1234568, 11.543211)
        );
        let frame = manifest.projection();
        assert_eq!(frame.x_for_lon(manifest.origin_lon), 0.0);
        assert_eq!(frame.z_for_lat(manifest.origin_lat), 0.0);

        let mut args = args_for(MUNICH, &["--origin", "-45,170"]);
        drop(prepare(&world, &req, &mut args).unwrap());
        let run = args.one_world_run.unwrap();
        assert_eq!(
            (run.origin_lat, run.origin_lon),
            (manifest.origin_lat, manifest.origin_lon)
        );
    }

    #[test]
    fn an_incompatible_setting_is_refused_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        drop(prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap());

        let err = prepare(&world, &req, &mut args_for(MUNICH, &["--scale", "2"]))
            .err()
            .unwrap();
        assert!(err.contains("world scale 2.00"), "{err}");

        let err = prepare(&world, &req, &mut args_for(MUNICH, &["--mode", "geo-only"]))
            .err()
            .unwrap();
        assert!(err.contains("with terrain"), "{err}");
    }

    #[test]
    fn a_new_world_has_room_for_all_of_earth() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &[]);
        assert!(!args.disable_height_limit);
        drop(prepare(&world, &req, &mut args).unwrap());
        assert!(args.disable_height_limit);
        let manifest = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(manifest.version, MANIFEST_VERSION);
        assert!(manifest.disable_height_limit);
        let e = manifest.elevation.unwrap();
        assert_eq!(e.y_for_metres(-430.0), -2014.0);
        assert_eq!(e.y_for_metres(520.0), -1064.0);
        assert!(e.y_for_metres(8849.0) <= 2016.0);
        // What the first run used is exactly what later runs read back.
        let run = args.one_world_run.as_ref().unwrap();
        assert_eq!(run.elevation, Some(e));
        assert_eq!(
            (run.origin_lat, run.origin_lon),
            (manifest.origin_lat, manifest.origin_lon)
        );

        let flat = dir.path().join("flat");
        let mut args = args_for(MUNICH, &["--mode", "geo-only"]);
        drop(prepare(&flat, &req, &mut args).unwrap());
        assert_eq!(Manifest::load(&flat).unwrap().unwrap().elevation, None);
    }

    #[test]
    fn the_height_multiplier_is_fixed_by_the_first_area() {
        let dir = tempfile::tempdir().unwrap();
        let req = LLBBox::from_str(MUNICH).unwrap();
        let real = dir.path().join("real");
        drop(prepare(&real, &req, &mut args_for(MUNICH, &[])).unwrap());
        let text = std::fs::read_to_string(Manifest::path_in(&real)).unwrap();
        assert!(!text.contains("height_multiplier"), "{text}");

        let world = dir.path().join("w");
        let mut args = args_for(MUNICH, &["--height-multiplier", "2"]);
        drop(prepare(&world, &req, &mut args).unwrap());
        let manifest = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(manifest.height_multiplier, 2.0);
        let e = manifest.elevation.unwrap();
        assert_eq!(e.blocks_per_meter, 2.0);
        assert_eq!(e.y_for_metres(520.0) - e.y_for_metres(-430.0), 1900.0);

        let mut args = args_for(MUNICH, &[]);
        drop(prepare(&world, &req, &mut args).unwrap());
        assert_eq!(args.height_multiplier, 2.0);
        assert_eq!(args.one_world_run.unwrap().elevation, Some(e));
    }

    #[test]
    fn the_cave_seed_and_datum_are_fixed_by_the_first_area() {
        let dir = tempfile::tempdir().unwrap();
        let req = LLBBox::from_str(MUNICH).unwrap();
        let plain = dir.path().join("plain");
        drop(prepare(&plain, &req, &mut args_for(MUNICH, &["--caves"])).unwrap());
        let text = std::fs::read_to_string(Manifest::path_in(&plain)).unwrap();
        assert!(!text.contains("cave_seed"), "{text}");
        assert!(!text.contains("cave_datum_y"), "{text}");

        let world = dir.path().join("w");
        let first = ["--caves", "--cave-seed", "42", "--cave-datum-y", "-1024"];
        drop(prepare(&world, &req, &mut args_for(MUNICH, &first)).unwrap());
        let manifest = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(manifest.cave_seed, Some(42));
        assert_eq!(manifest.cave_datum_y, Some(-1024));
        let later = [
            "--caves",
            "--cave-seed",
            "7",
            "--cave-datum-y",
            "0",
            "--seed",
            "5",
        ];
        for extra in [&["--caves"][..], &later] {
            let mut args = args_for(MUNICH, extra);
            drop(prepare(&world, &req, &mut args).unwrap());
            assert_eq!(args.cave_seed, Some(42));
            assert_eq!(args.cave_datum_y, Some(-1024));
            assert_eq!(args.seed, None);
        }
        assert!(!text.contains("\"seed\""), "{text}");

        let seeded = dir.path().join("s");
        drop(prepare(&seeded, &req, &mut args_for(MUNICH, &["--seed", "9"])).unwrap());
        assert_eq!(Manifest::load(&seeded).unwrap().unwrap().seed, Some(9));
        for extra in [&[][..], &["--seed", "5"]] {
            let mut args = args_for(MUNICH, extra);
            drop(prepare(&seeded, &req, &mut args).unwrap());
            assert_eq!(args.seed, Some(9));
        }
        let mut args = args_for(MUNICH, &later);
        drop(prepare(&plain, &req, &mut args).unwrap());
        assert_eq!((args.cave_seed, args.cave_datum_y), (None, None));
    }

    #[test]
    fn a_preview_uses_the_frame_the_run_builds_in() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let edges = |b: XZBBox| (b.min_x(), b.min_z(), b.max_x(), b.max_z());
        let fresh = preview_rect(None, &req, &mut args_for(MUNICH, &[])).unwrap();
        let mut args = args_for(MUNICH, &["--caves", "--cave-seed", "42"]);
        let session = prepare(&world, &req, &mut args).unwrap();
        assert_eq!(edges(fresh), edges(rect(&args, &session)));
        drop(session);

        let east = "48.130,11.585,48.145,11.610";
        let req = LLBBox::from_str(east).unwrap();
        let mut preview_args = args_for(east, &[]);
        let preview = preview_rect(Some(&world), &req, &mut preview_args).unwrap();
        assert_eq!(preview_args.cave_seed, Some(42));
        let own = preview_rect(None, &req, &mut args_for(east, &[])).unwrap();
        assert_ne!(edges(preview.clone()), edges(own));
        let mut args = args_for(east, &[]);
        let session = prepare(&world, &req, &mut args).unwrap();
        assert_eq!(edges(preview), edges(rect(&args, &session)));
    }

    #[test]
    fn an_older_world_keeps_its_build_height() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        drop(prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap());
        let mut manifest = Manifest::load(&world).unwrap().unwrap();
        manifest.version = 2;
        manifest.disable_height_limit = false;
        manifest.elevation = None;
        manifest.save(&world).unwrap();

        let mut args = args_for(MUNICH, &["--disable-height-limit"]);
        drop(prepare(&world, &req, &mut args).unwrap());
        assert!(!args.disable_height_limit);
        assert_eq!(args.one_world_run.unwrap().elevation, None);
        assert_eq!(Manifest::load(&world).unwrap().unwrap().version, 2);
    }

    #[test]
    fn a_locked_world_is_refused_and_the_session_holds_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let session = prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap();
        let err = prepare(&world, &req, &mut args_for(MUNICH, &[]))
            .err()
            .unwrap();
        assert!(err.contains("open in Minecraft"), "{err}");
        drop(session);

        let held = SessionLock::acquire(&world).unwrap();
        assert!(prepare(&world, &req, &mut args_for(MUNICH, &[])).is_err());
        drop(held);
        assert!(prepare(&world, &req, &mut args_for(MUNICH, &[])).is_ok());
    }

    #[test]
    fn a_refused_first_run_leaves_no_folder_behind() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        // Wider than the world border at scale 4, refused after the lock is taken.
        let huge = "-10.0,-179.0,10.0,179.0";
        let err = prepare(
            &world,
            &LLBBox::from_str(huge).unwrap(),
            &mut args_for(huge, &["--scale", "4"]),
        )
        .err()
        .unwrap();
        assert!(err.contains("world border"), "{err}");
        assert!(!world.exists());
    }

    #[test]
    fn a_foreign_folder_is_not_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        std::fs::create_dir_all(&world).unwrap();
        std::fs::write(world.join("level.dat"), b"x").unwrap();
        let err = prepare(
            &world,
            &LLBBox::from_str(MUNICH).unwrap(),
            &mut args_for(MUNICH, &[]),
        )
        .err()
        .unwrap();
        assert!(err.contains("not a One World"), "{err}");
    }

    #[test]
    fn refused_runs_create_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        assert!(prepare(&world, &req, &mut args_for(MUNICH, &["--rotation", "10"])).is_err());
        assert!(prepare(&world, &req, &mut args_for(MUNICH, &["--bedrock"])).is_err());
        let polar = "86.0,10.0,86.1,10.1";
        assert!(prepare(
            &world,
            &LLBBox::from_str(polar).unwrap(),
            &mut args_for(polar, &[])
        )
        .is_err());
        assert!(!world.exists());
    }

    #[test]
    fn an_area_past_the_world_border_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &["--scale", "4"]);
        let session = prepare(&world, &req, &mut args).unwrap();
        record(&args, &session, None);
        drop(session);
        let far = "-33.88,151.19,-33.85,151.23";
        let err = prepare(
            &world,
            &LLBBox::from_str(far).unwrap(),
            &mut args_for(far, &["--scale", "4"]),
        )
        .err()
        .unwrap();
        assert!(err.contains("world border"), "{err}");
    }

    #[test]
    fn covered_areas_are_dropped_and_an_identical_rerun_replaces_itself() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        let mut args = args_for(MUNICH, &[]);
        let s1 = prepare(&world, &req, &mut args).unwrap();
        record(&args, &s1, None);
        drop(s1);
        let mut again = args_for(MUNICH, &[]);
        let s1b = prepare(&world, &req, &mut again).unwrap();
        record(&again, &s1b, None);
        drop(s1b);
        let m = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(m.areas.len(), 1);
        assert_eq!(m.areas[0].id, 2);

        let big = "48.120,11.550,48.155,11.600";
        let mut args2 = args_for(big, &[]);
        let s2 = prepare(&world, &LLBBox::from_str(big).unwrap(), &mut args2).unwrap();
        record(&args2, &s2, None);
        let m = Manifest::load(&world).unwrap().unwrap();
        assert_eq!(m.areas.len(), 1);
        assert_eq!(m.areas[0].id, 3);
        assert_eq!(m.extent().unwrap().min_x(), rect(&args2, &s2).min_x());
    }

    #[test]
    fn a_damaged_manifest_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        let req = LLBBox::from_str(MUNICH).unwrap();
        drop(prepare(&world, &req, &mut args_for(MUNICH, &[])).unwrap());
        let mut m = Manifest::load(&world).unwrap().unwrap();
        m.scale = 0.0;
        m.save(&world).unwrap();
        let err = Manifest::load(&world).unwrap_err();
        assert!(err.contains("damaged"), "{err}");
    }

    #[test]
    fn preview_paths_stay_inside_the_preview_folder() {
        let w = Path::new("/w");
        assert_eq!(
            safe_preview_path(w, "arnis_one_world/previews/area-3.png"),
            Some(w.join(PREVIEW_DIR).join("area-3.png"))
        );
        assert_eq!(
            safe_preview_path(w, "arnis_one_world/previews/../level.dat"),
            None
        );
        assert_eq!(safe_preview_path(w, "../../etc/passwd"), None);
        assert_eq!(
            safe_preview_path(w, "arnis_one_world/previews/a/b.png"),
            None
        );
        assert_eq!(safe_preview_path(w, "arnis_one_world/previews/x.txt"), None);
    }

    #[test]
    fn a_piece_builds_its_halo_but_never_past_the_selection() {
        let selection = XZBBox::rect_from_min_max(-320, -288, 1023, 287).unwrap();
        let rect = XZBBox::rect_from_min_max(0, -288, 511, 287).unwrap();
        let b = piece_build_rect(&rect, &selection);
        let h = PIECE_HALO_BLOCKS;
        assert_eq!(
            (b.min_x(), b.min_z(), b.max_x(), b.max_z()),
            (-h, -288, 511 + h, 287)
        );
    }

    #[test]
    fn existing_chunks_are_read_from_the_region_headers() {
        let dir = tempfile::tempdir().unwrap();
        let region = dir.path().join("region");
        std::fs::create_dir_all(&region).unwrap();
        let mut header = vec![0u8; 8192];
        // Chunks (0, 0) and (1, 0) of region (0, 0), and (31, 31) of region (-1, -1).
        header[0..4].copy_from_slice(&[0, 0, 2, 1]);
        header[4..8].copy_from_slice(&[0, 0, 3, 1]);
        std::fs::write(region.join("r.0.0.mca"), &header).unwrap();
        let mut other = vec![0u8; 8192];
        let i = 4 * (31 + 31 * 32);
        other[i..i + 4].copy_from_slice(&[0, 0, 2, 1]);
        std::fs::write(region.join("r.-1.-1.mca"), &other).unwrap();

        let rect = XZBBox::rect_from_min_max(-16, -16, 15, 15).unwrap();
        assert_eq!(existing_chunks(dir.path(), &rect), 2);
        let rect = XZBBox::rect_from_min_max(16, 0, 31, 15).unwrap();
        assert_eq!(existing_chunks(dir.path(), &rect), 1);
        let rect = XZBBox::rect_from_min_max(512, 512, 527, 527).unwrap();
        assert_eq!(existing_chunks(dir.path(), &rect), 0);
    }

    #[test]
    fn a_manifest_without_areas_has_no_extent() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path().join("w");
        drop(
            prepare(
                &world,
                &LLBBox::from_str(MUNICH).unwrap(),
                &mut args_for(MUNICH, &[]),
            )
            .unwrap(),
        );
        assert!(Manifest::load(&world).unwrap().unwrap().extent().is_none());
    }
}
