use crate::args::Args;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use crate::data_processing::{self, GenerationOptions};
use crate::ground::{self, Ground};
use crate::map_preview;
use crate::map_transformation;
use crate::osm_parser;
use crate::overture;
use crate::progress::{self, emit_gui_progress_update};
use crate::retrieve_data;
use crate::telemetry::{self, send_log, LogLevel};
use crate::version_check;
use crate::world_editor::WorldFormat;
use colored::Colorize;
use fastnbt::Value;
use flate2::read::GzDecoder;
use log::LevelFilter;
use rfd::FileDialog;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{env, fs, io::Write};
use tauri_plugin_log::{Builder as LogBuilder, Target, TargetKind};

use crate::world_utils::SessionLock;

/// Removes a freshly created Java world directory. Called whenever generation
/// bails out before producing anything useful, so the user isn't left with a
/// growing pile of empty "Arnis World N" folders.
fn remove_new_java_world(path: &Path) {
    if path.exists() {
        if let Err(e) = fs::remove_dir_all(path) {
            eprintln!("Failed to remove newly created world after failure: {e}");
        }
    }
}

/// RAII guard that removes a newly created Java world on drop unless disarmed.
/// Must be declared *before* any `SessionLock` so the lock's file handle is
/// released first (Windows blocks folder removal otherwise).
struct NewWorldCleanup {
    path: PathBuf,
    armed: bool,
}

impl NewWorldCleanup {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for NewWorldCleanup {
    fn drop(&mut self) {
        if self.armed {
            remove_new_java_world(&self.path);
        }
    }
}

pub fn run_gui() -> Result<(), String> {
    // Configure thread pool with 90% CPU cap to keep system responsive
    crate::floodfill_cache::configure_rayon_thread_pool(0.9);

    // Clean up old cached elevation tiles on startup
    crate::elevation_data::cleanup_old_cached_tiles();

    // Launch the UI
    println!("Launching UI...");

    // Install panic hook for crash reporting
    telemetry::install_panic_hook();

    // Workaround WebKit2GTK issue with NVIDIA drivers and graphics issues
    // Source: https://github.com/tauri-apps/tauri/issues/10702
    #[cfg(target_os = "linux")]
    unsafe {
        // Disable problematic GPU features that cause map loading issues
        env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");

        // Force software rendering for better compatibility.
        // Only set if not already configured by the user, allowing manual override
        // for systems where software rendering causes EGL_BAD_PARAMETER (see #1247).
        if env::var("LIBGL_ALWAYS_SOFTWARE").is_err() {
            env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        }
        if env::var("GALLIUM_DRIVER").is_err() {
            env::set_var("GALLIUM_DRIVER", "softpipe");
        }

        // Note: Removed sandbox disabling for security reasons
        // Note: Removed Qt WebEngine flags as they don't apply to Tauri
    }

    tauri::Builder::default()
        .plugin(
            LogBuilder::default()
                .level(LevelFilter::Info)
                .targets([
                    Target::new(TargetKind::LogDir {
                        file_name: Some("arnis".into()),
                    }),
                    Target::new(TargetKind::Stdout),
                ])
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            gui_create_world,
            gui_get_default_save_path,
            gui_get_default_bedrock_save_path,
            gui_get_default_luanti_save_path,
            gui_set_save_path,
            gui_pick_save_directory,
            gui_pick_loot_table,
            gui_climate_preview,
            gui_pick_osm_file,
            gui_pick_pbf_file,
            gui_save_preset,
            gui_load_preset,
            gui_redraw_one_world_map,
            gui_snap_selection,
            gui_data_plan,
            gui_local_archive_info,
            gui_prepare_plan,
            gui_bake_archive,
            gui_cancel_bake,
            gui_bake_threads,
            gui_storage_info,
            gui_extract_size,
            gui_render_preview,
            gui_tree_pack_status,
            gui_tree_pack_layout,
            gui_start_generation,
            gui_get_version,
            gui_get_update_info,
            gui_get_platform,
            gui_clear_tile_caches,
            gui_get_cache_size,
            gui_get_mapillary_attributions,
            gui_get_world_map_data,
            gui_show_in_folder,
            gui_get_3d_model_attributions,
            gui_get_terrain_preview,
            gui_get_preview_landcover,
            gui_get_preview_buildings,
            gui_get_preview_facades,
            gui_precompute_facades,
            gui_cancel_precompute,
            gui_one_world_info,
            gui_one_world_overlap,
            gui_get_one_world_overlays,
            gui_log,
            gui_set_telemetry_consent
        ])
        .setup(|app| {
            let app_handle = app.handle();
            let main_window = tauri::Manager::get_webview_window(app_handle, "main")
                .ok_or_else(|| std::io::Error::other("Failed to get main window"))?;
            progress::set_main_window(main_window);
            Ok(())
        })
        .run(tauri::generate_context!())
        .map_err(|e| format!("Error while starting the application UI (Tauri): {e}"))
}

/// Detects the default Minecraft Java Edition saves directory for the current OS.
/// Checks standard install paths including Flatpak on Linux.
/// Falls back to Desktop, then current directory.
fn detect_minecraft_saves_directory() -> PathBuf {
    // Try standard Minecraft saves directories per OS
    let mc_saves: Option<PathBuf> = if cfg!(target_os = "windows") {
        env::var("APPDATA")
            .ok()
            .map(|appdata| PathBuf::from(appdata).join(".minecraft").join("saves"))
    } else if cfg!(target_os = "macos") {
        dirs::home_dir().map(|home| {
            home.join("Library/Application Support/minecraft")
                .join("saves")
        })
    } else if cfg!(target_os = "linux") {
        dirs::home_dir().map(|home| {
            let flatpak_path = home.join(".var/app/com.mojang.Minecraft/.minecraft/saves");
            if flatpak_path.exists() {
                flatpak_path
            } else {
                home.join(".minecraft/saves")
            }
        })
    } else {
        None
    };

    if let Some(saves_dir) = mc_saves {
        if saves_dir.exists() {
            return saves_dir;
        }
    }

    // Fallback to Desktop
    if let Some(desktop) = dirs::desktop_dir() {
        if desktop.exists() {
            return desktop;
        }
    }

    // Last resort: current directory
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Returns the default save path (auto-detected on first run).
/// The frontend stores/retrieves this via localStorage and passes it here for validation.
#[tauri::command]
fn gui_get_default_save_path() -> String {
    detect_minecraft_saves_directory().display().to_string()
}

/// Returns the default directory for Bedrock .mcworld files (the Desktop).
#[tauri::command]
fn gui_get_default_bedrock_save_path() -> String {
    crate::world_utils::get_bedrock_output_directory()
        .display()
        .to_string()
}

/// Returns the configured Bedrock output directory, or the default if it is unusable.
fn resolve_bedrock_output_dir(configured: &str) -> PathBuf {
    let trimmed = configured.trim();
    if !trimmed.is_empty() {
        let configured_dir = PathBuf::from(trimmed);
        if configured_dir.is_dir() {
            return configured_dir;
        }
        eprintln!(
            "Warning: Bedrock save path '{trimmed}' is not a directory, using the default instead."
        );
    }
    crate::world_utils::get_bedrock_output_directory()
}

/// Returns the default directory for Luanti/Minetest worlds.
#[tauri::command]
fn gui_get_default_luanti_save_path() -> String {
    crate::world_utils::get_luanti_worlds_directory()
        .display()
        .to_string()
}

/// Returns the configured Luanti worlds directory, or the default if it is unusable.
fn resolve_luanti_output_dir(configured: &str) -> PathBuf {
    let trimmed = configured.trim();
    if !trimmed.is_empty() {
        let configured_dir = PathBuf::from(trimmed);
        if configured_dir.is_dir() {
            return configured_dir;
        }
        eprintln!(
            "Warning: Luanti save path '{trimmed}' is not a directory, using the default instead."
        );
    }
    crate::world_utils::get_luanti_worlds_directory()
}

#[derive(serde::Serialize)]
struct AttributionRow {
    label: String,
    artist: String,
    license: String,
    license_url: Option<String>,
    source_url: String,
}

#[tauri::command]
fn gui_get_3d_model_attributions() -> Vec<AttributionRow> {
    crate::models_3d::wikidata::PERMISSIVE_ATTRIBUTIONS
        .iter()
        .map(|e| AttributionRow {
            label: e.label.clone(),
            artist: e
                .artist
                .clone()
                .unwrap_or_else(|| "Wikimedia contributor".into()),
            license: e.license.clone(),
            license_url: e.license_url.clone(),
            source_url: e.url.clone(),
        })
        .collect()
}

/// Validates and returns a user-provided save path.
/// Returns the path string if valid, or an error message.
#[tauri::command]
fn gui_set_save_path(path: String) -> Result<String, String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err("Path does not exist.".to_string());
    }
    if !p.is_dir() {
        return Err("Path is not a directory.".to_string());
    }
    Ok(path)
}

/// Sink for frontend diagnostics (tile/network failures, uncaught errors).
///
/// The webview console is unreachable in a release build, so anything the UI
/// learns about the user's network never reached the log file users are asked
/// to attach to bug reports. Routing it through `log` puts it in the same
/// LogDir target as the backend's own output.
#[tauri::command]
fn gui_log(level: String, message: String) {
    // The frontend is not a trusted formatter: cap the length so a runaway
    // handler cannot fill the log file, and keep it on one line so the file
    // stays greppable.
    const MAX_LEN: usize = 2000;
    let mut message: String = message.replace(['\n', '\r'], " ");
    if message.chars().count() > MAX_LEN {
        message = message.chars().take(MAX_LEN).collect::<String>() + "...[truncated]";
    }

    match level.as_str() {
        "error" => log::error!(target: "webview", "{message}"),
        "warn" => log::warn!(target: "webview", "{message}"),
        _ => log::info!(target: "webview", "{message}"),
    }
}

/// Trims `area_name` so "<base_name>: <area_name>" stays within 30 characters.
/// `None` when the base name leaves no room. Custom names can start with
/// "Arnis World " and already be that long, so the budget saturates.
fn fit_area_name(base_name: &str, area_name: String) -> Option<String> {
    let max_len = 30usize.saturating_sub(base_name.chars().count() + 2); // 2 for ": "
    if max_len == 0 {
        None
    } else if area_name.chars().count() > max_len {
        Some(area_name.chars().take(max_len).collect())
    } else {
        Some(area_name)
    }
}

/// Mirrors the frontend's consent record into the backend. Called on startup and
/// whenever the user answers or flips it, so crashes before the first generation
/// are covered and a withdrawal takes effect immediately rather than at the next
/// generation.
#[tauri::command]
fn gui_set_telemetry_consent(consent: bool) {
    telemetry::set_telemetry_consent(consent);
}

/// Opens a native folder-picker dialog and returns the chosen path.
#[tauri::command]
fn gui_pick_save_directory(start_path: String) -> Result<String, String> {
    let start = PathBuf::from(&start_path);
    let mut dialog = FileDialog::new();
    if start.is_dir() {
        dialog = dialog.set_directory(&start);
    }
    match dialog.pick_folder() {
        Some(folder) => Ok(folder.display().to_string()),
        None => Ok(start_path),
    }
}

/// Opens a native file picker for a chest loot table (JSON) and returns the
/// chosen path, or `current` when the user cancels.
#[tauri::command]
fn gui_pick_loot_table(current: String) -> Result<String, String> {
    Ok(pick_file(current, "JSON", &["json"]))
}

/// The same for a local OSM file (`--file`).
#[tauri::command]
fn gui_pick_osm_file(current: String) -> Result<String, String> {
    Ok(pick_file(current, "OSM", &["osm", "xml", "json"]))
}

#[tauri::command]
fn gui_pick_pbf_file(current: String) -> Result<String, String> {
    Ok(pick_file(current, "OSM PBF", &["pbf"]))
}

fn pick_file(current: String, kind: &str, extensions: &[&str]) -> String {
    let mut dialog = FileDialog::new().add_filter(kind, extensions);
    if let Some(dir) = Path::new(&current).parent().filter(|d| d.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    dialog
        .pick_file()
        .map_or(current, |file| file.display().to_string())
}

/// Saves an Advanced Features preset where the user picks; false when they
/// cancel.
#[tauri::command]
fn gui_save_preset(contents: String) -> Result<bool, String> {
    let Some(path) = FileDialog::new()
        .add_filter("JSON", &["json"])
        .set_file_name("arnis-preset.json")
        .save_file()
    else {
        return Ok(false);
    };
    fs::write(path, contents)
        .map(|()| true)
        .map_err(|e| e.to_string())
}

/// The preset file the user picks, or none when they cancel.
#[tauri::command]
fn gui_load_preset() -> Result<Option<String>, String> {
    FileDialog::new()
        .add_filter("JSON", &["json"])
        .pick_file()
        .map(|path| fs::read_to_string(path).map_err(|e| e.to_string()))
        .transpose()
}

/// `--climate-map` for the selected area, as a PNG data URL for the window.
#[tauri::command(async)]
fn gui_climate_preview(bbox_text: String) -> Result<String, String> {
    use clap::Parser;
    let prefix = env::temp_dir().join("arnis-climate-preview");
    let args = Args::try_parse_from([
        "arnis".into(),
        format!("--bbox={bbox_text}").into(),
        std::ffi::OsString::from("--climate-map"),
        prefix.clone().into(),
    ])
    .map_err(|e| e.to_string())?;
    crate::climate_field::render(&args)?;
    let png = std::fs::read(format!("{}.png", prefix.display())).map_err(|e| e.to_string())?;
    Ok(format!(
        "data:image/png;base64,{}",
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png)
    ))
}

/// The selection grown to whole cells of the One World it joins, for the
/// map: the bbox to generate, the outline it builds and the cell grid.
#[derive(serde::Serialize)]
struct SelectionSnap {
    /// `min_lat min_lon max_lat max_lon` at full precision.
    bbox: String,
    /// What the run builds: `[min_lat, min_lon, max_lat, max_lon]`.
    outline: [f64; 4],
    /// Cell lines inside it; empty past `MAX_DRAWN_CELLS`.
    lon_lines: Vec<f64>,
    lat_lines: Vec<f64>,
    cells: [i32; 2],
    regions: [i32; 2],
    /// Block (0, 0): the cell junction at the centre of a new world, or the
    /// existing world's origin.
    origin: [f64; 2],
    /// The run must be given `origin` as `--origin`.
    new_world: bool,
    /// Fit Inside found no whole cell on a side and took one.
    fallback: bool,
    /// Folder name of the world the snap is for: the named One World, or a
    /// fresh "Arnis World N" when no name was given.
    world_name: String,
    /// Pieces the job would build at once, as `--one-world-workers` sizes it.
    workers: usize,
    /// East-west and north-south size in kilometres on the ground.
    size_km: [f64; 2],
}

/// Past this many cells the map shows the outline and the count only.
const MAX_DRAWN_CELLS: i32 = 2000;

#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
fn gui_snap_selection(
    bbox_text: String,
    save_path: String,
    world_name: Option<String>,
    scale: f64,
    unit_regions: i32,
    snap_mode: Option<String>,
    square: Option<bool>,
    flags: Option<Vec<String>>,
) -> Result<SelectionSnap, String> {
    use crate::work_units::SnapMode;
    use clap::Parser;
    let scale_flag = format!("--scale={scale}");
    // The run's own flags size the workers; ones that do not parse (the run
    // says why) leave the defaults.
    let args = Args::try_parse_from(
        ["arnis", scale_flag.as_str()]
            .into_iter()
            .chain(flags.iter().flatten().map(String::as_str)),
    )
    .or_else(|_| Args::try_parse_from(["arnis", scale_flag.as_str()]))
    .map_err(|e| e.to_string())?;
    let requested = LLBBox::from_str(&bbox_text)?;
    // No name: the new One World a large selection becomes, named as a new
    // world is.
    let world = match &world_name {
        Some(name) => one_world_dir(&save_path, name),
        None => PathBuf::from(save_path.trim()).join(
            crate::world_utils::generate_unique_default_world_name(Path::new(save_path.trim())),
        ),
    };
    let mode = match snap_mode.as_deref() {
        Some("cover") => SnapMode::Cover,
        _ => SnapMode::FitInside,
    };
    let snap = crate::work_units::snap_to_cells(
        &world,
        &requested,
        &args,
        unit_regions,
        mode,
        square.unwrap_or(false),
    )?;
    let (rect, frame) = (&snap.rect, &snap.frame);
    let outline = crate::projection::llbbox_for_rect(frame, rect)?;
    let cell = 512 * unit_regions;
    let count = |lo: i32, hi: i32, side: i32| (hi + 1 - lo + side - 1) / side;
    let cells = [
        count(rect.min_x(), rect.max_x(), cell),
        count(rect.min_z(), rect.max_z(), cell),
    ];
    let drawn = cells[0].saturating_mul(cells[1]) <= MAX_DRAWN_CELLS;
    // The lattice lines strictly inside the outline.
    let lines = |lo: i32, hi: i32, to: &dyn Fn(f64) -> f64| -> Vec<f64> {
        if !drawn {
            return Vec::new();
        }
        (lo.div_euclid(cell) + 1..=hi.div_euclid(cell))
            .map(|i| i * cell)
            .filter(|&v| v > lo && v <= hi)
            .map(|v| to(f64::from(v)))
            .collect()
    };
    let (lo, hi) = (snap.bbox.min(), snap.bbox.max());
    Ok(SelectionSnap {
        bbox: format!("{} {} {} {}", lo.lat(), lo.lng(), hi.lat(), hi.lng()),
        outline: [
            outline.min().lat(),
            outline.min().lng(),
            outline.max().lat(),
            outline.max().lng(),
        ],
        lon_lines: lines(rect.min_x(), rect.max_x(), &|x| frame.lon_for_x(x)),
        lat_lines: lines(rect.min_z(), rect.max_z(), &|z| frame.lat_for_z(z)),
        cells,
        regions: [
            count(rect.min_x(), rect.max_x(), 512),
            count(rect.min_z(), rect.max_z(), 512),
        ],
        origin: [frame.origin_lat, frame.origin_lon],
        new_world: snap.new_world,
        fallback: snap.fallback,
        world_name: world
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        workers: crate::scale::budget::sizing(
            &args,
            cells[0].max(1).saturating_mul(cells[1].max(1)) as usize,
            (unit_regions * unit_regions).max(1) as u64,
        )
        .workers,
        // A block is 1/scale metres at the frame's origin.
        size_km: [
            f64::from(rect.max_x() + 1 - rect.min_x()) / scale / 1000.0,
            f64::from(rect.max_z() + 1 - rect.min_z()) / scale / 1000.0,
        ],
    })
}

/// The Download Plan panel: what a run of the selection with these settings
/// reads, and how much of it the caches already hold. Reads the disk only.
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
fn gui_data_plan(
    bbox_text: String,
    world_scale: f64,
    terrain_enabled: bool,
    skip_osm_objects: bool,
    canopy_height_enabled: bool,
    overture_enabled: bool,
    aws_only_elevation: bool,
    flags: Vec<String>,
) -> Result<crate::data_plan::DataPlan, String> {
    use clap::Parser;
    let mut args =
        Args::try_parse_from(std::iter::once("arnis").chain(flags.iter().map(String::as_str)))
            .map_err(|e| e.to_string())?;
    crate::args::validate_scale(world_scale)?;
    args.scale = world_scale;
    args.mode = if skip_osm_objects {
        crate::args::GenerationMode::TerrainOnly
    } else if terrain_enabled {
        crate::args::GenerationMode::GeoTerrain
    } else {
        crate::args::GenerationMode::GeoOnly
    };
    args.canopy_height = canopy_height_enabled;
    args.overture = overture_enabled;
    args.aws_only_elevation = aws_only_elevation;
    let bbox = LLBBox::from_str(&bbox_text)?;
    let root = cache_root();
    Ok(crate::data_plan::plan(&root, &args, bbox))
}

/// The Local Archive folder an empty field means, and where arnis-tiles is
/// (`None` when it is not installed).
#[derive(serde::Serialize)]
struct LocalArchiveInfo {
    default_folder: String,
    arnis_tiles: Option<String>,
}

#[tauri::command]
fn gui_local_archive_info(tiles_path: String) -> LocalArchiveInfo {
    LocalArchiveInfo {
        default_folder: crate::arnis_tiles::default_folder(&cache_root())
            .display()
            .to_string(),
        arnis_tiles: crate::arnis_tiles::locate(&tiles_path).map(|p| p.display().to_string()),
    }
}

fn cache_root() -> PathBuf {
    crate::elevation::cache::user_cache_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// The archive folder a Local Archive field names; empty is the default.
fn archive_folder(folder: &str) -> Result<PathBuf, String> {
    let folder = folder.trim();
    if folder.is_empty() {
        return Ok(crate::arnis_tiles::default_folder(&cache_root()));
    }
    crate::overture::pmtiles::local_path(folder)
        .ok_or_else(|| format!("{folder} is not a folder on this computer."))
}

/// arnis-tiles' own index cache and scratch.
fn arnis_tiles_state() -> PathBuf {
    cache_root().join("arnis").join("arnis-tiles")
}

fn arnis_tiles_exe(tiles_path: &str) -> Result<PathBuf, String> {
    crate::arnis_tiles::locate(tiles_path).ok_or_else(|| {
        format!(
            "arnis-tiles not found; get it from {}",
            crate::arnis_tiles::HOME
        )
    })
}

/// One extract of the Prepare Countries list.
#[derive(serde::Serialize)]
struct PrepareRow {
    #[serde(flatten)]
    extract: crate::arnis_tiles::Extract,
    /// The folder already holds its archive.
    baked: bool,
    /// Its archive: on disk once baked, else estimated from the download.
    archive_bytes: u64,
}

#[derive(serde::Serialize)]
struct PreparePlan {
    extracts: Vec<PrepareRow>,
    total_bytes: u64,
    uncovered_points: u64,
    /// The folder the archives go to, and the free space on its disk.
    folder: String,
    free_bytes: Option<u64>,
    /// The most a bake of the extracts not yet baked holds on disk at once.
    peak_bytes: u64,
}

/// `arnis-tiles prepare --dry-run` for a selection, kept per bbox for the
/// session: the first asks the Geofabrik index and HEADs a few extracts.
fn prepare_dry_run(
    bbox_text: &str,
    tiles_path: &str,
) -> Result<crate::arnis_tiles::Prepare, String> {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    static PLANS: Mutex<BTreeMap<String, crate::arnis_tiles::Prepare>> =
        Mutex::new(BTreeMap::new());

    let cached = PLANS
        .lock()
        .map_err(|e| e.to_string())?
        .get(bbox_text)
        .cloned();
    if let Some(p) = cached {
        return Ok(p);
    }
    let bbox = LLBBox::from_str(bbox_text)?;
    let exe = arnis_tiles_exe(tiles_path)?;
    let p = crate::arnis_tiles::dry_run(&exe, &arnis_tiles_state(), &bbox)?;
    PLANS
        .lock()
        .map_err(|e| e.to_string())?
        .insert(bbox_text.to_string(), p.clone());
    Ok(p)
}

/// Prepare Countries: the Geofabrik extracts covering the selection, each
/// marked baked when the folder already holds its archive, with the sizes of
/// what a bake downloads and leaves.
#[tauri::command(async)]
fn gui_prepare_plan(
    bbox_text: String,
    folder: String,
    tiles_path: String,
) -> Result<PreparePlan, String> {
    let bbox = LLBBox::from_str(&bbox_text)?;
    let folder = archive_folder(&folder)?;
    let plan = prepare_dry_run(&bbox_text, &tiles_path)?;
    let archives = crate::osm_tiles::local_coverage(&folder, &bbox)
        .map(|c| c.archives)
        .unwrap_or_default();
    let on_disk = |id: &str| archives.iter().find(|a| a.name == id);
    let todo: Vec<u64> = plan
        .extracts
        .iter()
        .filter(|e| on_disk(&e.id).is_none())
        .map(|e| e.bytes)
        .collect();
    Ok(PreparePlan {
        extracts: plan
            .extracts
            .into_iter()
            .map(|e| {
                let baked = on_disk(&e.id);
                PrepareRow {
                    baked: baked.is_some(),
                    archive_bytes: baked
                        .and_then(|a| a.bytes)
                        .unwrap_or_else(|| crate::data_plan::archive_bytes(e.bytes)),
                    extract: e,
                }
            })
            .collect(),
        total_bytes: plan.total_bytes,
        uncovered_points: plan.uncovered_points,
        free_bytes: crate::data_plan::free_bytes(&folder),
        folder: folder.display().to_string(),
        peak_bytes: crate::data_plan::archive_peak_bytes(&todo),
    })
}

/// The threads a bake gets and what that is of the machine, for the panel.
#[derive(serde::Serialize)]
struct BakeThreads {
    threads: usize,
    cpu_pct: u32,
    cores: usize,
    /// Downloads a Region Download prewarm keeps in flight at once.
    downloads: u32,
}

/// [`BakeThreads`] for the Extra Features `flags`, as a bake would use them.
#[tauri::command]
fn gui_bake_threads(flags: Vec<String>) -> Result<BakeThreads, String> {
    let args = meld_args(&flags, false)?;
    let threads = crate::scale::budget::bake_threads(&args);
    let cores = crate::transfer::cores();
    Ok(BakeThreads {
        threads,
        cpu_pct: crate::transfer::cpu_pct(threads, cores),
        cores,
        downloads: args
            .process
            .max_downloads
            .unwrap_or(crate::net::MAX_CONCURRENT_REQUESTS as u32),
    })
}

/// Where the OSM sources keep their downloads and bakes, with sizes. Walks
/// the folders, so off the webview thread.
#[tauri::command(async)]
fn gui_storage_info(folder: String) -> Result<Vec<crate::data_plan::Location>, String> {
    use crate::elevation::cache::dir_size_bytes;
    let archive = archive_folder(&folder)?;
    let root = cache_root();
    // What Clear Cache counts, plus the OSM sources' own, which it keeps:
    // Region Download extracts and bakes, and arnis-tiles' index.
    let arnis = root.join("arnis");
    let all = cache_size_bytes()
        + dir_size_bytes(&arnis.join("osm-pbf"))
        + dir_size_bytes(&arnis.join("arnis-tiles"));
    Ok(crate::data_plan::locations(&root, &archive, all))
}

/// The download size of a Region Download extract, asked once with a HEAD
/// request and kept for the Download Plan.
#[tauri::command(async)]
fn gui_extract_size(url: String) -> Result<u64, String> {
    crate::osm_pbf::remote_size(&url)
}

/// Set by Stop: ends a country bake, or a Region Download / offline download.
static BAKE_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Download & Bake: `arnis-tiles prepare` for the selection into the folder,
/// on the threads a generation worker gets (`flags`, as for a generation),
/// with its progress on the main bar and the panel's. `Ok(false)` when stopped.
#[tauri::command(async)]
fn gui_bake_archive(
    bbox_text: String,
    folder: String,
    tiles_path: String,
    flags: Vec<String>,
) -> Result<bool, String> {
    let args = meld_args(&flags, false)?;
    let threads = crate::scale::budget::bake_threads(&args);
    let bbox = LLBBox::from_str(&bbox_text)?;
    let folder = archive_folder(&folder)?;
    let exe = arnis_tiles_exe(&tiles_path)?;
    let _slot = BusySlot::acquire(BUSY_BAKE)?;
    let cancel = &BAKE_CANCEL;
    cancel.store(false, std::sync::atomic::Ordering::Release);
    progress::reset_progress_floor();
    progress::emit_gui_progress_update(0.0, "Choosing the extracts to bake...");
    let extracts = prepare_dry_run(&bbox_text, &tiles_path)
        .map(|p| p.extracts)
        .unwrap_or_default();
    let result = crate::arnis_tiles::bake(
        &exe,
        &arnis_tiles_state(),
        &folder,
        &bbox,
        &extracts,
        threads,
        cancel,
        &mut |pct, msg, t| progress::emit_gui_transfer(pct, msg, t),
    );
    match &result {
        Ok(true) => progress::emit_gui_progress_update(100.0, "Done! The local archive is ready."),
        Ok(false) => progress::emit_gui_progress_update(
            progress::MESSAGE_ONLY,
            "Bake stopped. Finished countries are kept; nothing half written was left.",
        ),
        Err(e) => progress::emit_gui_error(e),
    }
    result
}

/// Stops a running bake or download. Harmless when nothing runs.
#[tauri::command]
fn gui_cancel_bake() {
    BAKE_CANCEL.store(true, std::sync::atomic::Ordering::Release);
}

/// A live option preview card for one settings group, as a PNG data URL:
/// a tiny sample area built by this executable with the group's `flags`.
/// Cached on disk per group, flags and version. Offline, a sample the caches
/// cannot serve fails with `needs-data`.
#[tauri::command(async)]
fn gui_render_preview(group: String, flags: Vec<String>, offline: bool) -> Result<String, String> {
    use crate::option_preview::{render, Failure};
    let root = cache_root();
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    match render(&root, &exe, &group, &flags, offline) {
        Ok(png) => Ok(format!(
            "data:image/png;base64,{}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, png)
        )),
        Err(Failure::NeedsData) => Err("needs-data".to_string()),
        Err(Failure::Other(e)) => Err(e),
    }
}

/// The Tree Pack Folder an empty field means: `tree-packs` next to Arnis.
fn tree_pack_folder(folder: &str) -> PathBuf {
    match folder.trim() {
        "" => crate::trees::pack_dir::default_folder(),
        typed => PathBuf::from(typed),
    }
}

/// What a Tree Pack Folder holds, for its status line.
#[derive(serde::Serialize)]
struct TreePackStatus {
    folder: String,
    default_folder: String,
    exists: bool,
    found: usize,
    skipped: usize,
}

/// Scans the Tree Pack Folder (empty: the default) as a run would.
#[tauri::command(async)]
fn gui_tree_pack_status(folder: String) -> TreePackStatus {
    use crate::trees::pack_dir::{default_folder, PackDir, TreePackMode};
    let path = tree_pack_folder(&folder);
    let exists = path.is_dir();
    let scan = exists.then(|| PackDir::scan(&path, TreePackMode::Add));
    TreePackStatus {
        folder: path.display().to_string(),
        default_folder: default_folder().display().to_string(),
        exists,
        found: scan.as_ref().map_or(0, PackDir::found),
        skipped: scan.map_or(0, |s| s.skipped.len()),
    }
}

/// Create Folder Structure (`--init-tree-pack-dir`) or, with `export`, Export
/// Built-in Trees (`--export-tree-packs`) into the Tree Pack Folder.
#[tauri::command(async)]
fn gui_tree_pack_layout(folder: String, export: bool) -> Result<usize, String> {
    let path = tree_pack_folder(&folder);
    if export {
        crate::trees::pack_dir::export(&path)
    } else {
        crate::trees::pack_dir::init(&path)
    }
}

/// `--map-item-only`: redraws a One World's map item over every area. Holds
/// the generation slot, so it never runs beside a build of the same world.
#[tauri::command(async)]
fn gui_redraw_one_world_map(save_path: String, world_name: String) -> Result<i32, String> {
    let _slot = BusySlot::acquire(BUSY_GENERATION)?;
    crate::map_item::redraw_one_world_map(&one_world_dir(&save_path, &world_name))
}

/// Creates a new Java Edition world in the given base save directory.
/// Called when the user clicks "Create World".
///
/// `world_name` is `Some` only when the user has enabled the custom world
/// name setting and typed a name; it is sanitized and de-duplicated by
/// [`crate::world_utils::create_new_world_with_name`], which falls back to
/// the default "Arnis World N" scheme when it is `None` or unusable.
// `(async)` rather than a plain command: a bare `#[tauri::command]` runs on the
// main thread, and this one copies a world template onto disk while the user is
// looking at a button that has just gone grey.
#[tauri::command(async)]
fn gui_create_world(save_path: String, world_name: Option<String>) -> Result<String, i32> {
    let trimmed = save_path.trim();
    if trimmed.is_empty() {
        return Err(3);
    }
    let base = PathBuf::from(trimmed);
    if !base.is_dir() {
        return Err(3); // Error code 3: Failed to create new world
    }
    create_new_world(&base, world_name.as_deref()).map_err(|_| 3)
}

fn create_new_world(base_path: &Path, custom_name: Option<&str>) -> Result<String, String> {
    crate::world_utils::create_new_world_with_name(base_path, custom_name)
}

/// Adds localized area name to the world name in level.dat
fn add_localized_world_name(
    world_path: PathBuf,
    bbox: &LLBBox,
    body: crate::celestial::CelestialBody,
) -> PathBuf {
    // Only proceed if the path exists
    if !world_path.exists() {
        return world_path;
    }

    // Check the level.dat file first to get the current name
    let level_path = world_path.join("level.dat");

    if !level_path.exists() {
        return world_path;
    }

    // Try to read the current world name from level.dat
    let Ok(level_data) = std::fs::read(&level_path) else {
        return world_path;
    };

    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    if decoder.read_to_end(&mut decompressed_data).is_err() {
        return world_path;
    }

    let Ok(Value::Compound(ref root)) = fastnbt::from_bytes::<Value>(&decompressed_data) else {
        return world_path;
    };

    let Some(Value::Compound(ref data)) = root.get("Data") else {
        return world_path;
    };

    let Some(Value::String(current_name)) = data.get("LevelName") else {
        return world_path;
    };

    // Only modify if it's an Arnis world and doesn't already have an area name
    if !current_name.starts_with("Arnis World ") || current_name.contains(": ") {
        return world_path;
    }

    // Calculate center coordinates of bbox
    let center_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
    let center_lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;

    // Nominatim would reverse-geocode lunar coordinates into a terrestrial place.
    let area_name = if !body.is_earth() {
        body.display_name().to_string()
    } else {
        match retrieve_data::fetch_area_name(center_lat, center_lon) {
            Ok(Some(name)) => name,
            _ => return world_path, // Keep original name if no area name found
        }
    };

    let base_name = current_name.clone();
    let Some(truncated_area_name) = fit_area_name(&base_name, area_name) else {
        return world_path;
    };

    let new_name = format!("{base_name}: {truncated_area_name}");
    let mut write_succeeded = false;

    // Update the level.dat file with the new name
    if let Ok(level_data) = std::fs::read(&level_path) {
        let mut decoder = GzDecoder::new(level_data.as_slice());
        let mut decompressed_data = Vec::new();
        if decoder.read_to_end(&mut decompressed_data).is_ok() {
            if let Ok(mut nbt_data) = fastnbt::from_bytes::<Value>(&decompressed_data) {
                // Update the level name in NBT data
                if let Value::Compound(ref mut root) = nbt_data {
                    if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
                        data.insert("LevelName".to_string(), Value::String(new_name.clone()));

                        // Save the updated NBT data
                        if let Ok(serialized_data) = fastnbt::to_bytes(&nbt_data) {
                            let mut encoder = flate2::write::GzEncoder::new(
                                Vec::new(),
                                flate2::Compression::default(),
                            );
                            if encoder.write_all(&serialized_data).is_ok() {
                                if let Ok(compressed_data) = encoder.finish() {
                                    match std::fs::write(&level_path, compressed_data) {
                                        Ok(_) => write_succeeded = true,
                                        Err(e) => {
                                            eprintln!(
                                                "Failed to update level.dat with area name: {e}"
                                            );
                                            #[cfg(feature = "gui")]
                                            send_log(
                                                LogLevel::Warning,
                                                "Failed to update level.dat with area name",
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if write_succeeded {
        progress::emit_world_name_update(&new_name);
    }

    // Return the original path since we didn't change the directory name
    world_path
}

/// Calculates the default spawn point at X=1, Z=1 relative to the world origin.
/// This is used when no spawn point is explicitly selected by the user.
fn calculate_default_spawn(xzbbox: &XZBBox) -> (i32, i32) {
    (xzbbox.min_x() + 1, xzbbox.min_z() + 1)
}

/// Sets the player spawn point in level.dat using Minecraft XZ coordinates.
/// The Y coordinate is set to a temporary value (150) and will be updated
/// after terrain generation by `update_player_spawn_y_after_generation`.
fn set_player_spawn_in_level_dat(
    world_path: &str,
    spawn_x: i32,
    spawn_z: i32,
) -> Result<(), String> {
    // Default y spawn position since terrain elevation cannot be determined yet
    let y = 150.0;

    // Read and update the level.dat file
    let level_path = PathBuf::from(world_path).join("level.dat");
    if !level_path.exists() {
        return Err(format!("Level.dat not found at {level_path:?}"));
    }

    // Read the level.dat file
    let level_data = match std::fs::read(&level_path) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to read level.dat: {e}")),
    };

    // Decompress and parse the NBT data
    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    if let Err(e) = decoder.read_to_end(&mut decompressed_data) {
        return Err(format!("Failed to decompress level.dat: {e}"));
    }

    let mut nbt_data = match fastnbt::from_bytes::<Value>(&decompressed_data) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to parse level.dat NBT data: {e}")),
    };

    // Update player position and world spawn point
    if let Value::Compound(ref mut root) = nbt_data {
        if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
            // Set world spawn point
            data.insert("SpawnX".to_string(), Value::Int(spawn_x));
            data.insert("SpawnY".to_string(), Value::Int(y as i32));
            data.insert("SpawnZ".to_string(), Value::Int(spawn_z));

            // Update player position if Player compound exists
            if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
                if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
                    // Safely update position values with bounds checking
                    if pos.len() >= 3 {
                        if let Some(Value::Double(ref mut pos_x)) = pos.get_mut(0) {
                            *pos_x = spawn_x as f64;
                        }
                        if let Some(Value::Double(ref mut pos_y)) = pos.get_mut(1) {
                            *pos_y = y;
                        }
                        if let Some(Value::Double(ref mut pos_z)) = pos.get_mut(2) {
                            *pos_z = spawn_z as f64;
                        }
                    }
                }
            }
        }
    }

    // Serialize and save the updated level.dat
    let serialized_data = match fastnbt::to_bytes(&nbt_data) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to serialize updated level.dat: {e}")),
    };

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    if let Err(e) = encoder.write_all(&serialized_data) {
        return Err(format!("Failed to compress updated level.dat: {e}"));
    }

    let compressed_data = match encoder.finish() {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to finalize compression for level.dat: {e}")),
    };

    // Write the updated level.dat file
    if let Err(e) = std::fs::write(level_path, compressed_data) {
        return Err(format!("Failed to write updated level.dat: {e}"));
    }

    Ok(())
}

// Puts the player on the world spawn column at terrain height + 3, after generation.
// `xzbbox` must be the box the world was generated from, post-rotation when a
// rotation was applied, since `ground` is indexed against it.
pub fn update_player_spawn_y_after_generation(
    world_path: &Path,
    xzbbox: &XZBBox,
    ground: &Ground,
) -> Result<(), String> {
    // Read the current level.dat file to get existing spawn coordinates
    let level_path = PathBuf::from(world_path).join("level.dat");
    if !level_path.exists() {
        return Err(format!("Level.dat not found at {level_path:?}"));
    }

    // Read the level.dat file
    let level_data = match std::fs::read(&level_path) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to read level.dat: {e}")),
    };

    // Decompress and parse the NBT data
    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    if let Err(e) = decoder.read_to_end(&mut decompressed_data) {
        return Err(format!("Failed to decompress level.dat: {e}"));
    }

    let mut nbt_data = match fastnbt::from_bytes::<Value>(&decompressed_data) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to parse level.dat NBT data: {e}")),
    };

    // Get existing spawn coordinates and calculate new Y based on terrain
    let (existing_spawn_x, existing_spawn_z) = if let Value::Compound(ref root) = nbt_data {
        if let Some(Value::Compound(ref data)) = root.get("Data") {
            let spawn_x = data.get("SpawnX").and_then(|v| {
                if let Value::Int(x) = v {
                    Some(*x)
                } else {
                    None
                }
            });
            let spawn_z = data.get("SpawnZ").and_then(|v| {
                if let Value::Int(z) = v {
                    Some(*z)
                } else {
                    None
                }
            });

            match (spawn_x, spawn_z) {
                (Some(x), Some(z)) => (x, z),
                _ => {
                    return Err("Spawn coordinates not found in level.dat".to_string());
                }
            }
        } else {
            return Err("Invalid level.dat structure: no Data compound".to_string());
        }
    } else {
        return Err("Invalid level.dat structure: root is not a compound".to_string());
    };

    // Calculate terrain-based Y coordinate
    let spawn_y = if ground.elevation_enabled {
        // Deriving the bbox from lat/lng here would give the pre-rotation
        // extents and sample the wrong point on rotated worlds.
        let relative_x = existing_spawn_x - xzbbox.min_x();
        let relative_z = existing_spawn_z - xzbbox.min_z();
        let terrain_point = XZPoint::new(relative_x, relative_z);

        ground.level(terrain_point) + 3 // Add 3 blocks above terrain for safety
    } else {
        -61 // Default Y if no terrain
    };

    // Update player position and world spawn point
    if let Value::Compound(ref mut root) = nbt_data {
        if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
            data.insert("SpawnY".to_string(), Value::Int(spawn_y));

            // The template pins Pos to (-5, -5), a column that is neither the spawn point
            // nor inside the generated regions. Move it onto the column just sampled,
            // matching what set_spawn_in_level_dat writes.
            if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
                if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
                    if let Some(Value::Double(ref mut pos_x)) = pos.get_mut(0) {
                        *pos_x = existing_spawn_x as f64;
                    }
                    if let Some(Value::Double(ref mut pos_y)) = pos.get_mut(1) {
                        *pos_y = spawn_y as f64;
                    }
                    if let Some(Value::Double(ref mut pos_z)) = pos.get_mut(2) {
                        *pos_z = existing_spawn_z as f64;
                    }
                }
            }
        }
    }

    // Serialize and save the updated level.dat
    let serialized_data = match fastnbt::to_bytes(&nbt_data) {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to serialize updated level.dat: {e}")),
    };

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    if let Err(e) = encoder.write_all(&serialized_data) {
        return Err(format!("Failed to compress updated level.dat: {e}"));
    }

    let compressed_data = match encoder.finish() {
        Ok(data) => data,
        Err(e) => return Err(format!("Failed to finalize compression for level.dat: {e}")),
    };

    // Write the updated level.dat file
    if let Err(e) = std::fs::write(level_path, compressed_data) {
        return Err(format!("Failed to write updated level.dat: {e}"));
    }

    Ok(())
}

/// Fetches a reduced-resolution elevation + land-cover grid for the 3D
/// terrain preview. Returns one raw binary blob (layout in preview_3d.rs)
/// so megabytes of grid data skip JSON serialization.
#[tauri::command]
async fn gui_get_terrain_preview(
    bbox_text: String,
    aws_only: bool,
) -> Result<tauri::ipc::Response, String> {
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        crate::preview_3d::build_preview_payload(&bbox_text, aws_only)
    })
    .await
    .map_err(|e| format!("Preview task failed: {e}"))??;
    Ok(tauri::ipc::Response::new(bytes))
}

/// ESA land-cover grid for the 3D preview, fetched lazily when the user
/// enables the overlay toggle (layout in preview_3d.rs).
#[tauri::command]
async fn gui_get_preview_landcover(bbox_text: String) -> Result<tauri::ipc::Response, String> {
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        crate::preview_3d::build_landcover_grid(&bbox_text)
    })
    .await
    .map_err(|e| format!("Preview land cover task failed: {e}"))??;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Facade wall quads for the 3D preview: lon/lat corners pushed clear of the
/// extruded footprint (see `preview_walls_from_cache`), wall height, and the
/// 8 px/m texture as a data URL.
///
/// Read from the facade cache, so whatever a generation or the Precompute
/// button has already built for this area shows without a setting and without
/// the network. An area nothing has been built for yields an empty list.
#[tauri::command]
async fn gui_get_preview_facades(bbox_text: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::mapillary::facades::preview_walls_from_cache(&bbox_text)
    })
    .await
    .map_err(|e| format!("Preview facades task failed: {e}"))?
}

/// Overture building footprints for the 3D preview as GeoJSON. Size-gated;
/// the frontend ignores all errors (buildings are a best-effort overlay).
#[tauri::command]
async fn gui_get_preview_buildings(bbox_text: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::preview_3d::build_buildings_geojson(&bbox_text)
    })
    .await
    .map_err(|e| format!("Preview buildings task failed: {e}"))?
}

#[tauri::command]
fn gui_get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Latest release info from the GitHub Releases API + a comparison to the running version.
///
/// Off the main thread for the same reason as [`gui_get_cache_size`]: this is a
/// blocking HTTPS request with a 5s connect and 10s read timeout, the front end
/// asks for it while the window is already on screen, and a command without
/// `async` runs inline on the thread that owns the webview. On a network that
/// drops the connection to GitHub rather than refusing it, that timeout was the
/// window not repainting.
#[tauri::command]
async fn gui_get_update_info() -> Result<version_check::UpdateInfo, String> {
    tauri::async_runtime::spawn_blocking(|| {
        version_check::check_for_updates().map_err(|e| format!("Update check failed: {e}"))
    })
    .await
    .map_err(|e| format!("Update check task failed: {e}"))?
}

/// Compile-time target platform: "windows" / "macos" / "linux" / "unknown".
#[tauri::command]
fn gui_get_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

/// How much disk every Arnis cache holds together, as a short string like
/// "812 MB". The settings panel shows it next to the clear button so the user
/// can tell whether clearing is worth it.
///
/// Off the main thread, because the cost is in the number of cached files
/// rather than in their size, and a `#[tauri::command]` without `async` runs
/// inline on the thread that owns the webview. A tile cache with a Mapillary
/// facade run in it reaches tens of thousands of files, and the walk was
/// freezing the window for seconds at a time; a late number is fine, a frozen
/// window is not.
#[tauri::command]
async fn gui_get_cache_size() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(cache_size_string)
        .await
        .map_err(|e| format!("Cache size task failed: {e}"))
}

/// Every cache root's size added up, as a short human string.
fn cache_size_string() -> String {
    crate::elevation::cache::format_size(cache_size_bytes())
}

/// Every cache root's size added up.
fn cache_size_bytes() -> u64 {
    use crate::elevation::cache::{dir_size_bytes, get_base_cache_dir};

    // The tile cache root already contains the Mapillary facade cache, which
    // lives under it as its own provider directory.
    let mut total = dir_size_bytes(&get_base_cache_dir());
    total = total.saturating_add(dir_size_bytes(&crate::land_cover::land_cover_cache_dir()));
    total = total.saturating_add(dir_size_bytes(&crate::canopy::canopy_cache_dir()));
    // Its own root beside the tile cache, and Clear Cache deletes it.
    total = total.saturating_add(dir_size_bytes(&crate::overture::cache_root()));
    if let Some(d) = crate::osm_tiles::cache_root() {
        total = total.saturating_add(dir_size_bytes(&d));
    }
    for root in crate::models_3d::model_cache_roots() {
        total = total.saturating_add(dir_size_bytes(&root));
    }
    total
}

/// The Mapillary imagery this generation used, one row per photograph, for the
/// License and Credits panel. Mapillary imagery is CC BY-SA and every image has
/// to name its photographer, so this is an obligation, not a nicety.
#[tauri::command]
fn gui_get_mapillary_attributions() -> Vec<MapillaryCreditRow> {
    crate::mapillary::credits::list()
        .into_iter()
        .map(|c| MapillaryCreditRow {
            title: c.title.clone(),
            username: c.uploader().to_string(),
            image_url: c.image_url(),
            // Empty where the export named no uploader; the panel then shows
            // the name as plain text rather than as a link that goes nowhere.
            profile_url: c.profile_url(),
        })
        .collect()
}

#[derive(serde::Serialize)]
struct MapillaryCreditRow {
    title: String,
    username: String,
    image_url: String,
    profile_url: String,
}

/// Wipe the elevation-tile, ESA-land-cover and Mapillary facade on-disk caches,
/// so subsequent generations re-download from the upstream providers. This is
/// what the "Clean tile cache" button in the GUI's Application settings panel
/// calls into.
///
/// Returns a single human-readable status line on success (the JS side
/// surfaces it as a toast-style notification), and an `Err` only when
/// one or more files couldn't be deleted; that case is rare (usually
/// a file still locked by a live generation run) but worth making
/// visible so the user knows the wipe was partial.
///
/// The cache roots themselves are left on disk; only their *contents*
/// are removed, so the next elevation/land-cover fetch doesn't have to
/// recreate the directory tree.
#[tauri::command]
async fn gui_clear_tile_caches() -> Result<String, String> {
    // Held for the whole wipe, not checked once: a generation that took the
    // slot while the files were still going would read its caches out from
    // under itself.
    let slot = BusySlot::acquire(BUSY_CLEAR)
        .map_err(|e| format!("{e} Clear the caches once it has finished."))?;
    // Off the webview thread for the same reason as `gui_get_cache_size`: the
    // cost is in the number of files, and a facade cache reaches tens of
    // thousands.
    tauri::async_runtime::spawn_blocking(move || {
        let _slot = slot;
        clear_tile_caches_now()
    })
    .await
    .map_err(|e| format!("Cache clear task failed: {e}"))?
}

fn clear_tile_caches_now() -> Result<String, String> {
    use crate::elevation::cache::clear_all_cached_tiles;
    use crate::land_cover::clear_land_cover_cache;
    use crate::models_3d::clear_model_caches;

    let combined = clear_all_cached_tiles()
        .combined(clear_land_cover_cache())
        .combined(crate::canopy::clear_canopy_cache())
        .combined(crate::overture::clear_overture_cache())
        .combined(crate::osm_tiles::clear_osm_tiles_cache())
        .combined(clear_model_caches());
    let megabytes = combined.bytes_freed as f64 / (1024.0 * 1024.0);

    if combined.errors > 0 {
        return Err(format!(
            "Cleared {} cached file{} ({:.1} MB), but {} file{} could not be removed",
            combined.files_deleted,
            if combined.files_deleted == 1 { "" } else { "s" },
            megabytes,
            combined.errors,
            if combined.errors == 1 { "" } else { "s" },
        ));
    }

    if combined.files_deleted == 0 {
        return Ok("Tile cache was already empty".to_string());
    }

    Ok(format!(
        "Cleared {} cached file{} ({:.1} MB freed)",
        combined.files_deleted,
        if combined.files_deleted == 1 { "" } else { "s" },
        megabytes,
    ))
}

/// Returns the world map image data as base64 and geo bounds for overlay display.
/// Returns None if the map image or metadata doesn't exist.
#[tauri::command]
fn gui_get_world_map_data(world_path: String) -> Result<Option<WorldMapData>, String> {
    // Prefer the just-finished generation's preview; Bedrock has no world dir.
    if let Some(r) = map_preview::last_preview_result() {
        if r.png_path.exists() {
            let image_data =
                fs::read(&r.png_path).map_err(|e| format!("Failed to read map image: {e}"))?;
            let base64_image =
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image_data);
            return Ok(Some(WorldMapData {
                image_base64: format!("data:image/png;base64,{}", base64_image),
                min_lat: r.min_lat,
                max_lat: r.max_lat,
                min_lon: r.min_lon,
                max_lon: r.max_lon,
                min_mc_x: r.min_mc_x,
                max_mc_x: r.max_mc_x,
                min_mc_z: r.min_mc_z,
                max_mc_z: r.max_mc_z,
            }));
        }
    }

    // Empty for Bedrock; don't fall back to reading files from the CWD.
    if world_path.is_empty() {
        return Ok(None);
    }

    let world_dir = PathBuf::from(&world_path);
    let map_path = world_dir.join("arnis_world_map.png");
    let metadata_path = world_dir.join("metadata.json");

    // Check if both files exist
    if !map_path.exists() || !metadata_path.exists() {
        return Ok(None);
    }

    // Read and encode the map image as base64
    let image_data = fs::read(&map_path).map_err(|e| format!("Failed to read map image: {e}"))?;
    let base64_image =
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image_data);

    // Read metadata
    let metadata_content =
        fs::read_to_string(&metadata_path).map_err(|e| format!("Failed to read metadata: {e}"))?;
    let metadata: serde_json::Value = serde_json::from_str(&metadata_content)
        .map_err(|e| format!("Failed to parse metadata: {e}"))?;

    // Extract geo bounds (metadata uses camelCase from serde)
    let min_lat = metadata["minGeoLat"]
        .as_f64()
        .ok_or("Missing minGeoLat in metadata")?;
    let max_lat = metadata["maxGeoLat"]
        .as_f64()
        .ok_or("Missing maxGeoLat in metadata")?;
    let min_lon = metadata["minGeoLon"]
        .as_f64()
        .ok_or("Missing minGeoLon in metadata")?;
    let max_lon = metadata["maxGeoLon"]
        .as_f64()
        .ok_or("Missing maxGeoLon in metadata")?;

    // Extract Minecraft coordinate bounds
    let min_mc_x = metadata["minMcX"].as_i64().unwrap_or(0) as i32;
    let max_mc_x = metadata["maxMcX"].as_i64().unwrap_or(0) as i32;
    let min_mc_z = metadata["minMcZ"].as_i64().unwrap_or(0) as i32;
    let max_mc_z = metadata["maxMcZ"].as_i64().unwrap_or(0) as i32;

    Ok(Some(WorldMapData {
        image_base64: format!("data:image/png;base64,{}", base64_image),
        min_lat,
        max_lat,
        min_lon,
        max_lon,
        min_mc_x,
        max_mc_x,
        min_mc_z,
        max_mc_z,
    }))
}

/// Data structure for world map overlay
#[derive(serde::Serialize)]
struct WorldMapData {
    image_base64: String,
    min_lat: f64,
    max_lat: f64,
    min_lon: f64,
    max_lon: f64,
    // Minecraft coordinate bounds for coordinate copying
    min_mc_x: i32,
    max_mc_x: i32,
    min_mc_z: i32,
    max_mc_z: i32,
}

#[derive(serde::Serialize)]
struct OneWorldInfo {
    world_path: String,
    exists: bool,
    /// The folder exists but is not a One World.
    foreign: bool,
    locked: bool,
    area_count: usize,
    scale: Option<f64>,
    height_multiplier: Option<f64>,
    terrain: Option<bool>,
    disable_height_limit: Option<bool>,
    aws_only_elevation: Option<bool>,
    /// Changes whenever an area is recorded.
    revision: u32,
}

fn one_world_dir(save_path: &str, world_name: &str) -> PathBuf {
    let name = crate::world_utils::world_folder_name(world_name)
        .unwrap_or_else(|| crate::one_world::DEFAULT_WORLD_NAME.to_string());
    PathBuf::from(save_path.trim()).join(name)
}

#[tauri::command(async)]
fn gui_one_world_info(save_path: String, world_name: String) -> Result<OneWorldInfo, String> {
    let world_path = one_world_dir(&save_path, &world_name);
    let manifest = crate::one_world::Manifest::load(&world_path)?;
    let foreign = manifest.is_none()
        && world_path.exists()
        && fs::read_dir(&world_path)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
    Ok(OneWorldInfo {
        world_path: world_path.display().to_string(),
        exists: manifest.is_some(),
        foreign,
        locked: manifest.is_some() && crate::world_utils::world_is_locked(&world_path),
        area_count: manifest.as_ref().map(|m| m.areas.len()).unwrap_or(0),
        scale: manifest.as_ref().map(|m| m.scale),
        height_multiplier: manifest.as_ref().map(|m| m.height_multiplier),
        terrain: manifest.as_ref().map(|m| m.terrain),
        disable_height_limit: manifest.as_ref().map(|m| m.disable_height_limit),
        aws_only_elevation: manifest.as_ref().map(|m| m.aws_only_elevation),
        revision: manifest.as_ref().map(|m| m.next_area_id).unwrap_or(0),
    })
}

/// Chunks of the selected area that already exist in the One World.
#[tauri::command(async)]
fn gui_one_world_overlap(
    save_path: String,
    world_name: String,
    bbox_text: String,
) -> Result<u64, String> {
    let world_path = one_world_dir(&save_path, &world_name);
    let Some(manifest) = crate::one_world::Manifest::load(&world_path)? else {
        return Ok(0);
    };
    let bbox = LLBBox::from_str(&bbox_text)?;
    let (rect, _) = crate::projection::snap_bbox_to_chunks(&manifest.projection(), &bbox)?;
    Ok(crate::one_world::existing_chunks(&world_path, &rect))
}

/// Map overlays plus the frame the teleport menu projects with.
#[derive(serde::Serialize)]
struct OneWorldOverlays {
    origin_lat: f64,
    origin_lon: f64,
    scale: f64,
    areas: Vec<WorldMapData>,
}

#[tauri::command(async)]
fn gui_get_one_world_overlays(world_path: String) -> Result<Option<OneWorldOverlays>, String> {
    let world_dir = PathBuf::from(world_path.trim());
    let Some(manifest) = crate::one_world::Manifest::load(&world_dir)? else {
        return Ok(None);
    };
    let mut out = Vec::with_capacity(manifest.areas.len());
    for area in &manifest.areas {
        let image_base64 = match area
            .preview
            .as_deref()
            .and_then(|rel| crate::one_world::safe_preview_path(&world_dir, rel))
        {
            Some(path) => match fs::read(path) {
                Ok(bytes) => format!(
                    "data:image/png;base64,{}",
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes)
                ),
                Err(_) => String::new(),
            },
            None => String::new(),
        };
        out.push(WorldMapData {
            image_base64,
            min_lat: area.min_lat,
            max_lat: area.max_lat,
            min_lon: area.min_lon,
            max_lon: area.max_lon,
            min_mc_x: area.min_x,
            max_mc_x: area.max_x,
            min_mc_z: area.min_z,
            max_mc_z: area.max_z,
        });
    }
    Ok(Some(OneWorldOverlays {
        origin_lat: manifest.origin_lat,
        origin_lon: manifest.origin_lon,
        scale: manifest.scale,
        areas: out,
    }))
}

/// Reveals a file or folder in the system file explorer.
/// On Windows, opens files with the default application (e.g. .mcworld with Minecraft
/// Bedrock), except OneDrive paths which are revealed in Explorer to avoid the shell's
/// "cannot find" error on unsynced/placeholder files. Directories always open in Explorer.
#[tauri::command]
fn gui_show_in_folder(path: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        // OneDrive files can be cloud placeholders / mid-sync that `start` can't launch
        // ("Windows cannot find <path>"), so reveal-and-highlight instead of opening.
        if path.to_lowercase().contains("onedrive") {
            std::process::Command::new("explorer")
                .args(["/select,", &path])
                .spawn()
                .map_err(|e| format!("Failed to open explorer: {}", e))?;
        } else if std::process::Command::new("cmd")
            // Otherwise open with the default app (e.g. .mcworld with Minecraft Bedrock);
            // for directories `start ""` opens Explorer. Falls back to explorer /select.
            .args(["/C", "start", "", &path])
            .spawn()
            .is_err()
        {
            std::process::Command::new("explorer")
                .args(["/select,", &path])
                .spawn()
                .map_err(|e| format!("Failed to open explorer: {}", e))?;
        }
    }

    #[cfg(target_os = "macos")]
    {
        // On macOS, just reveal in Finder
        std::process::Command::new("open")
            .args(["-R", &path])
            .spawn()
            .map_err(|e| format!("Failed to open Finder: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        // On Linux, just show in file manager
        let path_parent = std::path::Path::new(&path)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());

        // Try nautilus with select first, then fall back to xdg-open on parent
        if std::process::Command::new("nautilus")
            .args(["--select", &path])
            .spawn()
            .is_err()
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(&path_parent)
                .spawn();
        }
    }

    Ok(())
}

/// What the settings row shows after a precompute: one line and its tooltip.
///
/// `built` is what the row's colour means. Green is "there are facades here
/// now", so a run that was cancelled and a run that found nothing both come
/// back plain rather than as a success or as a failure: neither is something
/// the user did wrong, and neither left a facade behind.
#[derive(serde::Serialize)]
struct PrecomputeOutcome {
    summary: String,
    detail: String,
    built: bool,
}

/// Set by [`gui_cancel_precompute`] and read by the running pipeline.
///
/// One flag for the process, because [`BUSY`] already allows only one
/// precompute at a time. It is cleared when a precompute starts, so a cancel
/// left over from the previous one cannot stop the next before it begins.
static PRECOMPUTE_CANCEL: std::sync::OnceLock<Arc<std::sync::atomic::AtomicBool>> =
    std::sync::OnceLock::new();

fn precompute_cancel() -> &'static Arc<std::sync::atomic::AtomicBool> {
    PRECOMPUTE_CANCEL.get_or_init(|| Arc::new(std::sync::atomic::AtomicBool::new(false)))
}

/// Fetches the Mapillary imagery for the selected box and runs the whole facade
/// pipeline over it, so the walls are in the cache before any world asks.
///
/// Returns the line the settings row shows, `Ok` or `Err` alike: what came back
/// is what the user reads, because the point of the button is that pressing it
/// never leaves them wondering what happened. `Err` is only for a refusal or a
/// failure; a run that was cancelled or found nothing is an `Ok` with `built`
/// false, since neither is something the user did wrong.
#[tauri::command]
async fn gui_precompute_facades(
    bbox_text: String,
    mapillary_token: String,
) -> Result<PrecomputeOutcome, String> {
    tauri::async_runtime::spawn_blocking(move || precompute_facades(&bbox_text, &mapillary_token))
        .await
        .map_err(|e| format!("Precompute task failed: {e}"))?
}

fn precompute_facades(bbox_text: &str, token: &str) -> Result<PrecomputeOutcome, String> {
    use crate::mapillary::{bbox_area_m2, PRECOMPUTE_MAX_AREA_M2};

    let token = token.trim();
    if token.is_empty() {
        return Err("Add a Mapillary token above: there is nothing to fetch without one.".into());
    }
    let bbox = LLBBox::from_str(bbox_text.trim())
        .map_err(|_| "Select an area on the map first.".to_string())?;

    // Before the slot, so a box that will be refused does not first make the
    // Generate button unavailable for as long as it takes to say so.
    let area = bbox_area_m2(bbox);
    if area > PRECOMPUTE_MAX_AREA_M2 {
        // The row is one line of the settings panel, so a refusal says what is
        // wrong there and puts the reason behind it after a blank line, which
        // the front end hangs on the row as its tooltip.
        return Err(format!(
            "This area is {:.2} km², over the {:.2} km² limit.\n\n\
             What the pipeline costs follows the ground the box covers: 0.034 km² of Munich \
             took under twenty minutes and 470 MB from cold, which puts this limit at the \
             better part of an hour already. Precompute a large area in pieces instead; the \
             cache keeps every wall each piece builds, and no piece redoes another's.",
            area / 1e6,
            PRECOMPUTE_MAX_AREA_M2 / 1e6,
        ));
    }

    let _slot = BusySlot::acquire(BUSY_PRECOMPUTE)?;
    let cancel = precompute_cancel();
    cancel.store(false, std::sync::atomic::Ordering::Release);

    match crate::mapillary::precompute(bbox, token, Arc::clone(cancel)) {
        Ok(report) => Ok(PrecomputeOutcome {
            summary: report.summary(),
            detail: report.detail(),
            built: report.walls > 0,
        }),
        // Not an error: the user asked for it. The pipeline stops between
        // stages, and the walls it had already finished are written per wall as
        // they are built rather than at the end, so they are kept and the next
        // precompute over this area starts from them.
        Err(e) if e == "cancelled" => Ok(PrecomputeOutcome {
            summary: "Precompute cancelled. The walls it had already built are cached.".to_string(),
            detail: format!(
                "Cancelling waits for the stage in flight, so the run may have gone on for some \
                 minutes after the button. Press Precompute again to carry on from what is in {}.",
                crate::mapillary::facade_cache_dir().display()
            ),
            built: false,
        }),
        // A refused token, an Overpass outage, a download that never arrived:
        // the pipeline's own words, said whole rather than summarised, because
        // they are the only thing that says which of those it was. The prefix
        // is here so the row is not a bare technical sentence with no subject.
        Err(e) => Err(format!("Precompute failed: {e}")),
    }
}

/// Asks a running precompute to stop.
///
/// It stops at the next stage boundary or the next wall, not immediately: the
/// imagery search and the registration stage are each one call and neither is
/// interruptible, so a cancel during the long middle of a cold run is noticed
/// when that stage ends. Idempotent, and harmless when nothing is running.
#[tauri::command]
fn gui_cancel_precompute() {
    precompute_cancel().store(true, std::sync::atomic::Ordering::Release);
}

/// What owns the process: nothing, a generation, or a facade precompute.
///
/// Set while a generation owns the process. The world floor, terrain floor and filler-chunk
/// base are process globals read from deep inside the block writers, and the terrain floor
/// is derived from the bbox's own elevation, so a second run would retune all three under the
/// first one's feet. The progress channel and world path are shared besides.
///
/// One value rather than a flag per job, because two atomics can be taken by
/// two callers at once. The precompute has to exclude a generation for a reason
/// of its own: `pipeline::run` clears the Mapillary attribution store on entry,
/// so a precompute started beside a generation would take the credits of the
/// world being built with it.
static BUSY: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(BUSY_IDLE);
const BUSY_IDLE: u8 = 0;
const BUSY_GENERATION: u8 = 1;
const BUSY_PRECOMPUTE: u8 = 2;
const BUSY_CLEAR: u8 = 3;
const BUSY_BAKE: u8 = 4;

/// Owns [`BUSY`] for the length of one job and clears it on drop, including
/// on the early-return paths before the worker is spawned.
#[derive(Debug)]
struct BusySlot;

impl BusySlot {
    /// `Err` naming the holder when something else already owns the process.
    fn acquire(job: u8) -> Result<Self, String> {
        use std::sync::atomic::Ordering;
        match BUSY.compare_exchange(BUSY_IDLE, job, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Ok(Self),
            // Short on purpose: `emit_gui_error` cuts a message at 35
            // characters, and a sentence that ends mid-word says less than a
            // short one that finishes.
            Err(BUSY_PRECOMPUTE) => Err("A precompute is running.".to_string()),
            Err(BUSY_CLEAR) => Err("The caches are being cleared.".to_string()),
            Err(BUSY_BAKE) => Err("A country bake is running.".to_string()),
            Err(_) => Err("A generation is already running.".to_string()),
        }
    }
}

impl Drop for BusySlot {
    fn drop(&mut self) {
        BUSY.store(BUSY_IDLE, std::sync::atomic::Ordering::Release);
    }
}

/// The command line that asks the CLI for what `args` holds, without the
/// executable. A job's pieces are runs of this executable, and the window
/// was started without one, so the job's own is rebuilt from the settings:
/// the window's own fields, then `flags`, the Advanced Features flags `args`
/// was parsed from. `--bbox`, the spawn and the per-process knobs are the
/// coordinator's to set per piece. `world_path` is the One World folder;
/// without one the line asks for a single run's inputs (a prewarm).
fn piece_argv(args: &Args, world_path: Option<&Path>, flags: &[String]) -> Vec<std::ffi::OsString> {
    use clap::ValueEnum;
    fn name<T: ValueEnum>(v: &T) -> String {
        v.to_possible_value()
            .map(|p| p.get_name().to_string())
            .unwrap_or_default()
    }
    let mut out: Vec<std::ffi::OsString> = Vec::new();
    if let Some(world_path) = world_path {
        out.push("--one-world".into());
        if let (Some(dir), Some(world)) = (world_path.parent(), world_path.file_name()) {
            out.extend(["--output-dir".into(), dir.into(), "--world-name".into()]);
            out.push(world.into());
        }
    }
    let mut values = vec![
        format!("--downloader={}", args.downloader),
        format!("--scale={}", args.scale),
        format!("--height-multiplier={}", args.height_multiplier),
        format!("--body={}", name(&args.body)),
        format!("--projection={}", args.projection),
        format!("--ground-level={}", args.ground_level),
        format!("--mode={}", name(&args.mode)),
        format!("--interior={}", args.interior),
        format!("--max-tree-size={}", name(&args.max_tree_size)),
        format!("--canopy-height={}", args.canopy_height),
        format!("--overture={}", args.overture),
        format!("--overture-source={}", name(&args.overture_source)),
        format!("--rotation={}", args.rotation),
        format!("--map-item={}", args.map_item),
        format!("--gamemode={}", name(&args.gamemode)),
        format!("--world-time={}", args.world_time),
        format!("--world-type={}", name(&args.world_type)),
        format!("--signage={}", name(&args.signage)),
        format!(
            "--mapillary-facade-mode={}",
            name(&args.mapillary_facade_mode)
        ),
        format!("--facade-detail={}", name(&args.facade_detail)),
        format!("--facade-px={}", args.facade_px),
    ];
    let switches = [
        ("--fillground", args.fillground),
        ("--caves", args.caves),
        ("--legacy-trees", args.legacy_trees),
        ("--no-3d", !args.use_3d),
        ("--debug", args.debug),
        ("--disable-height-limit", args.disable_height_limit),
        ("--aws-only-elevation", args.aws_only_elevation),
        ("--bake-lighting", args.bake_lighting),
        ("--voxy-lod", args.voxy_lod),
        ("--map-preview", args.map_preview),
        ("--building-facades", args.building_facades),
    ];
    values.extend(switches.iter().filter(|f| f.1).map(|f| f.0.to_string()));
    if let Some(t) = args.timeout {
        values.push(format!("--timeout={}", t.as_secs()));
    }
    if let Some(on) = args.mapillary_facades {
        values.push(format!("--mapillary-facades={on}"));
    }
    // ponytail: the token rides on the pieces' command lines, visible to this
    // user's other processes; pass it in their environment if that matters.
    if let Some(token) = &args.mapillary_token {
        values.push(format!("--mapillary-token={token}"));
    }
    out.extend(
        values
            .into_iter()
            .chain(flags.iter().cloned())
            .map(Into::into),
    );
    out
}

/// The Advanced Features settings, parsed by the CLI's own parser so the
/// window accepts exactly what the flags accept. `flags` are `--name=value`
/// tokens; none is the stock run. The checks `validate_args` makes on these flags are
/// repeated, as the window never runs it.
fn meld_args(flags: &[String], one_world: bool) -> Result<Args, String> {
    use clap::Parser;
    let args =
        Args::try_parse_from(std::iter::once("arnis").chain(flags.iter().map(String::as_str)))
            .map_err(|e| {
                let text = e.to_string();
                let line = text.lines().next().unwrap_or_default();
                line.trim_start_matches("error: ").to_string()
            })?;
    args.snow.validate(one_world)?;
    if let Some(y) = args.cave_datum_y {
        crate::args::check_cave_datum_y(y)?;
    }
    Ok(args)
}

/// Offline Mode stops a run the caches could not serve, naming what they
/// lacked, rather than build flat or empty ground there.
fn offline_complete() -> Result<(), String> {
    let missing = crate::net::offline_misses();
    if !crate::net::offline() || missing.is_empty() {
        return Ok(());
    }
    let what: Vec<String> = missing.into_iter().map(|(what, _)| what).collect();
    let msg = format!(
        "Offline Mode: not downloaded yet: {}. Download the area for offline use first.",
        what.join(", ")
    );
    // In full, where emit_gui_error would cut the list short.
    emit_gui_progress_update(0.0, &format!("Error! {msg}"));
    Err(msg)
}

/// `--prewarm` lives in the CLI, so the window runs it there, on the same
/// command line a piece gets, with its progress on the window's bar.
/// Stop (`gui_cancel_bake`) kills the child; its half-done extract download
/// is removed, and a bake is written whole or not at all.
fn prewarm_in_child(argv: &[std::ffi::OsString], bbox_text: &str) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    BAKE_CANCEL.store(false, Ordering::Release);
    emit_gui_progress_update(0.0, "Downloading for offline use...");
    let run = crate::scale::run_piece_until(argv, &[], Some(&BAKE_CANCEL), |p| {
        emit_gui_progress_update(p * 100.0, "")
    });
    if BAKE_CANCEL.swap(false, Ordering::AcqRel) {
        crate::osm_pbf::remove_partial_downloads();
        emit_gui_progress_update(
            progress::MESSAGE_ONLY,
            "Stopped. What finished is cached; nothing half written was left.",
        );
        return Ok(());
    }
    match run {
        Ok(_) => {
            emit_gui_progress_update(100.0, &format!("Done! Cached for offline use: {bbox_text}"));
            Ok(())
        }
        Err(failure) => {
            // The child's own error line, not its whole output tail.
            let lines = failure.message.lines();
            let line = lines
                .clone()
                .rev()
                .find(|l| l.contains("Error"))
                .or(lines.last())
                .unwrap_or_default()
                .to_string();
            emit_gui_progress_update(0.0, &format!("Error! {line}"));
            Err(failure.message)
        }
    }
}

// Everything before the `spawn` below - the spawn point written into level.dat,
// the tall-world datapack install - runs synchronously in this call, and a plain
// `#[tauri::command]` would run all of it on the main thread with the window
// stalled behind it. `(async)` puts it on the async runtime instead, so the GUI
// stays live from the click until the first progress event.
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
#[allow(unused_variables)]
fn gui_start_generation(
    bbox_text: String,
    selected_world: String,
    bedrock_save_path: String,
    luanti_save_path: String,
    world_scale: f64,
    height_multiplier: f64,
    ground_level: i32,
    terrain_enabled: bool,
    skip_osm_objects: bool,
    interior_enabled: bool,
    fillground_enabled: bool,
    caves_enabled: bool,
    legacy_trees_enabled: bool,
    max_tree_size: String,
    canopy_height_enabled: bool,
    overture_enabled: bool,
    use_3d_enabled: bool,
    disable_height_limit: bool,
    aws_only_elevation: bool,
    bake_lighting_enabled: bool,
    voxy_lod_enabled: bool,
    is_new_world: bool,
    spawn_point: Option<(f64, f64)>,
    telemetry_consent: bool,
    world_format: String,
    rotation_angle: f64,
    gamemode: String,
    world_time: i64,
    world_type: String,
    map_item: bool,
    signage: String,
    mapillary_token: String,
    facades_enabled: bool,
    facade_mode: String,
    building_facades_enabled: bool,
    facade_detail: String,
    celestial_body_name: String,
    one_world: bool,
    one_world_name: String,
    // Advanced Features, as CLI flags (`--name=value`, or a bare switch).
    // None is the stock run; the per-process and piece flags among them
    // (--threads, --unit-regions, ...) are read the same way.
    flags: Vec<String>,
) -> Result<(), String> {
    use progress::emit_gui_error;
    use LLBBox;

    // A One World is resolved inside the worker, once `Args` exist.
    let one_world = one_world && world_format == "java" && celestial_body_name == "earth";
    let is_new_world = is_new_world && !one_world;

    // Claim the process before touching any shared state. The frontend disables its button
    // for the same reason; this is the authoritative check behind it.
    let generation_slot = match BusySlot::acquire(BUSY_GENERATION) {
        Ok(slot) => slot,
        Err(msg) => {
            emit_gui_error(&msg);
            return Err(msg);
        }
    };

    progress::reset_progress_floor();

    let mut meld = match meld_args(&flags, one_world) {
        Ok(meld) => meld,
        Err(msg) => {
            emit_gui_error(&msg);
            return Err(msg);
        }
    };
    // Process-wide, so set every run: a run without a table gets the built-in
    // one back. Unlike the CLI, which warns and goes on, a table that does not
    // load stops the run, since the user picked it in this window.
    let loot = match meld
        .loot_table
        .as_deref()
        .map(crate::element_processing::subprocessor::buildings_loot::load_loot_table)
    {
        Some(Err(e)) => {
            let msg = format!("Chest Loot Table: {e}");
            emit_gui_error(&msg);
            return Err(msg);
        }
        loaded => loaded.and_then(Result::ok),
    };
    crate::element_processing::subprocessor::buildings_loot::set_loot_table(loot);

    let process = std::mem::take(&mut meld.process);
    // Process-wide, so set every run: a run without them gets the network
    // and Arnis's own Overpass back.
    crate::net::set_offline(process.offline);
    crate::retrieve_data::set_overpass_urls(process.overpass_url.clone());
    // A prewarm only downloads, so no world is made, locked or named for it.
    let prewarm = process.prewarm;
    let is_new_world = is_new_world && !prewarm;
    // Pieces are a One World feature; anywhere else the fields are inert.
    let units = if one_world {
        std::mem::take(&mut meld.units)
    } else {
        Default::default()
    };
    // The global pool was built once at startup, so a per-run count gets its
    // own pool. ponytail: threads that are not rayon workers (std::thread
    // spawns inside the run) still fan out on the global pool.
    let pool = process
        .thread_count()
        .and_then(|n| rayon::ThreadPoolBuilder::new().num_threads(n).build().ok());
    // Process-wide, so set every run: a run without the knob gets the stock
    // ceiling back.
    crate::net::set_max_requests(
        process
            .max_downloads
            .map_or(crate::net::MAX_CONCURRENT_REQUESTS, |n| n as usize),
    );

    // Resolved before validation: off Earth the slider value is ignored, so
    // validating it could reject a run over a scale that never gets used.
    // Substituted here, not just in Args, because the spawn transform and world
    // bounds below must see the same scale.
    let celestial_body = crate::celestial::CelestialBody::from_str_lossy(&celestial_body_name);
    let world_scale = if celestial_body.is_earth() {
        world_scale
    } else {
        celestial_body.world_scale()
    };
    // apply_body_defaults clears this off Earth, but it runs after the datapack install below
    // and after the Args literal is built, so both would still see the raw frontend value.
    let disable_height_limit = disable_height_limit && celestial_body.is_earth();

    // The GUI builds Args directly and never runs validate_args, so guard the scale here
    // rather than letting it panic deep in the coordinate transform after the fetch.
    if celestial_body.is_earth() {
        if let Err(e) = crate::args::validate_scale(world_scale) {
            emit_gui_error(&e);
            return Err(e);
        }
    }
    if let Err(e) = crate::args::validate_height_multiplier(height_multiplier) {
        emit_gui_error(&e);
        return Err(e);
    }

    // Store telemetry consent for crash reporting
    telemetry::set_telemetry_consent(telemetry_consent);

    // Send generation click telemetry
    telemetry::send_generation_click();

    // For new Java worlds, set the spawn point in level.dat
    // Only update player position for Java worlds - Bedrock worlds don't have a pre-existing
    // level.dat to modify (the spawn point will be set when the .mcworld is created)
    if is_new_world && world_format != "bedrock" && !world_format.starts_with("luanti") {
        let prep_result: Result<(), String> = (|| -> Result<(), String> {
            let llbbox = LLBBox::from_str(&bbox_text)
                .map_err(|e| format!("Failed to parse bounding box: {e}"))?;

            let (transformer, xzbbox) = CoordTransformer::llbbox_to_xzbbox(&llbbox, world_scale)
                .map_err(|e| format!("Failed to create coordinate transformer: {e}"))?;

            let (spawn_x, spawn_z) = if let Some(coords) = spawn_point {
                let llpoint = LLPoint::new(coords.0, coords.1)
                    .map_err(|e| format!("Failed to parse spawn point: {e}"))?;

                if llbbox.contains(&llpoint) {
                    let xzpoint = transformer.transform_point(llpoint);
                    (xzpoint.x, xzpoint.z)
                } else {
                    calculate_default_spawn(&xzbbox)
                }
            } else {
                calculate_default_spawn(&xzbbox)
            };

            let (spawn_x, spawn_z) = map_transformation::rotate::rotate_xz_point(
                spawn_x,
                spawn_z,
                rotation_angle.clamp(-90.0, 90.0),
                &xzbbox,
            );

            set_player_spawn_in_level_dat(&selected_world, spawn_x, spawn_z)
                .map_err(|e| format!("Failed to set spawn point: {e}"))?;

            if disable_height_limit {
                crate::world_utils::install_tall_datapack(std::path::Path::new(&selected_world))
                    .map_err(|e| format!("Failed to install tall-world datapack: {e}"))?;
            }

            Ok(())
        })();

        if let Err(error_msg) = prep_result {
            eprintln!("{error_msg}");
            emit_gui_error(&error_msg);
            remove_new_java_world(&PathBuf::from(&selected_world));
            return Err(error_msg);
        }
    }

    tauri::async_runtime::spawn(async move {
        // Held until the worker finishes, on every path, so the globals stay this run's.
        let _generation_slot = generation_slot;
        let work = move || {
            let world_path = if one_world {
                one_world_dir(&selected_world, &one_world_name)
            } else {
                PathBuf::from(&selected_world)
            };

            // Determine world format from UI selection first (needed for session lock decision)

            let luanti_game = if world_format.starts_with("luanti") {
                Some(crate::luanti_block_map::LuantiGame::Mineclonia)
            } else {
                None
            };

            let world_format = if world_format == "bedrock" {
                WorldFormat::BedrockMcWorld
            } else if world_format.starts_with("luanti") {
                WorldFormat::LuantiWorld
            } else {
                WorldFormat::JavaAnvil
            };

            // Arm cleanup for freshly created Java worlds. Declared before the
            // SessionLock so the lock's file handle is released first on drop
            // (Windows needs that to remove the parent folder).
            let mut cleanup_guard: Option<NewWorldCleanup> =
                if is_new_world && world_format == WorldFormat::JavaAnvil {
                    Some(NewWorldCleanup::new(world_path.clone()))
                } else {
                    None
                };

            // Resolved up front because the disk space check below needs it
            let bedrock_output_dir = match world_format {
                WorldFormat::BedrockMcWorld => resolve_bedrock_output_dir(&bedrock_save_path),
                _ => PathBuf::new(),
            };
            let luanti_output_dir = match world_format {
                WorldFormat::LuantiWorld => resolve_luanti_output_dir(&luanti_save_path),
                _ => PathBuf::new(),
            };

            // Check available disk space before starting generation (minimum 3GB required)
            const MIN_DISK_SPACE_BYTES: u64 = 3 * 1024 * 1024 * 1024; // 3 GB
            let check_path = match world_format {
                WorldFormat::JavaAnvil => world_path.clone(),
                WorldFormat::BedrockMcWorld => bedrock_output_dir.clone(),
                WorldFormat::LuantiWorld => luanti_output_dir.clone(),
            };
            // Probe the nearest existing ancestor: a missing or space-containing
            // path otherwise confuses the Windows volume lookup, which then reports
            // 0 bytes free. Only block on a confident positive reading; treat an
            // error or a 0/undeterminable result as "can't tell" and proceed (#824).
            let probe_path = {
                let mut p = check_path.as_path();
                loop {
                    if p.exists() {
                        break p.to_path_buf();
                    }
                    match p.parent() {
                        Some(parent) => p = parent,
                        // No existing ancestor (e.g. a bare relative path): probe
                        // the current dir so the query always hits a real path.
                        None => {
                            break std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                        }
                    }
                }
            };
            match fs2::available_space(&probe_path) {
                Ok(available) if available > 0 && available < MIN_DISK_SPACE_BYTES => {
                    let error_msg = "Not enough disk space available.".to_string();
                    eprintln!("{error_msg}");
                    emit_gui_error(&error_msg);
                    return Err(error_msg);
                }
                Ok(_) => {} // Sufficient, or 0/undeterminable: don't false-block
                Err(e) => {
                    // Log warning but don't block generation if we can't check space
                    eprintln!("Warning: Could not check disk space: {e}");
                }
            }

            // Acquire session lock for Java worlds only
            // Session lock prevents Minecraft from having the world open during generation
            // Bedrock worlds are generated as .mcworld files and don't need this lock.
            // A One World is locked by `prepare` below.
            let mut _session_lock: Option<SessionLock> =
                if world_format == WorldFormat::JavaAnvil && !one_world && !prewarm {
                    match SessionLock::acquire(&world_path) {
                        Ok(lock) => Some(lock),
                        Err(e) => {
                            let error_msg = format!("Failed to acquire session lock: {e}");
                            eprintln!("{error_msg}");
                            emit_gui_error(&error_msg);
                            return Err(error_msg);
                        }
                    }
                } else {
                    None
                };

            // Parse the bounding box from the text with proper error handling
            let mut bbox = match LLBBox::from_str(&bbox_text) {
                Ok(bbox) => bbox,
                Err(e) => {
                    let error_msg = format!("Failed to parse bounding box: {e}");
                    eprintln!("{error_msg}");
                    emit_gui_error(&error_msg);
                    return Err(error_msg);
                }
            };

            // Determine output path and level name based on format
            let (generation_path, level_name) = match world_format {
                _ if prewarm => (world_path.clone(), None),
                WorldFormat::JavaAnvil => {
                    // Java: use the selected world path, add localized name if new
                    let updated_path = if is_new_world {
                        add_localized_world_name(world_path.clone(), &bbox, celestial_body)
                    } else {
                        world_path.clone()
                    };
                    (updated_path, None)
                }
                WorldFormat::BedrockMcWorld => {
                    // Bedrock: generate .mcworld in the configured directory
                    let (output_path, lvl_name) =
                        crate::world_utils::build_bedrock_output(&bbox, bedrock_output_dir);
                    progress::emit_world_name_update(&lvl_name);
                    (output_path, Some(lvl_name))
                }
                WorldFormat::LuantiWorld => {
                    let worlds_dir = luanti_output_dir.clone();
                    let _ = std::fs::create_dir_all(&worlds_dir);
                    let mut counter = 1;
                    let world_name = loop {
                        let candidate = format!("Arnis Luanti World {counter}");
                        if !worlds_dir.join(&candidate).exists() {
                            break candidate;
                        }
                        counter += 1;
                    };
                    let luanti_path = worlds_dir.join(&world_name);
                    println!(
                        "Creating Luanti world at: {}",
                        luanti_path.display().to_string().bright_white().bold()
                    );
                    (luanti_path, Some(world_name))
                }
            };

            // Create generation options. Spawn point and facade job are set below.
            let mut generation_options = GenerationOptions {
                path: generation_path.clone(),
                format: world_format,
                level_name,
                spawn_point: None,
                luanti_game,
                ground_level,
                facades: crate::mapillary::FacadeJob::default(),
            };

            // Create an Args instance with the chosen bounding box
            // Note: path is used for Java-specific features like spawn point update
            let single_tall_java =
                world_format == WorldFormat::JavaAnvil && !one_world && disable_height_limit;
            let mut args: Args = Args {
                bbox: Some(bbox),
                file: meld.file,
                save_json_file: None,
                path: Some(if world_format == WorldFormat::JavaAnvil {
                    generation_path.clone()
                } else {
                    world_path.clone()
                }),
                bedrock: world_format == WorldFormat::BedrockMcWorld,
                luanti: world_format == WorldFormat::LuantiWorld,
                downloader: "requests".to_string(),
                scale: world_scale,
                height_multiplier,
                projection: crate::projection::ProjectionKind::Local,
                one_world: false,
                world_name: None,
                origin: if one_world { meld.origin } else { None },
                one_world_run: None,
                ground_level,
                mode: if skip_osm_objects {
                    crate::args::GenerationMode::TerrainOnly
                } else if terrain_enabled {
                    crate::args::GenerationMode::GeoTerrain
                } else {
                    crate::args::GenerationMode::GeoOnly
                },
                legacy_terrain: false,
                interior: interior_enabled,
                loot_table: meld.loot_table,
                dump_loot_table: None,
                map_item_only: false,
                fillground: fillground_enabled,
                caves: caves_enabled,
                // The asset pack, biome mix and zone preview are CLI aids; the GUI toggle
                // carves with the defaults and a `cave-pack` folder next to the executable.
                cave_asset_pack: None,
                cave_biomes: None,
                cave_zone_map: None,
                cave_zone_map_step: None,
                cave_seed: meld.cave_seed,
                cave_datum_y: meld.cave_datum_y,
                seed: meld.seed,
                legacy_trees: legacy_trees_enabled,
                max_tree_size: crate::trees::tree_library::TreeSize::from_str_lossy(&max_tree_size),
                tree_realm: meld.tree_realm,
                tree_size_weights: meld.tree_size_weights,
                tree_pack_dir: meld.tree_pack_dir,
                tree_pack_mode: meld.tree_pack_mode,
                init_tree_pack_dir: None,
                export_tree_packs: None,
                canopy_height: canopy_height_enabled,
                // Overture only adds buildings, as in run_cli.
                overture: overture_enabled && meld.buildings,
                buildings: meld.buildings,
                // Auto picks whichever transport is cheaper for the area. The
                // two are not bit-identical - tiles quantise coordinates to a
                // 0.4 m lattice and keep the largest ring of a multipolygon the
                // Parquet reader drops entirely - but both differences are far
                // below a block, so the choice is not worth a GUI setting.
                overture_source: crate::args::OvertureSource::Auto,
                osm_tiles_url: meld.osm_tiles_url,
                no_tile_archive: meld.no_tile_archive,
                osm_pbf: meld.osm_pbf,
                osm_pbf_url: meld.osm_pbf_url,
                use_3d: use_3d_enabled,
                props: meld.props,
                props_min_scale: meld.props_min_scale,
                debug: false,
                timeout: Some(std::time::Duration::from_secs(40)),
                spawn_lat: None,
                spawn_lng: None,
                rotation: rotation_angle.clamp(-90.0, 90.0),
                disable_height_limit,
                // A One World fixes its own build height, and the pair only
                // means something for a tall Java world.
                min_y: meld.min_y.filter(|_| single_tall_java),
                max_y: meld.max_y.filter(|_| single_tall_java),
                aws_only_elevation,
                benchmark: false,
                bake_lighting: bake_lighting_enabled,
                voxy_lod: voxy_lod_enabled,
                gamemode: crate::args::GameMode::from_str_lossy(&gamemode),
                world_time: world_time.clamp(0, 23999),
                world_type: crate::args::WorldType::from_str_lossy(&world_type),
                // One World merges into Anvil files, and only Java has them.
                region_format: if world_format == WorldFormat::JavaAnvil && !one_world {
                    meld.region_format
                } else {
                    crate::args::RegionFormat::Mca
                },
                blinear_level: meld.blinear_level,
                world_border: meld.world_border && world_format == WorldFormat::JavaAnvil,
                map_item,
                // Frontend refuses previews for rotated worlds, skip the work there.
                map_preview: world_format != WorldFormat::LuantiWorld
                    && rotation_angle.abs() <= f64::EPSILON,
                signage: crate::args::SignageLevel::from_str_lossy(&signage),
                road_detail: meld.road_detail,
                // The settings toggle and the token together: the toggle is what
                // the user turns off to keep a saved token without paying for the
                // download, and without a token there is nothing to fetch.
                mapillary_facades: Some(facades_enabled),
                mapillary_token: Some(mapillary_token.trim().to_string()).filter(|t| !t.is_empty()),
                mapillary_probe: false,
                mapillary_debug_dir: None,
                // A CLI aid only: `--mapillary-facades-dir` builds from a
                // prepared export instead of fetching one, which is how the
                // Python lab's output is reviewed. The GUI fetches into the
                // cache and the Precompute button fills it, so it has no field.
                mapillary_facades_dir: None,
                // Dumping a wall's intermediate products is a CLI debug aid.
                mapillary_facade_debug_dir: None,
                mapillary_facade_debug_walls: String::new(),
                // Passed through even on Bedrock and Luanti, where the photo
                // panels cannot work: `facades::install` builds the blocks and
                // drops the panels, and `generate_world_with_options` says so
                // out loud. Coercing it here would only hide a stale setting.
                mapillary_facade_mode: crate::args::FacadeMode::from_str_lossy(&facade_mode),
                // The frontend already sends false on a world format that
                // cannot show item displays, and `data_processing` checks the
                // format again, so a stale setting cannot leak through.
                building_facades: building_facades_enabled,
                facade_detail: crate::args::FacadeDetail::from_str_lossy(&facade_detail),
                // No GUI field: the detail level above already says how much
                // atlas the panels may take, and the budget lowers this when
                // it has to.
                facade_px: 16,
                // The set is compiled in; pointing at a replacement is a CLI
                // aid.
                building_facades_dir: None,
                body: celestial_body,
                // Stock unless Advanced Features set a knob.
                scatter: meld.scatter,
                fields: meld.fields,
                process,
                units,
                snow: meld.snow,
                water: meld.water,
                climate_mode: meld.climate_mode,
                // A preview that exits; no window run asks for it.
                climate_map: None,
            };
            // Same helper the CLI uses. Anything read before this point (the world prep
            // above) has to apply the body rules on its own.
            crate::args::apply_body_defaults(&mut args);
            // Same as run_cli: caves carve into the filled ground, so they bring it with them.
            if args.caves {
                args.fillground = true;
            }
            // The window never runs validate_args, so the floor and ceiling
            // are checked here, then written into the pack the world got above.
            if let Err(e) = crate::y_bounds::check(&args).and_then(|()| {
                if is_new_world {
                    crate::y_bounds::patch_datapack(&world_path, &args)
                } else {
                    Ok(())
                }
            }) {
                emit_gui_error(&e);
                return Err(e);
            }
            if args.process.prewarm {
                let mut argv = piece_argv(&args, one_world.then_some(world_path.as_path()), &flags);
                argv.extend([
                    format!("--bbox={bbox_text}").into(),
                    "--progress=json".into(),
                ]);
                // A bake in the child runs on what a generation worker gets.
                if args.process.thread_count().is_none() {
                    let threads = crate::scale::budget::bake_threads(&args);
                    argv.push(format!("--threads={threads}").into());
                }
                return prewarm_in_child(&argv, &bbox_text);
            }

            let mut one_world_extending = false;
            if one_world {
                // Asked for before `prepare` settles the world's values, as
                // the user's own command line would ask.
                let pieces_argv = args
                    .units
                    .coordinates()
                    .then(|| piece_argv(&args, Some(&world_path), &flags));
                let session = match crate::one_world::prepare(&world_path, &bbox, &mut args) {
                    Ok(session) => session,
                    Err(e) => {
                        eprintln!("{e}");
                        emit_gui_error(&e);
                        emit_gui_progress_update(progress::MESSAGE_ONLY, &format!("Error! {e}"));
                        return Err(e);
                    }
                };
                bbox = session.llbbox;
                one_world_extending = args.one_world_run.as_ref().is_some_and(|r| r.extending);
                if session.created {
                    cleanup_guard = Some(NewWorldCleanup::new(world_path.clone()));
                }
                _session_lock = Some(session.lock);
                if args.disable_height_limit && session.created {
                    if let Err(e) = crate::world_utils::install_tall_datapack(&world_path) {
                        let error_msg = format!("Failed to install tall-world datapack: {e}");
                        eprintln!("{error_msg}");
                        emit_gui_error(&error_msg);
                        return Err(error_msg);
                    }
                }
                progress::emit_world_name_update(
                    world_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or(crate::one_world::DEFAULT_WORLD_NAME),
                );
                if let Some(argv) = pieces_argv {
                    // The coordinator places the spawn, as --spawn-lat/--spawn-lng.
                    (args.spawn_lat, args.spawn_lng) = spawn_point.unzip();
                    emit_gui_progress_update(progress::MESSAGE_ONLY, "Building pieces...");
                    if let Err(e) = crate::scale::run(&args, &world_path, &bbox, &argv) {
                        eprintln!("{e}");
                        if crate::scale::has_finished_pieces(&world_path) {
                            // Kept: generating the same area again resumes it.
                            if let Some(g) = cleanup_guard.as_mut() {
                                g.disarm();
                            }
                        }
                        emit_gui_error(&e);
                        return Err(e);
                    }
                    if let Some(g) = cleanup_guard.as_mut() {
                        g.disarm();
                    }
                    drop(_session_lock);
                    emit_gui_progress_update(100.0, "Done! World generation completed.");
                    println!("{}", "Done! World generation completed.".green().bold());
                    return Ok(());
                }
            }
            let args = args;
            // Same as run_cli: after a One World has applied the seed it
            // keeps, before anything rolls a die. Process-wide, so set every run.
            crate::deterministic_rng::set_world_seed(args.seed.unwrap_or(0));

            // Calculate MC spawn coordinates from lat/lng if spawn point was provided
            // Otherwise, default to X=1, Z=1 (relative to xzbbox min coordinates).
            // An extended One World keeps its spawn unless a marker was placed.
            let mc_spawn_point: Option<(i32, i32)> = if let Ok((transformer, pre_rot_bbox)) =
                crate::projection::ProjectionSpec::from_args(&args).transformer(&bbox)
            {
                let marker = spawn_point.and_then(|(lat, lng)| LLPoint::new(lat, lng).ok());
                match (marker, one_world_extending) {
                    (None, true) => None,
                    (marker, _) => {
                        let (sx, sz) = match marker {
                            Some(llpoint) => {
                                let xzpoint = transformer.transform_point(llpoint);
                                (xzpoint.x, xzpoint.z)
                            }
                            None => calculate_default_spawn(&pre_rot_bbox),
                        };
                        Some(map_transformation::rotate::rotate_xz_point(
                            sx,
                            sz,
                            args.rotation,
                            &pre_rot_bbox,
                        ))
                    }
                }
            } else {
                None
            };
            generation_options.spawn_point = mc_spawn_point;
            // Y is corrected after generation.
            if one_world && !one_world_extending {
                if let Some((sx, sz)) = mc_spawn_point {
                    if let Err(e) =
                        set_player_spawn_in_level_dat(&world_path.display().to_string(), sx, sz)
                    {
                        eprintln!("Warning: Failed to set spawn point: {e}");
                    }
                }
            }

            // Same as run_cli: the facade pipeline needs only the bbox, and its
            // downloads are the longest part of a run that uses it, so it starts
            // now and is collected just before the buildings.
            generation_options.facades = crate::mapillary::FacadeJob::start(&args, bbox);
            let generation_options = generation_options;

            // Same as run_cli: fix the dimension span before the editor is touched.
            crate::world_editor::set_world_bounds(
                ground::extended_min_y_for(&args),
                ground::world_top_y_for(&args),
            );

            // Ask Args, not the frontend flag: below OBJECT_SKIP_SCALE objects are skipped
            // regardless of the selected generation mode.
            if args.skip_objects() {
                // Generate ground data (terrain) for terrain-only mode
                let mut ground = ground::generate_ground_data(&args, bbox);
                offline_complete()?;
                // Matches run_cli.
                ground.mark_beaches();

                // Create empty parsed_elements and xzbbox for terrain-only mode
                let mut parsed_elements = Vec::new();
                let (_coord_transformer, mut xzbbox) =
                    crate::projection::ProjectionSpec::from_args(&args)
                        .transformer(&bbox)
                        .map_err(|e| format!("Failed to create coordinate transformer: {}", e))?;

                // The spawn point is rotated above, so skipping the world
                // rotation here would drop the player outside the terrain.
                map_transformation::transform_map(&mut parsed_elements, &mut xzbbox, &mut ground);

                if rotation_angle.abs() > f64::EPSILON {
                    map_transformation::rotate::rotate_world(
                        rotation_angle.clamp(-90.0, 90.0),
                        &mut parsed_elements,
                        &mut xzbbox,
                        &mut ground,
                    )
                    .map_err(|e| format!("Rotation failed: {e}"))?;
                }

                if let Err(e) = data_processing::generate_world_with_options(
                    parsed_elements,
                    xzbbox,
                    bbox,
                    ground,
                    &args,
                    generation_options.clone(),
                    osm_parser::OutlineSuppression::new(),
                    osm_parser::PartGroups::new(),
                ) {
                    emit_gui_error(&e);
                    return Err(e);
                }
                if let Some(g) = cleanup_guard.as_mut() {
                    g.disarm();
                }
                // Explicitly release session lock before showing Done message
                // so Minecraft can open the world immediately
                drop(_session_lock);
                emit_gui_progress_update(100.0, "Done! World generation completed.");
                println!("{}", "Done! World generation completed.".green().bold());

                return Ok(());
            }

            // OSM, Overture and elevation/land-cover fetches only need the bbox, run them in parallel
            let (fetch_result, overture_data, ground) = std::thread::scope(|s| {
                let overture_handle = s.spawn(|| {
                    if args.overture {
                        overture::fetch_overture_buildings(
                            &bbox,
                            &crate::projection::ProjectionSpec::from_args(&args),
                            args.overture_source,
                            args.debug,
                        )
                    } else {
                        overture::OvertureData::default()
                    }
                });
                let ground_handle = s.spawn(|| ground::generate_ground_data(&args, bbox));
                // A local file stands in for the download; the area is still
                // the selection, as with the CLI's --bbox.
                let fetch_result = match args.file.as_deref() {
                    Some(file) => retrieve_data::fetch_data_from_file(file).map(|(data, _)| data),
                    None => retrieve_data::fetch_osm_data(
                        bbox,
                        args.debug,
                        "requests",
                        None,
                        &args.osm_tiles_url,
                        !args.no_tile_archive,
                        crate::osm_pbf::Source::from_args(&args).as_ref(),
                    ),
                };
                // A panicked worker already reported itself through the panic hook.
                // Overture is supplementary, so drop it and keep going; terrain is
                // not, so hand the failure back instead of taking the app down.
                let overture_data = overture_handle.join().unwrap_or_else(|_| {
                    eprintln!("Overture fetch failed, continuing without Overture buildings.");
                    overture::OvertureData::default()
                });
                (fetch_result, overture_data, ground_handle.join().ok())
            });
            offline_complete()?;

            let Some(ground) = ground else {
                let error_msg = "Terrain fetch failed unexpectedly".to_string();
                eprintln!("{error_msg}");
                emit_gui_error(&error_msg);
                return Err(error_msg);
            };

            // Run world generation
            match fetch_result {
                Ok(raw_data) => {
                    let (mut parsed_elements, mut xzbbox, outline_suppression, part_groups) =
                        osm_parser::parse_osm_data(
                            raw_data,
                            bbox,
                            args.debug,
                            &crate::projection::ProjectionSpec::from_args(&args),
                        );

                    let overture::OvertureData {
                        elements: overture_elements,
                        hints: overture_hints,
                    } = overture_data;

                    // Fill height/levels on OSM buildings that have neither
                    overture_hints.apply(&mut parsed_elements);

                    // Merge supplementary Overture buildings against parsed OSM
                    if !overture_elements.is_empty() {
                        let unique_overture =
                            overture::deduplicate_against_osm(overture_elements, &parsed_elements);
                        parsed_elements.extend(unique_overture);
                    }

                    parsed_elements.sort_by(|el1, el2| {
                        let (el1_priority, el2_priority) =
                            (osm_parser::get_priority(el1), osm_parser::get_priority(el2));
                        match (
                            el1.tags().contains_key("landuse"),
                            el2.tags().contains_key("landuse"),
                        ) {
                            (true, false) => std::cmp::Ordering::Greater,
                            (false, true) => std::cmp::Ordering::Less,
                            _ => el1_priority.cmp(&el2_priority),
                        }
                    });

                    let mut ground = ground;

                    // OSM water override first, then bridge repair.
                    ground.apply_osm_water_override(&parsed_elements, &xzbbox);
                    ground.apply_osm_land_override(&parsed_elements, &xzbbox, args.scale);
                    ground.apply_bridge_land_cover_repair(&parsed_elements, &xzbbox, args.scale);
                    ground.mark_beaches();

                    // Transform map (parsed_elements). Operations are defined in a json file
                    map_transformation::transform_map(
                        &mut parsed_elements,
                        &mut xzbbox,
                        &mut ground,
                    );

                    // Apply rotation if specified
                    if rotation_angle.abs() > f64::EPSILON {
                        map_transformation::rotate::rotate_world(
                            rotation_angle.clamp(-90.0, 90.0),
                            &mut parsed_elements,
                            &mut xzbbox,
                            &mut ground,
                        )
                        .map_err(|e| format!("Rotation failed: {e}"))?;
                    }

                    if let Err(e) = data_processing::generate_world_with_options(
                        parsed_elements,
                        xzbbox,
                        bbox,
                        ground,
                        &args,
                        generation_options.clone(),
                        outline_suppression,
                        part_groups,
                    ) {
                        eprintln!("World generation failed: {e}");
                        send_log(LogLevel::Error, &format!("World generation failed: {e}"));
                        emit_gui_error(&e);
                        return Err(e);
                    }
                    if let Some(g) = cleanup_guard.as_mut() {
                        g.disarm();
                    }
                    // Explicitly release session lock before showing Done message
                    // so Minecraft can open the world immediately
                    drop(_session_lock);
                    emit_gui_progress_update(100.0, "Done! World generation completed.");
                    println!("{}", "Done! World generation completed.".green().bold());

                    Ok(())
                }
                Err(e) => {
                    emit_gui_error(&e.to_string());
                    // cleanup_guard removes the new world, and SessionLock releases
                    // its file handle first via reverse drop order.
                    Err(e.to_string())
                }
            }
        };
        // On `pool` when Extra Features asked for a thread count, else the global pool.
        let blocking = move || match pool {
            Some(pool) => pool.install(work),
            None => work(),
        };
        if let Err(e) = tokio::task::spawn_blocking(blocking).await {
            let error_msg = format!("Error in blocking task: {e}");
            eprintln!("{error_msg}");
            emit_gui_error(&error_msg);
            // Session lock will be automatically released when the task fails
        }
    });

    Ok(())
}

#[cfg(test)]
mod piece_tests {
    use super::{meld_args, piece_argv};
    use crate::args::Args;
    use clap::Parser;

    /// A piece run from the window must ask for what the window asked for:
    /// every setting the GUI fills in comes back from the rebuilt command line.
    #[test]
    fn the_pieces_command_line_parses_back_to_the_same_settings() {
        let dir = tempfile::tempdir().unwrap();
        let base = |extra: &[std::ffi::OsString]| {
            let mut argv: Vec<std::ffi::OsString> = vec![
                "arnis".into(),
                "--bbox".into(),
                "44.43,26.08,44.45,26.11".into(),
            ];
            argv.extend_from_slice(extra);
            Args::try_parse_from(argv).unwrap()
        };
        let mut args = base(&[
            "--one-world".into(),
            "--output-dir".into(),
            dir.path().into(),
            "--world-name".into(),
            "My World".into(),
        ]);
        // Off their CLI defaults, so a dropped field shows.
        args.scale = 0.37;
        args.height_multiplier = 1.5;
        args.ground_level = -40;
        args.mode = crate::args::GenerationMode::GeoOnly;
        args.interior = true;
        args.fillground = true;
        args.caves = true;
        args.legacy_trees = true;
        args.max_tree_size = crate::trees::tree_library::TreeSize::from_str_lossy("small");
        args.canopy_height = false;
        args.overture = false;
        args.use_3d = false;
        args.timeout = Some(std::time::Duration::from_secs(40));
        args.disable_height_limit = true;
        args.aws_only_elevation = true;
        args.bake_lighting = true;
        args.voxy_lod = true;
        args.map_preview = true;
        args.map_item = false;
        args.gamemode = crate::args::GameMode::from_str_lossy("survival");
        args.world_time = 18000;
        args.world_type = crate::args::WorldType::from_str_lossy("flat");
        args.signage = crate::args::SignageLevel::from_str_lossy("none");
        args.mapillary_facades = Some(false);
        args.mapillary_token = Some("MLY|1|x".into());
        args.mapillary_facade_mode = crate::args::FacadeMode::from_str_lossy("blocks");
        args.building_facades = true;
        args.facade_detail = crate::args::FacadeDetail::from_str_lossy("high");
        // The Advanced Features fields, set the way the window sets them.
        let flags = [
            "--snow-mode=manual",
            "--snow-y=150",
            "--road-detail=compact",
            "--rocks",
            "--rock-density=0.07",
            "--bushes",
            "--bush-density=0.13",
            "--no-buildings",
            "--loot-table=my loot.json",
            "--field-mix=prairie",
            "--farm-crops=wheat=60,sunflower=20,fallow=20",
            "--field-scale=175",
            "--grass-texture",
            "--grass-mix=plains=3,flower=1",
            "--land-texture",
            "--land-mix=prairie",
            "--tree-realm=eur",
            "--tree-size-weights=small=50,tall=150,giant=0",
            "--cave-seed=12345",
            "--cave-datum-y=-128",
            "--river-bed=v1",
            "--water-detail=scaled",
            "--climate-mode=per-position",
            "--seed=42",
            "--osm-tiles-url=https://tiles.example/v1",
            "--no-tile-archive",
            "--overpass-url=http://a/api,http://b/api",
            "--offline",
            "--file=area.osm",
            "--props=car,windturbine",
            "--props-min-scale=0.5",
            "--threads=6",
            "--ram-budget-mb=4000",
            "--max-downloads=8",
            "--one-world-workers=auto",
            "--unit-regions=2",
            "--world-border",
        ]
        .map(String::from);
        let meld = meld_args(&flags, true).unwrap();
        args.snow = meld.snow;
        args.road_detail = meld.road_detail;
        args.scatter = meld.scatter;
        args.buildings = meld.buildings;
        args.loot_table = meld.loot_table;
        args.fields = meld.fields;
        args.tree_realm = meld.tree_realm;
        args.tree_size_weights = meld.tree_size_weights;
        args.tree_pack_dir = meld.tree_pack_dir;
        args.tree_pack_mode = meld.tree_pack_mode;
        args.cave_seed = meld.cave_seed;
        args.cave_datum_y = meld.cave_datum_y;
        args.water = meld.water;
        args.climate_mode = meld.climate_mode;
        args.seed = meld.seed;
        args.osm_tiles_url = meld.osm_tiles_url;
        args.no_tile_archive = meld.no_tile_archive;
        args.file = meld.file;
        args.props = meld.props;
        args.props_min_scale = meld.props_min_scale;
        args.process = meld.process;
        args.units = meld.units;
        args.world_border = meld.world_border;
        let back = base(&piece_argv(
            &args,
            Some(&dir.path().join("My World")),
            &flags,
        ));
        assert_eq!(format!("{back:?}"), format!("{args:?}"));
    }

    /// The window's offline download runs the CLI on this line, so it must
    /// pass the CLI's own checks, without a world to write.
    #[test]
    fn a_prewarm_line_passes_the_cli_checks() {
        let args = Args::try_parse_from(["arnis", "--bbox", "44.43,26.08,44.45,26.11"]).unwrap();
        let flags = ["--no-tile-archive", "--prewarm"].map(String::from);
        let mut argv: Vec<std::ffi::OsString> = vec!["arnis".into()];
        argv.extend(piece_argv(&args, None, &flags));
        argv.push("--bbox=44.43 26.08 44.45 26.11".into());
        let back = Args::try_parse_from(argv).unwrap();
        assert!(back.process.prewarm && back.no_tile_archive && !back.one_world);
        crate::args::validate_args(&back).unwrap();
    }

    /// No Meld flag is the stock run, and the window refuses what the CLI does.
    #[test]
    fn meld_settings_follow_the_cli() {
        use clap::Parser;
        let stock = Args::parse_from(["arnis"]);
        assert_eq!(
            format!("{:?}", meld_args(&[], true).unwrap()),
            format!("{stock:?}")
        );
        let refused = |flags: &[&str], one_world| {
            meld_args(
                &flags.iter().map(|f| f.to_string()).collect::<Vec<_>>(),
                one_world,
            )
            .is_err()
        };
        assert!(refused(&["--cave-datum-y=100"], false));
        assert!(refused(&["--farm-crops=wheat=x"], false));
        assert!(refused(&["--tree-realm=mars"], false));
        assert!(refused(&["--field-scale=500"], false));
    }
}

#[cfg(test)]
mod fit_area_name_tests {
    use super::fit_area_name;

    #[test]
    fn short_base_keeps_or_truncates_area() {
        assert_eq!(
            fit_area_name("Arnis World 1", "Berlin".into()).as_deref(),
            Some("Berlin")
        );
        // 30 - 13 - 2 = 15 characters of room
        assert_eq!(
            fit_area_name("Arnis World 1", "Charlottenburg-Wilmersdorf".into()).as_deref(),
            Some("Charlottenburg-")
        );
    }

    #[test]
    fn long_base_leaves_name_alone_instead_of_underflowing() {
        assert_eq!(
            fit_area_name("Arnis World 1 of my home town", "X".into()),
            None
        );
        assert_eq!(
            fit_area_name("Arnis World Downtown Berlin 2026", "X".into()),
            None
        );
    }

    #[test]
    fn counts_characters_not_bytes() {
        // 16 characters but 48 bytes; there is still room for 12 characters.
        let out = fit_area_name(
            "Arnis World 東京東京",
            "大阪大阪大阪大阪大阪大阪大阪".into(),
        );
        assert_eq!(out.map(|s| s.chars().count()), Some(12));
    }
}

#[cfg(test)]
mod generation_slot_tests {
    use super::{BusySlot, BUSY_CLEAR, BUSY_GENERATION, BUSY_PRECOMPUTE};

    /// The slot is one process global, so two tests taking it at once would
    /// each see the other's claim and fail for no reason of their own.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The wipe holds the slot for its whole length, so a generation cannot
    /// start reading the caches while the files are still going.
    #[test]
    fn a_cache_wipe_and_a_generation_exclude_each_other() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());

        let clearing = BusySlot::acquire(BUSY_CLEAR).expect("the wipe gets the slot");
        let refused = BusySlot::acquire(BUSY_GENERATION).expect_err("the generation waits");
        assert!(refused.contains("cleared"), "{refused}");
        drop(clearing);

        let generating = BusySlot::acquire(BUSY_GENERATION).expect("the generation gets it");
        let refused = BusySlot::acquire(BUSY_CLEAR).expect_err("the wipe waits");
        assert!(refused.contains("generation"), "{refused}");
        drop(generating);
    }

    #[test]
    fn second_generation_is_refused_until_the_first_finishes() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // The world floor, terrain floor and filler base are process globals, so a second
        // concurrent run would retune them under the first one's feet.
        let first = BusySlot::acquire(BUSY_GENERATION).expect("the first generation gets the slot");
        assert!(
            BusySlot::acquire(BUSY_GENERATION).is_err(),
            "a second generation must be refused while the first holds the slot"
        );
        drop(first);
        assert!(
            BusySlot::acquire(BUSY_GENERATION).is_ok(),
            "the slot must be free again once the first generation finishes"
        );
    }

    /// The two jobs exclude each other, and each is told which one is in the
    /// way: a precompute refused with "a generation is running" would send the
    /// user looking for a generation they had already finished.
    #[test]
    fn a_precompute_and_a_generation_exclude_each_other_and_say_which() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let generating = BusySlot::acquire(BUSY_GENERATION).expect("the generation gets the slot");
        let refused = BusySlot::acquire(BUSY_PRECOMPUTE).expect_err("the precompute is refused");
        assert!(refused.contains("generation"), "{refused}");
        drop(generating);

        let precomputing = BusySlot::acquire(BUSY_PRECOMPUTE).expect("the precompute gets it");
        let refused = BusySlot::acquire(BUSY_GENERATION).expect_err("the generation is refused");
        assert!(refused.contains("precompute"), "{refused}");
        assert!(
            BusySlot::acquire(BUSY_PRECOMPUTE).is_err(),
            "and so is a second precompute, which is the double click"
        );
        drop(precomputing);
        assert!(BusySlot::acquire(BUSY_GENERATION).is_ok());
    }
}

#[cfg(test)]
mod locale_tests {
    use std::collections::BTreeSet;

    // en-US is the source of truth, every other locale must match its key set
    const LOCALES: &[(&str, &str)] = &[
        ("en-US", include_str!("gui/locales/en-US.json")),
        ("en", include_str!("gui/locales/en.json")),
        ("ar", include_str!("gui/locales/ar.json")),
        ("de", include_str!("gui/locales/de.json")),
        ("es", include_str!("gui/locales/es.json")),
        ("fi", include_str!("gui/locales/fi.json")),
        ("fr-FR", include_str!("gui/locales/fr-FR.json")),
        ("hu", include_str!("gui/locales/hu.json")),
        ("ja", include_str!("gui/locales/ja.json")),
        ("ka-GE", include_str!("gui/locales/ka-GE.json")),
        ("ko", include_str!("gui/locales/ko.json")),
        ("lt", include_str!("gui/locales/lt.json")),
        ("lv", include_str!("gui/locales/lv.json")),
        ("pl", include_str!("gui/locales/pl.json")),
        ("pt-BR", include_str!("gui/locales/pt-BR.json")),
        ("ru", include_str!("gui/locales/ru.json")),
        ("sl", include_str!("gui/locales/sl.json")),
        ("sv", include_str!("gui/locales/sv.json")),
        ("ua", include_str!("gui/locales/ua.json")),
        ("zh-CN", include_str!("gui/locales/zh-CN.json")),
    ];

    fn locale_keys(name: &str, raw: &str) -> BTreeSet<String> {
        let value: serde_json::Value = serde_json::from_str(raw)
            .unwrap_or_else(|e| panic!("{name}.json is invalid JSON: {e}"));
        value
            .as_object()
            .unwrap_or_else(|| panic!("{name}.json is not a JSON object"))
            .keys()
            .cloned()
            .collect()
    }

    #[test]
    fn locales_match_en_us_keys() {
        let reference = locale_keys(LOCALES[0].0, LOCALES[0].1);
        let mut errors = Vec::new();
        for (name, raw) in &LOCALES[1..] {
            let keys = locale_keys(name, raw);
            let missing: Vec<_> = reference.difference(&keys).cloned().collect();
            let extra: Vec<_> = keys.difference(&reference).cloned().collect();
            if !missing.is_empty() {
                errors.push(format!(
                    "{name}.json is missing keys: {}",
                    missing.join(", ")
                ));
            }
            if !extra.is_empty() {
                errors.push(format!(
                    "{name}.json has keys not in en-US.json: {}",
                    extra.join(", ")
                ));
            }
        }
        assert!(
            errors.is_empty(),
            "Locale key mismatches:\n{}",
            errors.join("\n")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_level_dat(world: &Path) -> Value {
        let raw = fs::read(world.join("level.dat")).unwrap();
        let mut buf = Vec::new();
        GzDecoder::new(raw.as_slice())
            .read_to_end(&mut buf)
            .unwrap();
        fastnbt::from_bytes(&buf).unwrap()
    }

    fn write_level_dat(world: &Path, root: &Value) {
        let bytes = fastnbt::to_bytes(root).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&bytes).unwrap();
        fs::write(world.join("level.dat"), encoder.finish().unwrap()).unwrap();
    }

    /// The readout's numbers: regions are cells times the cell size, pieces
    /// are cells, and kilometres are blocks over the scale.
    #[test]
    fn the_selection_readout_counts_regions_cells_and_kilometres() {
        let tmp = tempfile::tempdir().unwrap();
        let save = tmp.path().to_string_lossy().to_string();
        let bbox = "44.43 26.0 44.46 26.15".to_string();
        for (scale, n) in [(1.0, 4), (0.5, 2)] {
            for (mode, square) in [("fit", false), ("cover", false), ("fit", true)] {
                let snap = gui_snap_selection(
                    bbox.clone(),
                    save.clone(),
                    Some(String::new()),
                    scale,
                    n,
                    Some(mode.to_string()),
                    Some(square),
                    None,
                )
                .unwrap();
                let what = format!("scale {scale} n {n} {mode} square {square}");
                assert!(snap.new_world, "{what}");
                assert_eq!(snap.regions[0], snap.cells[0] * n, "{what}");
                assert_eq!(snap.regions[1], snap.cells[1] * n, "{what}");
                for i in 0..2 {
                    let km = f64::from(snap.regions[i]) * 512.0 / scale / 1000.0;
                    assert!((snap.size_km[i] - km).abs() < 1e-9, "{what}");
                }
                if square {
                    assert_eq!(snap.cells[0], snap.cells[1], "{what}");
                } else {
                    // About 12 km by 3.3 km: wider than tall.
                    assert!(snap.cells[0] > snap.cells[1], "{what}");
                }
            }
        }
        // Nothing was created: the snap only reads.
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
    }

    /// A large selection without One World becomes a new One World: the snap
    /// names it as a new world is named, and sizes its workers from the flags.
    #[test]
    fn an_unnamed_snap_is_for_the_next_new_world() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("Arnis World 1")).unwrap();
        let save = tmp.path().to_string_lossy().to_string();
        let snap = |bbox: &str, workers: &str| {
            gui_snap_selection(
                bbox.to_string(),
                save.clone(),
                None,
                1.0,
                2,
                None,
                None,
                Some(vec![format!("--one-world-workers={workers}")]),
            )
            .unwrap()
        };
        let large = snap("44.43 26.0 44.46 26.15", "2");
        assert_eq!(large.world_name, "Arnis World 2");
        assert!(large.new_world);
        assert!(large.cells[0] * large.cells[1] > 2);
        assert_eq!(large.workers, 2);
        // Never more workers than pieces.
        let small = snap("44.430 26.000 44.431 26.001", "6");
        assert_eq!(small.cells, [1, 1]);
        assert_eq!(small.workers, 1);
        // Flags that do not parse still give a snap.
        let odd = snap("44.43 26.0 44.46 26.15", "many");
        assert_eq!(odd.cells, large.cells);
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn the_player_starts_on_the_world_spawn_column() {
        let tmp = tempfile::tempdir().unwrap();
        let world = PathBuf::from(crate::world_utils::create_new_world(tmp.path()).unwrap());

        let mut root = read_level_dat(&world);
        let Value::Compound(ref mut map) = root else {
            panic!("root not a compound")
        };
        let Some(Value::Compound(data)) = map.get_mut("Data") else {
            panic!("missing Data")
        };
        data.insert("SpawnX".to_string(), Value::Int(120));
        data.insert("SpawnZ".to_string(), Value::Int(-340));
        write_level_dat(&world, &root);

        let xzbbox = XZBBox::rect_from_min_max(0, 0, 511, 511).unwrap();
        update_player_spawn_y_after_generation(&world, &xzbbox, &Ground::new_flat(-62)).unwrap();

        let root = read_level_dat(&world);
        let Value::Compound(map) = root else {
            panic!("root not a compound")
        };
        let Some(Value::Compound(data)) = map.get("Data") else {
            panic!("missing Data")
        };
        assert_eq!(data.get("SpawnY"), Some(&Value::Int(-61)));
        let Some(Value::Compound(player)) = data.get("Player") else {
            panic!("missing Player")
        };
        let Some(Value::List(pos)) = player.get("Pos") else {
            panic!("missing Pos")
        };
        assert_eq!(
            pos.as_slice(),
            [
                Value::Double(120.0),
                Value::Double(-61.0),
                Value::Double(-340.0)
            ]
        );
    }
}
