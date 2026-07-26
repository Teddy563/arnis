#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod args;
mod bedrock_block_map;
mod bench;
mod biome;
mod block_definitions;
mod bresenham;
mod caves;
mod climate;
mod climate_map;
mod clipping;
mod colors;
mod coordinate_system;
mod data_processing;
mod deepslate;
mod deterministic_rng;
mod element_processing;
mod elevation;
mod elevation_data;
mod elevation_map;
mod floodfill;
mod floodfill_cache;
mod ground;
mod ground_generation;
mod land_cover;
mod land_cover_bridge_repair;
mod land_cover_osm_water_override;
mod luanti_block_map;
mod map_item;
mod map_item_palette;
mod map_preview;
mod map_renderer;
mod map_transformation;
mod models_3d;
mod osm_parser;
mod overture;
#[cfg(feature = "gui")]
mod progress;
mod projection;
mod lowland;
mod region;
mod retrieve_data;
mod road_bearings;
mod schematic;
mod structures;
#[cfg(feature = "gui")]
mod telemetry;
#[cfg(test)]
mod test_utilities;
mod tile;
mod tree_library;
mod version_check;
mod water_depth;
mod world_editor;
mod world_utils;

use args::Args;
use clap::Parser;
use colored::*;
use std::path::PathBuf;
use std::{env, fs, io::Write};

// mimalloc scales far better than the system allocator under the concurrent
// 4 KiB section-vec / hashmap churn of tile-parallel processing.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "gui")]
mod gui;

// If the user does not want the GUI, it's easiest to just mock the progress module to do nothing
#[cfg(not(feature = "gui"))]
mod progress {
    pub fn emit_gui_error(_message: &str) {}
    pub fn emit_gui_progress_update(_progress: f64, _message: &str) {}
    pub fn emit_gui_progress_update_ex(_progress: f64, _message: &str, _streaming: bool) {}
    pub fn emit_map_preview_ready() {}
    pub fn emit_show_in_folder(_path: &str) {}
    pub fn is_running_with_gui() -> bool {
        false
    }
}
#[cfg(target_os = "windows")]
use windows::Win32::System::Console::{AttachConsole, FreeConsole, ATTACH_PARENT_PROCESS};

fn run_cli() {
    // Configure thread pool with 90% CPU cap to keep system responsive
    floodfill_cache::configure_rayon_thread_pool(0.9);

    let version: &str = env!("CARGO_PKG_VERSION");
    let repository: &str = env!("CARGO_PKG_REPOSITORY");
    println!(
        r#"
        ▄████████    ▄████████ ███▄▄▄▄    ▄█     ▄████████
        ███    ███   ███    ███ ███▀▀▀██▄ ███    ███    ███
        ███    ███   ███    ███ ███   ███ ███▌   ███    █▀
        ███    ███  ▄███▄▄▄▄██▀ ███   ███ ███▌   ███
      ▀███████████ ▀▀███▀▀▀▀▀   ███   ███ ███▌ ▀███████████
        ███    ███ ▀███████████ ███   ███ ███           ███
        ███    ███   ███    ███ ███   ███ ███     ▄█    ███
        ███    █▀    ███    ███  ▀█   █▀  █▀    ▄████████▀
                     ███    ███

                          version {}
                {}
        "#,
        version,
        repository.bright_white().bold()
    );

    // Fire-and-forget update check; prints a one-line notice on a background thread.
    version_check::check_for_updates_async();

    // Parse input arguments
    let mut args: Args = Args::parse();

    // --caves needs solid host rock to carve into.
    if args.caves {
        args.fillground = true;
    }

    // Hard offline mode for elevation: signal the elevation providers (which run
    // deep in the call stack and don't see `args`) via a process-wide env var.
    // When the flag is absent, this is never set and every offline gate is a no-op.
    if args.offline {
        std::env::set_var("ARNIS_OFFLINE", "1");
    }
    // Regional-only elevation: forbid AWS Terrain Tiles anywhere in the elevation path,
    // including the PER-TILE AWS Terrarium recovery deep inside fetch_fixed_tile_grid
    // (which the outer provider select can't see). Same process-wide env-var pattern as
    // offline mode. Absent = never set, every AWS-no gate is a no-op.
    if args.regional_elevation_only {
        std::env::set_var("ARNIS_NO_AWS", "1");
    }

    // Map-item-only mode: add the world map item to an EXISTING Java world at --output-dir and
    // exit, with no generation. Meld runs this once over a fully merged master world; the map
    // footprint is read from the saved region files, so no OSM re-parse or master-origin is needed.
    if args.map_item_only {
        let Some(world_path) = args.path.clone() else {
            eprintln!(
                "{}: --map-item-only requires --output-dir pointing at an existing Java world",
                "Error".red().bold()
            );
            std::process::exit(1);
        };
        match map_item::write_map_item_for_existing_world(&world_path) {
            Ok(()) => {
                println!("World map item written to {}", world_path.display());
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{}: {}", "Error".red().bold(), e);
                std::process::exit(1);
            }
        }
    }

    // Age out stale cached elevation tiles (best-effort; throttled to once/day + swept on a
    // background thread inside). Skipped entirely in Meld tile-mode (master-origin set) and the
    // download-only / terrain-only fast paths — Meld manages the shared cache and spawns this
    // per cell, so even the throttle-stat is pointless N times per run.
    if args.master_origin_lat.is_none() && !args.download_only && !args.download_terrain_only {
        elevation_data::cleanup_old_cached_tiles();
    }

    // Validate arguments (path requirements differ between Java and Bedrock)
    if let Err(e) = args::validate_args(&args) {
        eprintln!("{}: {}", "Error".red().bold(), e);
        std::process::exit(1);
    }

    // Dump the built-in default loot table to JSON and exit. Meld's web loot
    // editor calls this once to seed its default + "reset to default".
    if let Some(p) = &args.dump_loot_table {
        crate::element_processing::subprocessor::buildings_loot::dump_default_loot_table(p);
        std::process::exit(0);
    }

    // Load a custom chest loot table if one was supplied, BEFORE any (parallel)
    // cell generation so the shared config is set before worker threads read it.
    // A malformed file warns and keeps the built-in default (never aborts a run).
    if let Some(p) = &args.loot_table {
        crate::element_processing::subprocessor::buildings_loot::init_loot_from_path(p);
    }

    // Download-only mode: fetch OSM to --save-json-file and exit, before any world
    // creation. Lets Meld pre-fetch a whole region's OSM once and feed it to every
    // cell via --file (one serial request instead of N parallel ones that rate-limit).
    if args.download_only {
        let out = args
            .save_json_file
            .as_deref()
            .expect("validate_args guarantees --save-json-file is set with --download-only");
        match retrieve_data::fetch_data_from_overpass(
            args.bbox,
            args.debug,
            args.downloader.as_str(),
            Some(out),
            &args.overpass_url,
            &args.road_detail,
        ) {
            Ok(_) => {
                println!("Download-only: OSM data saved to {out}");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{}: {}", "Error".red().bold(), e);
                std::process::exit(1);
            }
        }
    }

    // Terrain-only mode: warm the AWS elevation tile cache for --bbox in ONE process
    // (8 concurrent) and exit, before any world creation. Lets Meld pre-warm a region's
    // terrain serially so the later parallel cells hit the cache instead of bursting S3
    // with ~64 concurrent requests (which rate-limits and truncates tiles -> flat seams).
    if args.download_terrain_only {
        // AWS tiles are the global base; skip them only when the user hard-disabled AWS.
        let (aws_ok, aws_fail) = if args.regional_elevation_only {
            (0usize, 0usize)
        } else {
            match elevation::providers::aws_terrain::prefetch_tiles(&args.bbox) {
                Ok((ok, fail)) => (ok, fail),
                Err(e) => {
                    eprintln!("{}: {}", "Error".red().bold(), e);
                    std::process::exit(1);
                }
            }
        };
        println!("Terrain prefetch: {aws_ok} tile(s) cached, {aws_fail} failed.");
        // Also warm the regional high-res provider generation will select (IGN, USGS,
        // GSI...): one sequential pass fills its fixed-tile disk cache, so the later
        // PARALLEL cells read from disk instead of rate-limiting the provider live --
        // which used to make every cell silently fall back to AWS's broken tiles.
        let mut regional_failed = false;
        if !args.aws_only_elevation {
            match elevation::prefetch_regional(&args.bbox, args.scale) {
                Ok(Some(name)) => println!("Regional elevation warm: '{name}' cached."),
                Ok(None) => {}
                Err(e) => {
                    regional_failed = true;
                    eprintln!("Regional elevation warm failed: {e}");
                }
            }
        }
        let fail = aws_fail > 0 || (regional_failed && args.regional_elevation_only);
        std::process::exit(if fail { 2 } else { 0 });
    }

    // Cave zone-map mode: render the cave BIOME ZONE layout for --bbox to PNG overlays and
    // exit, before any world creation. Uses the exact zone picker + seed + --cave-biomes
    // multipliers the real --caves carve would use, so the preview matches the world.
    if args.cave_zone_map.is_some() {
        if let Some(spec) = &args.cave_biomes {
            match caves::decoration::BiomeAmounts::parse(spec) {
                Ok(a) => caves::decoration::set_biome_amounts(a),
                Err(e) => {
                    eprintln!("{}: --cave-biomes: {}", "Error".red().bold(), e);
                    std::process::exit(1);
                }
            }
        }
        match caves::zone_map::render(&args) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("{}: {}", "Error".red().bold(), e);
                std::process::exit(1);
            }
        }
    }

    // Climate-map mode: render the Koppen CLIMATE layout for --bbox to a PNG overlay and exit,
    // before any world creation. Uses the same grouped Climate the real generation applies, so
    // the preview matches the biome tint / arid-polar surfaces the world will get.
    if args.climate_map.is_some() {
        match climate_map::render(&args) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("{}: {}", "Error".red().bold(), e);
                std::process::exit(1);
            }
        }
    }

    // Elevation-map mode: render the heightmap for --bbox to a PNG overlay and exit, using the
    // real provider stack generation uses, so the preview matches the world's terrain.
    if args.elevation_map.is_some() {
        match elevation_map::render(&args) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("{}: {}", "Error".red().bold(), e);
                std::process::exit(1);
            }
        }
    }

    // Overture pre-warm mode: fetch + cache the Overture building byte-ranges overlapping --bbox in
    // ONE process and exit, before any world creation. Lets Meld warm a region's buildings up front
    // (a data-pack-style download) so the later parallel cells read ranges from disk instead of each
    // doing a cold fetch. Reuses the exact range cache the cells read, so it is always compatible.
    if args.prewarm_overture {
        println!(
            "{} Pre-warming Overture building cache for bbox…",
            "  [+]".bold()
        );
        let n = overture::fetch_overture_buildings(
            &args.bbox,
            args.scale,
            args.debug,
            args.tile_invariant_rendering,
        )
        .len();
        println!("Overture prewarm: ranges cached for bbox ({n} buildings in range)");
        std::process::exit(0);
    }

    // Elevation pre-warm mode: fetch + cache the elevation tiles overlapping --bbox in ONE process
    // and exit, before any world creation. Fills the exact per-tile disk cache the generation cells
    // read (Mapterhorn globally, a regional provider where one covers, or AWS when forced), so the
    // later parallel cells hit disk instead of rate-limiting the tile server.
    if args.prewarm_elevation {
        println!(
            "{} Pre-warming elevation tile cache for bbox…",
            "  [+]".bold()
        );
        match elevation::prefetch_elevation(&args.bbox, args.scale, args.aws_only_elevation) {
            Ok((name, cached, absent, failed)) => {
                println!(
                    "Elevation prewarm: provider '{name}', {cached} tile(s) cached, \
                     {absent} ocean/absent, {failed} failed."
                );
                std::process::exit(if failed > 0 { 2 } else { 0 });
            }
            Err(e) => {
                eprintln!("{}: {}", "Error".red().bold(), e);
                std::process::exit(1);
            }
        }
    }

    // Heads-up for very large areas: generation is long and memory-heavy, and big
    // requests load the public OpenStreetMap / elevation servers. Non-blocking.
    // Placed after the Meld download-only / terrain-only early-exits so a prefetch
    // call never trips the warning.
    {
        const MAX_RECOMMENDED_AREA_KM2: f64 = 250.0;
        let b = &args.bbox;
        let mid_lat = ((b.min().lat() + b.max().lat()) / 2.0).to_radians();
        let width_m = (b.max().lng() - b.min().lng()) * 111_320.0 * mid_lat.cos();
        let height_m = (b.max().lat() - b.min().lat()) * 111_320.0;
        let area_km2 = (width_m * height_m).abs() / 1_000_000.0;
        if area_km2 > MAX_RECOMMENDED_AREA_KM2 {
            eprintln!(
                "{} Large area selected (~{:.0} km²). Generation may take a long time and \
                 use many GB of memory, and places heavy load on public OpenStreetMap and \
                 elevation servers. Use a smaller area if this was unintended.",
                "Note:".yellow().bold(),
                area_km2
            );
        }
    }

    // Determine world format and output path
    let world_format = if args.bedrock {
        world_editor::WorldFormat::BedrockMcWorld
    } else if args.luanti {
        world_editor::WorldFormat::LuantiWorld
    } else {
        world_editor::WorldFormat::JavaAnvil
    };

    // Build the generation output path and level name
    let (generation_path, level_name) = if args.bedrock {
        // Bedrock: generate .mcworld file in user-specified path or Desktop
        let output_dir = args
            .path
            .clone()
            .unwrap_or_else(world_utils::get_bedrock_output_directory);
        let (output_path, lvl_name) = world_utils::build_bedrock_output(&args.bbox, output_dir);
        (output_path, Some(lvl_name))
    } else if args.luanti {
        let base_dir = args
            .path
            .clone()
            .unwrap_or_else(world_utils::get_luanti_worlds_directory);
        let _ = std::fs::create_dir_all(&base_dir);
        let mut counter = 1;
        let world_name = loop {
            let candidate = format!("Arnis Luanti World {counter}");
            if !base_dir.join(&candidate).exists() {
                break candidate;
            }
            counter += 1;
        };
        let world_path = base_dir.join(&world_name);
        println!(
            "Creating Luanti world at: {}",
            world_path.display().to_string().bright_white().bold()
        );
        (world_path, Some(world_name))
    } else {
        // Java: the CLI's --output-dir/--path IS the intended world folder (caller owns naming/
        // versioning, e.g. Meld passing a version-named dir) — write region/ directly into it
        // instead of nesting an auto-numbered "Arnis World N" subfolder (that's the GUI's job).
        let base_dir = args.path.clone().unwrap();
        let world_path = match world_utils::create_world_at(&base_dir) {
            Ok(path) => PathBuf::from(path),
            Err(e) => {
                eprintln!("{} {}", "Error:".red().bold(), e);
                std::process::exit(1);
            }
        };
        println!(
            "Created new world at: {}",
            world_path.display().to_string().bright_white().bold()
        );
        if args.disable_height_limit {
            if let Err(e) = world_utils::install_tall_datapack(&world_path) {
                eprintln!(
                    "{} Failed to install tall-world datapack: {}",
                    "Error:".red().bold(),
                    e
                );
                std::process::exit(1);
            }
            eprintln!(
                "Note: tall-world datapack installed (requires Minecraft 1.21.4+). \
                 First load will prompt 'Experimental Features'; world can't be uploaded to Realms."
            );
        }
        (world_path, None)
    };

    // Top-level phase timer (active only under --benchmark). generate_world has
    // its own internal Bench for the block-placement phases.
    let mut bench = bench::Bench::new(args.benchmark);

    // Fetch data
    let raw_data = match (&args.osm_tile_dir, &args.file) {
        // Meld grid path: read the cell's slippy tiles straight from the cache dir,
        // no pre-merged clump file required.
        (Some(dir), _) => osm_parser::OsmData::from_tile_dir(dir, args.bbox, args.osm_tile_z),
        (None, Some(file)) => retrieve_data::fetch_data_from_file(file),
        (None, None) => retrieve_data::fetch_data_from_overpass(
            args.bbox,
            args.debug,
            args.downloader.as_str(),
            args.save_json_file.as_deref(),
            &args.overpass_url,
            &args.road_detail,
        ),
    }
    .expect("Failed to fetch data");
    bench.mark("osm_fetch");

    let mut ground = ground::generate_ground_data(&args);
    bench.mark("terrain_total");

    // Parse raw data
    let (mut parsed_elements, mut xzbbox, outline_suppression, part_groups) =
        osm_parser::parse_osm_data(
            raw_data,
            args.bbox,
            args.scale,
            args.debug,
            args.master_origin_lat,
            args.master_origin_lng,
            args.tile_invariant_rendering,
        ); /* Option<u64> already */
    bench.mark("parse_osm");

    // Fetch supplementary building data from Overture Maps — ONLY when buildings are enabled.
    // This is a per-run network fetch (STAC index + partition reads) and, measured, the single
    // dominant per-cell cost (~93% of a cell's wall time). With --no-buildings the buildings are
    // discarded anyway, so fetching them is pure waste: skip it entirely.
    if args.buildings {
        println!("{} Fetching Overture Maps data...", "  [+]".bold());
        let overture_elements = overture::fetch_overture_buildings(
            &args.bbox,
            args.scale,
            args.debug,
            args.tile_invariant_rendering,
        );
        bench.mark("overture_fetch");
        if !overture_elements.is_empty() {
            let before_count = parsed_elements.len();
            let unique_overture =
                overture::deduplicate_against_osm(overture_elements, &parsed_elements);
            parsed_elements.extend(unique_overture);
            let added = parsed_elements.len() - before_count;
            println!(
                "  Added {} buildings from Overture Maps",
                added.to_string().bright_white().bold()
            );
        } else {
            println!("  No additional buildings from Overture Maps for this area");
        }
    }

    parsed_elements
        .sort_by_key(|element: &osm_parser::ProcessedElement| osm_parser::get_priority(element));
    bench.mark("sort_priority");

    // OSM water override first, then bridge repair handles remaining bridge-shadow cells.
    ground.apply_osm_water_override(&parsed_elements, &xzbbox);
    if args.debug {
        ground.save_land_cover_debug_image("landcover_debug_post_osm_water");
    }
    ground.apply_bridge_land_cover_repair(&parsed_elements, &xzbbox, args.scale);
    if args.debug {
        ground.save_land_cover_debug_image("landcover_debug_post_bridge_repair");
    }
    bench.mark("landcover_osm_repair");

    // Write the parsed OSM data to a file for inspection
    if args.debug {
        let mut buf = std::io::BufWriter::new(
            fs::File::create("parsed_osm_data.txt").expect("Failed to create output file"),
        );
        for element in &parsed_elements {
            writeln!(
                buf,
                "Element ID: {}, Type: {}, Tags: {:?}",
                element.id(),
                element.kind(),
                element.tags(),
            )
            .expect("Failed to write to output file");
        }
    }

    // Transform map (parsed_elements). Operations are defined in a json file
    map_transformation::transform_map(&mut parsed_elements, &mut xzbbox, &mut ground);
    bench.mark("transform_map");

    // Apply rotation if specified
    if args.rotation.abs() > f64::EPSILON {
        if let Err(e) = map_transformation::rotate::rotate_world(
            args.rotation,
            &mut parsed_elements,
            &mut xzbbox,
            &mut ground,
        ) {
            eprintln!("{} Rotation failed: {}", "Error:".red().bold(), e);
            std::process::exit(1);
        }
    }

    // Convert spawn lat/lng to Minecraft XZ coordinates if provided
    let spawn_point: Option<(i32, i32)> = match (args.spawn_lat, args.spawn_lng) {
        (Some(lat), Some(lng)) => {
            use coordinate_system::geographic::LLPoint;
            use coordinate_system::transformation::CoordTransformer;

            let llpoint = LLPoint::new(lat, lng).unwrap_or_else(|e| {
                eprintln!("{} Invalid spawn coordinates: {}", "Error:".red().bold(), e);
                std::process::exit(1);
            });

            let (transformer, pre_rot_bbox) = CoordTransformer::llbbox_to_xzbbox(
                &args.bbox,
                args.scale,
                args.master_origin_lat,
                args.master_origin_lng,
            )
            .unwrap_or_else(|e| {
                eprintln!(
                    "{} Failed to convert spawn point: {}",
                    "Error:".red().bold(),
                    e
                );
                std::process::exit(1);
            });

            let xzpoint = transformer.transform_point(llpoint);
            let (sx, sz) = map_transformation::rotate::rotate_xz_point(
                xzpoint.x,
                xzpoint.z,
                args.rotation,
                &pre_rot_bbox,
            );

            Some((sx, sz))
        }
        _ => None,
    };

    // Derive terrain-aware spawn Y while `ground` is still in scope (it gets
    // moved into `generate_world_with_options` below). Used only for Java's
    // post-generation `set_spawn_in_level_dat` call — Bedrock derives spawn Y
    // independently inside `BedrockWriter::write_level_dat`.
    let spawn_y_for_java = spawn_point.map(|(sx, sz)| {
        use coordinate_system::cartesian::XZPoint;
        let rel = XZPoint::new(sx - xzbbox.min_x(), sz - xzbbox.min_z());
        ground.level(rel) + 3
    });

    // Build generation options
    let luanti_game = if args.luanti {
        Some(luanti_block_map::LuantiGame::Mineclonia)
    } else {
        None
    };

    let generation_options = data_processing::GenerationOptions {
        path: generation_path.clone(),
        format: world_format,
        level_name,
        spawn_point,
        luanti_game,
        ground_level: args.ground_level,
    };

    // Generate world
    match data_processing::generate_world_with_options(
        parsed_elements,
        xzbbox,
        args.bbox,
        ground,
        &args,
        generation_options,
        outline_suppression,
        part_groups,
    ) {
        Ok(_) => {
            if args.bedrock {
                println!(
                    "{} Bedrock world saved to: {}",
                    "Done!".green().bold(),
                    generation_path.display()
                );
            }

            // For Java Edition, update spawn point in level.dat if provided
            if !args.bedrock {
                if let (Some((spawn_x, spawn_z)), Some(spawn_y)) = (spawn_point, spawn_y_for_java) {
                    if let Err(e) = world_utils::set_spawn_in_level_dat(
                        &generation_path,
                        spawn_x,
                        spawn_y,
                        spawn_z,
                    ) {
                        eprintln!(
                            "{} Failed to set spawn point in level.dat: {}",
                            "Warning:".yellow().bold(),
                            e
                        );
                    }
                }

                // Apply game mode + initial world time to level.dat (Java).
                if let Err(e) = world_utils::apply_java_world_settings(
                    &generation_path,
                    args.gamemode,
                    args.world_time,
                ) {
                    eprintln!(
                        "{} Failed to apply world settings: {}",
                        "Warning:".yellow().bold(),
                        e
                    );
                }
            }
        }
        Err(e) => {
            eprintln!("{} {}", "Error:".red().bold(), e);
            std::process::exit(1);
        }
    }
}

fn main() {
    // If on Windows, free and reattach to the parent console when using as a CLI tool
    // Either of these can fail, but if they do it is not an issue, so the return value is ignored
    #[cfg(target_os = "windows")]
    unsafe {
        let _ = FreeConsole();
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }

    // Only run CLI mode if the user supplied args.
    #[cfg(feature = "gui")]
    {
        let gui_mode = std::env::args().len() == 1; // Just "arnis" with no args
        if gui_mode {
            gui::run_gui();
        }
    }

    run_cli();
}
