//! Java Edition Anvil format world saving.
//!
//! This module handles saving worlds in the Java Edition Anvil (.mca) format.

use super::common::{Chunk, ChunkToModify, Section};
use super::WorldEditor;
use crate::block_definitions::GRASS_BLOCK;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use fastanvil::Region;
use fastnbt::{ByteArray, LongArray, Value};
use fnv::FnvHashMap;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// The `DataVersion` stamped into every chunk, so a generated world loads natively in the
/// target release without a DataFixer upgrade pass.
///
/// Runtime-settable because it is version-dependent: [`set_data_version`] is called once
/// per run from the resolved [`crate::mc_version`] capabilities. The default is the value
/// the writer has always emitted, kept so a run that names no version is unchanged. A
/// WRONG value here produces a world that loads and then quietly misbehaves, which is why
/// the table it comes from only carries numbers read out of real worlds.
static DATA_VERSION: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(4440);

/// Set the `DataVersion` for this run. Call before the first chunk is written.
pub fn set_data_version(v: i32) {
    DATA_VERSION.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// The `DataVersion` chunks are being written with.
#[inline]
pub fn data_version() -> i32 {
    DATA_VERSION.load(std::sync::atomic::Ordering::Relaxed)
}

/// Cached base chunk sections (grass at Y=-62)
/// Computed once on first use and reused for all empty chunks
static BASE_CHUNK_SECTIONS: OnceLock<Vec<Section>> = OnceLock::new();

/// Get or create the cached base chunk sections
fn get_base_chunk_sections() -> &'static [Section] {
    BASE_CHUNK_SECTIONS.get_or_init(|| {
        let mut chunk = ChunkToModify::default();
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, -62, z, GRASS_BLOCK);
            }
        }
        chunk.sections().collect()
    })
}

#[cfg(feature = "gui")]
use crate::telemetry::{send_log, LogLevel};

impl<'a> WorldEditor<'a> {
    /// Helper function to create a base chunk with grass blocks at Y -62
    /// Uses cached sections for efficiency - only serialization happens per chunk
    pub(super) fn create_base_chunk(
        abs_chunk_x: i32,
        abs_chunk_z: i32,
        bake_lighting: bool,
        biome_value: &Value,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
        // Use cached sections (computed once on first call)
        let sections = get_base_chunk_sections();

        // Prepare chunk data with cloned sections
        let chunk_data = Chunk {
            sections: sections.to_vec(),
            x_pos: abs_chunk_x,
            z_pos: abs_chunk_z,
            is_light_on: 0,
            other: FnvHashMap::default(),
        };

        let chunk_nbt = create_chunk_nbt(&chunk_data, bake_lighting, biome_value);

        let mut ser_buffer = Vec::with_capacity(8192);
        fastnbt::to_writer(&mut ser_buffer, &chunk_nbt)?;

        Ok(ser_buffer)
    }

    /// Saves the world in Java Edition Anvil format.
    ///
    /// Uses parallel processing with rayon for fast region saving.
    /// Returns an error if any region fails to save (e.g. disk full).
    pub(super) fn save_java(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        println!("{} Saving world...", "[7/7]".bold());
        emit_gui_progress_update(90.0, "Saving world...");

        // Save metadata with error handling
        if let Err(e) = self.save_metadata() {
            eprintln!("Failed to save world metadata: {}", e);
            #[cfg(feature = "gui")]
            send_log(LogLevel::Warning, "Failed to save world metadata.");
            // Continue with world saving even if metadata fails
        }

        // World scaffolding seeds `region/r.0.0.mca` from the bundled Anvil template so a
        // freshly created world is loadable even before generation writes anything. Under
        // B_Linear nothing ever overwrites that file — the container gives it a different
        // extension — so it would linger as the one Anvil region in a b_linear world and
        // read as a mixed-format world to every tool downstream.
        if self.region_container == super::RegionContainer::BlinearV3 {
            let stray = self.world_dir.join("region").join("r.0.0.mca");
            if stray.exists() {
                if let Err(e) = std::fs::remove_file(&stray) {
                    eprintln!("Failed to remove the Anvil scaffold region {stray:?}: {e}");
                }
            }
        }

        if self.world.regions.is_empty() {
            return Ok(());
        }

        // Compute region bounds from original bbox to skip halo regions.
        // A region at (rx, rz) covers blocks [rx*512 .. rx*512+511] × [rz*512 .. rz*512+511].
        let min_region_x = self.xzbbox.min_x().div_euclid(512);
        let max_region_x = self.xzbbox.max_x().div_euclid(512);
        let min_region_z = self.xzbbox.min_z().div_euclid(512);
        let max_region_z = self.xzbbox.max_z().div_euclid(512);

        let total_regions = self
            .world
            .regions
            .keys()
            .filter(|(rx, rz)| {
                *rx >= min_region_x
                    && *rx <= max_region_x
                    && *rz >= min_region_z
                    && *rz <= max_region_z
            })
            .count() as u64;

        if total_regions == 0 {
            return Ok(());
        }

        let save_pb = ProgressBar::new(total_regions);
        save_pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:45}] {pos}/{len} regions ({eta})",
                )
                .unwrap()
                .progress_chars("█▓░"),
        );

        let regions_processed = AtomicU64::new(0);
        let should_stop = std::sync::atomic::AtomicBool::new(false);
        let first_error: Mutex<Option<Box<dyn std::error::Error + Send + Sync>>> = Mutex::new(None);

        self.world
            .regions
            .par_iter()
            .for_each(|((region_x, region_z), region_to_modify)| {
                // Skip halo regions outside the original bbox.
                if *region_x < min_region_x
                    || *region_x > max_region_x
                    || *region_z < min_region_z
                    || *region_z > max_region_z
                {
                    return;
                }

                // Fast-path: bail out without locking once an error has been recorded.
                if should_stop.load(Ordering::Acquire) {
                    return;
                }

                if let Err(e) = self.save_single_region(*region_x, *region_z, region_to_modify) {
                    let mut guard = first_error.lock().unwrap_or_else(|p| p.into_inner());
                    if guard.is_none() {
                        *guard = Some(e);
                    }
                    should_stop.store(true, Ordering::Release);
                    return;
                }

                // Update progress
                let regions_done = regions_processed.fetch_add(1, Ordering::SeqCst) + 1;

                let update_interval = (total_regions / 10).max(1);
                if regions_done.is_multiple_of(update_interval) || regions_done == total_regions {
                    let progress = 90.0 + (regions_done as f64 / total_regions as f64) * 9.0;
                    emit_gui_progress_update(progress, "Saving world...");
                }

                save_pb.inc(1);
            });

        save_pb.finish();

        if let Some(e) = first_error.lock().unwrap_or_else(|p| p.into_inner()).take() {
            return Err(e);
        }

        Ok(())
    }

    /// Saves a single region to disk.
    ///
    /// Optimized for new world creation, writes chunks directly without reading existing data.
    /// This assumes we're creating a fresh world, not modifying an existing one.
    pub(super) fn save_single_region(
        &self,
        region_x: i32,
        region_z: i32,
        region_to_modify: &super::common::RegionToModify,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        write_region_to_disk(
            &self.world_dir,
            &self.llbbox,
            self.ground_origin_x,
            self.ground_origin_z,
            self.xzbbox.min_z(),
            self.xzbbox.max_z(),
            self.ground.as_deref(),
            self.bake_lighting,
            region_x,
            region_z,
            region_to_modify,
            self.region_container,
            self.blinear_level,
        )
    }
}

/// Latitude that drives the temperature-based biome (taiga/forest/jungle) for a
/// chunk, and therefore its grass and leaf tint.
///
/// Single-world (no `--seed`): the cell's bbox-CENTRE latitude, as before.
///
/// Tile-invariant (`--seed`/Meld): each chunk's latitude is derived from its
/// absolute world Z using this cell's own `llbbox`<->`xzbbox` mapping. Because the
/// origin-anchored coordinate fix makes `lat(block_z)` a single global straight
/// line, every cell reconstructs the SAME line from its two endpoints, so two
/// adjacent cells agree on a shared chunk's latitude. That removes the per-cell
/// biome step (and the grass/leaf-colour seam it caused) at cell borders.
fn biome_lat_for_chunk(
    abs_chunk_z: i32,
    center_lat: f64,
    xz_min_z: i32,
    xz_max_z: i32,
    north_lat: f64,
    south_lat: f64,
) -> f64 {
    if !crate::ground_generation::tile_invariant_enabled() {
        return center_lat;
    }
    if xz_max_z <= xz_min_z {
        return center_lat;
    }
    let min_z = f64::from(xz_min_z);
    let max_z = f64::from(xz_max_z);
    let world_z = f64::from(abs_chunk_z * 16 + 8); // chunk-centre block Z
    let t = (world_z - min_z) / (max_z - min_z);
    north_lat + t * (south_lat - north_lat)
}

/// Where one region's chunks go. Both variants take the same uncompressed chunk NBT
/// and differ only in how they frame and compress it on disk, which is what lets the
/// container be a per-run switch rather than a separate world format.
enum RegionSink {
    Anvil(Region<File>),
    Blinear(super::blinear::BlinearRegionWriter),
}

impl RegionSink {
    fn create(
        world_dir: &std::path::Path,
        region_x: i32,
        region_z: i32,
        container: super::RegionContainer,
        blinear_level: i32,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        match container {
            super::RegionContainer::Anvil => Ok(RegionSink::Anvil(create_region_file(
                world_dir, region_x, region_z,
            )?)),
            super::RegionContainer::BlinearV3 => Ok(RegionSink::Blinear(
                super::blinear::BlinearRegionWriter::create(
                    world_dir,
                    region_x,
                    region_z,
                    blinear_level,
                )?,
            )),
        }
    }

    fn write_chunk(
        &mut self,
        chunk_x: usize,
        chunk_z: usize,
        nbt: &[u8],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self {
            RegionSink::Anvil(region) => {
                region.write_chunk(chunk_x, chunk_z, nbt)?;
                Ok(())
            }
            RegionSink::Blinear(writer) => writer.write_chunk(chunk_x, chunk_z, nbt),
        }
    }

    /// Anvil publishes as it goes, so only B_Linear has work left here.
    fn finish(self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self {
            RegionSink::Anvil(_) => Ok(()),
            RegionSink::Blinear(writer) => writer.finish(),
        }
    }
}

/// Open (truncating) a fresh `r.X.Z.mca` under `world_dir/region`.
fn create_region_file(
    world_dir: &std::path::Path,
    region_x: i32,
    region_z: i32,
) -> Result<Region<File>, Box<dyn std::error::Error + Send + Sync>> {
    let region_dir = world_dir.join("region");
    let out_path = region_dir.join(format!("r.{}.{}.mca", region_x, region_z));
    std::fs::create_dir_all(&region_dir)?;

    const REGION_TEMPLATE: &[u8] = include_bytes!("../../assets/minecraft/region.template");
    let mut region_file: File = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&out_path)?;
    region_file.write_all(REGION_TEMPLATE)?;
    Ok(Region::from_stream(region_file)?)
}

/// Serialize one region's chunks to its `.mca`. Shared by the synchronous save
/// path and the background flush worker (hence free-standing, not `&self`).
///
/// `ground_origin_x/z` is the MAIN world's origin (tile editors override it via
/// `set_ground_origin`) so biome land-cover lookups index the shared global grid;
/// `xz_min_z/xz_max_z` is THIS cell's own bbox z-extent, used to reconstruct the
/// global latitude line per chunk for tile-invariant (`--seed`/Meld) rendering.
#[allow(clippy::too_many_arguments)]
fn write_region_to_disk(
    world_dir: &std::path::Path,
    llbbox: &crate::coordinate_system::geographic::LLBBox,
    ground_origin_x: i32,
    ground_origin_z: i32,
    xz_min_z: i32,
    xz_max_z: i32,
    ground: Option<&crate::ground::Ground>,
    bake_lighting: bool,
    region_x: i32,
    region_z: i32,
    region_to_modify: &super::common::RegionToModify,
    container: super::RegionContainer,
    blinear_level: i32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut region = RegionSink::create(world_dir, region_x, region_z, container, blinear_level)?;
    let mut ser_buffer = Vec::with_capacity(8192);

    // World-center latitude drives temperature-based biome variants (taiga
    // vs forest vs jungle) at chunk-build time. Cheap to recompute.
    let center_lat = (llbbox.min().lat() + llbbox.max().lat()) * 0.5;
    let north_lat = llbbox.max().lat(); // +Z is south, so max lat is the north edge
    let south_lat = llbbox.min().lat();

    // First pass: write all chunks that have content
    for (&(chunk_x, chunk_z), chunk_to_modify) in &region_to_modify.chunks {
        if !chunk_to_modify.sections.is_empty() || !chunk_to_modify.other.is_empty() {
            let abs_chunk_x = chunk_x + (region_x * 32);
            let abs_chunk_z = chunk_z + (region_z * 32);
            // Correctness fix H2: dedup block-entity / entity compound lists by
            // coordinate before serializing. Tile halos cause the same boundary
            // banner/sign/chest to be merged into `other` 2+ times (common.rs
            // does an `extend` with no dedup); writing them verbatim would
            // duplicate the block entity in-game.
            let mut other = chunk_to_modify.other.clone();
            for key in ["block_entities", "entities"] {
                if let Some(Value::List(list)) = other.get_mut(key) {
                    *list = dedup_compound_list(list);
                }
            }
            let chunk = Chunk {
                sections: chunk_to_modify.sections().collect(),
                x_pos: abs_chunk_x,
                z_pos: abs_chunk_z,
                is_light_on: 0,
                other,
            };

            let biome_value = crate::biome::build_chunk_biome_nbt(
                abs_chunk_x,
                abs_chunk_z,
                ground_origin_x,
                ground_origin_z,
                ground,
                biome_lat_for_chunk(
                    abs_chunk_z,
                    center_lat,
                    xz_min_z,
                    xz_max_z,
                    north_lat,
                    south_lat,
                ),
            );
            let chunk_nbt = create_chunk_nbt(&chunk, bake_lighting, &biome_value);
            ser_buffer.clear();
            fastnbt::to_writer(&mut ser_buffer, &chunk_nbt)?;
            region.write_chunk(chunk_x as usize, chunk_z as usize, &ser_buffer)?;
        }
    }

    // Second pass: ensure all chunks exist (fill with base layer if not).
    // Skip entirely when region already has all 1024 chunks (common after ground gen).
    if region_to_modify.chunks.len() < 1024 {
        for chunk_x in 0..32 {
            for chunk_z in 0..32 {
                if !region_to_modify.chunks.contains_key(&(chunk_x, chunk_z)) {
                    let abs_chunk_x = chunk_x + (region_x * 32);
                    let abs_chunk_z = chunk_z + (region_z * 32);
                    let biome_value = crate::biome::build_chunk_biome_nbt(
                        abs_chunk_x,
                        abs_chunk_z,
                        ground_origin_x,
                        ground_origin_z,
                        ground,
                        biome_lat_for_chunk(
                            abs_chunk_z,
                            center_lat,
                            xz_min_z,
                            xz_max_z,
                            north_lat,
                            south_lat,
                        ),
                    );
                    let ser_buffer = WorldEditor::create_base_chunk(
                        abs_chunk_x,
                        abs_chunk_z,
                        bake_lighting,
                        &biome_value,
                    )?;
                    region.write_chunk(chunk_x as usize, chunk_z as usize, &ser_buffer)?;
                }
            }
        }
    }

    region.finish()
}

/// Owned, `Send` context for writing regions off the main thread (background flush).
/// Mirrors the fields `write_region_to_disk` needs from a `WorldEditor`.
pub(crate) struct RegionWriteCtx {
    world_dir: std::path::PathBuf,
    llbbox: crate::coordinate_system::geographic::LLBBox,
    ground_origin_x: i32,
    ground_origin_z: i32,
    xz_min_z: i32,
    xz_max_z: i32,
    ground: Option<std::sync::Arc<crate::ground::Ground>>,
    bake_lighting: bool,
    container: super::RegionContainer,
    blinear_level: i32,
}

impl RegionWriteCtx {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        world_dir: std::path::PathBuf,
        llbbox: crate::coordinate_system::geographic::LLBBox,
        ground_origin_x: i32,
        ground_origin_z: i32,
        xz_min_z: i32,
        xz_max_z: i32,
        ground: Option<std::sync::Arc<crate::ground::Ground>>,
        bake_lighting: bool,
        container: super::RegionContainer,
        blinear_level: i32,
    ) -> Self {
        Self {
            world_dir,
            llbbox,
            ground_origin_x,
            ground_origin_z,
            xz_min_z,
            xz_max_z,
            ground,
            bake_lighting,
            container,
            blinear_level,
        }
    }

    pub(crate) fn write(
        &self,
        region_x: i32,
        region_z: i32,
        region_to_modify: &super::common::RegionToModify,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        write_region_to_disk(
            &self.world_dir,
            &self.llbbox,
            self.ground_origin_x,
            self.ground_origin_z,
            self.xz_min_z,
            self.xz_max_z,
            self.ground.as_deref(),
            self.bake_lighting,
            region_x,
            region_z,
            region_to_modify,
            self.container,
            self.blinear_level,
        )
    }
}

/// Helper function to get entity coordinates
/// Note: Currently unused since we write directly without merging, but kept for potential future use
#[inline]
#[allow(dead_code)]
fn get_entity_coords(entity: &HashMap<String, Value>) -> Option<(i32, i32, i32)> {
    if let Some(Value::List(pos)) = entity.get("Pos") {
        if pos.len() == 3 {
            if let (Some(x), Some(y), Some(z)) = (
                value_to_i32(&pos[0]),
                value_to_i32(&pos[1]),
                value_to_i32(&pos[2]),
            ) {
                return Some((x, y, z));
            }
        }
    }

    let (Some(x), Some(y), Some(z)) = (
        entity.get("x").and_then(value_to_i32),
        entity.get("y").and_then(value_to_i32),
        entity.get("z").and_then(value_to_i32),
    ) else {
        return None;
    };

    Some((x, y, z))
}

/// Correctness fix H2: dedup a block-entity / entity compound list by coordinate.
///
/// Ways and relations are assigned to every tile their AABB+halo overlaps, so a
/// boundary banner/sign/chest gets emitted by 2+ tile editors. The merge in
/// `common.rs` does `self_list.extend(other_list)` with no coordinate dedup, so
/// the same block entity ends up in `chunk.other["block_entities"]` more than
/// once and is written verbatim to the `.mca`, producing an in-game duplicate.
/// (Bedrock already dedups via its own `dedup_compound_list`; Java did not.)
///
/// Keys on the absolute `(x, y, z)` int tags inside each compound (block
/// entities store absolute coords; entities use a `Pos` list — both handled by
/// `get_entity_coords`). On a coordinate collision the FIRST compound in
/// tile-merge order is kept, which is deterministic and safe because duplicates
/// for the same coordinate carry identical content. Non-compound values and
/// compounds without coordinates are passed through untouched.
fn dedup_compound_list(values: &[Value]) -> Vec<Value> {
    let mut seen: std::collections::HashSet<(i32, i32, i32)> = std::collections::HashSet::new();
    let mut deduped: Vec<Value> = Vec::with_capacity(values.len());

    for value in values {
        if let Value::Compound(map) = value {
            if let Some(coords) = get_entity_coords(map) {
                if !seen.insert(coords) {
                    // Already kept a compound at this coordinate: drop the duplicate.
                    continue;
                }
            }
        }
        deduped.push(value.clone());
    }

    deduped
}

// Reads a string blockstate property, if present.
fn prop_str<'a>(props: Option<&'a Value>, key: &str) -> Option<&'a str> {
    if let Some(Value::Compound(m)) = props {
        if let Some(Value::String(s)) = m.get(key) {
            return Some(s.as_str());
        }
    }
    None
}

// Light a block emits, 0 if none. Honours the `lit` state where it matters.
fn block_light_emission(name: &str, props: Option<&Value>) -> u8 {
    let unlit = matches!(prop_str(props, "lit"), Some("false"));
    match name {
        "minecraft:beacon"
        | "minecraft:conduit"
        | "minecraft:end_gateway"
        | "minecraft:end_portal"
        | "minecraft:fire"
        | "minecraft:glowstone"
        | "minecraft:jack_o_lantern"
        | "minecraft:lantern"
        | "minecraft:lava"
        | "minecraft:ochre_froglight"
        | "minecraft:pearlescent_froglight"
        | "minecraft:verdant_froglight"
        | "minecraft:sea_lantern"
        | "minecraft:shroomlight" => 15,
        "minecraft:campfire" if !unlit => 15,
        "minecraft:end_rod" | "minecraft:torch" | "minecraft:wall_torch" => 14,
        "minecraft:crying_obsidian"
        | "minecraft:soul_lantern"
        | "minecraft:soul_torch"
        | "minecraft:soul_wall_torch" => 10,
        "minecraft:soul_campfire" if !unlit => 10,
        "minecraft:glow_lichen" => 7,
        "minecraft:redstone_torch" | "minecraft:redstone_wall_torch" if !unlit => 7,
        "minecraft:sculk_catalyst" => 6,
        "minecraft:magma_block" => 3,
        "minecraft:brewing_stand" | "minecraft:brown_mushroom" => 1,
        _ => 0,
    }
}

// Blocks that don't occlude skylight: air, glass, leaves, water/ice, plants, thin decor.
fn is_light_transparent(name: &str) -> bool {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    if n == "tinted_glass" {
        return false; // tinted glass blocks all light, unlike other glass
    }
    if n.ends_with("air") || n.ends_with("leaves") || n.contains("glass") {
        return true;
    }
    const EXACT: &[&str] = &[
        "water",
        "ice",
        "frosted_ice",
        "bubble_column",
        "grass",
        "short_grass",
        "tall_grass",
        "fern",
        "large_fern",
        "dead_bush",
        "bush",
        "dandelion",
        "poppy",
        "blue_orchid",
        "allium",
        "azure_bluet",
        "oxeye_daisy",
        "cornflower",
        "lily_of_the_valley",
        "wither_rose",
        "torchflower",
        "red_tulip",
        "orange_tulip",
        "white_tulip",
        "pink_tulip",
        "sunflower",
        "lilac",
        "rose_bush",
        "peony",
        "pink_petals",
        "spore_blossom",
        "small_dripleaf",
        "small_amethyst_bud",
        "medium_amethyst_bud",
        "large_amethyst_bud",
        "amethyst_cluster",
        "hanging_roots",
        "glow_lichen",
        // Crops. Missing here, they were treated as OPAQUE, so a column's skylight scan
        // stopped AT the crop and wrote 0 into the crop's own cell. With isLightOn set,
        // the client believes it: the crop renders black and is destroyed on the first
        // random tick, because a crop needs light 8. Measured before the fix: 3,211,825
        // of 3,211,825 crop cells at SkyLight 0.
        "wheat",
        "carrots",
        "potatoes",
        "beetroots",
        "torchflower_crop",
        "pitcher_crop",
        "melon_stem",
        "pumpkin_stem",
        "attached_melon_stem",
        "attached_pumpkin_stem",
        "cocoa",
        "big_dripleaf",
        "big_dripleaf_stem",
        "sweet_berry_bush",
        "cactus",
        "bamboo",
        "sugar_cane",
        "nether_wart",
        "lily_pad",
        "sea_pickle",
        "cobweb",
        "snow",
        "lever",
        "tripwire",
        "tripwire_hook",
        "end_rod",
        "lightning_rod",
        "scaffolding",
        "pointed_dripstone",
        "turtle_egg",
    ];
    if EXACT.contains(&n) {
        return true;
    }
    if n.ends_with("_block") || n.ends_with("_stem") {
        return false;
    }
    // Slabs/stairs are left opaque: their solid half occludes light via shape.
    const SUB: &[&str] = &[
        "torch", "lantern", "sign", "rail", "carpet", "candle", "chain", "ladder", "banner",
        "sapling", "coral", "sprouts", "roots", "vine", "flower", "mushroom", "amethyst", "_bud",
        "seagrass", "kelp", "petals", "fence", "wall", "_bars", "door",
    ];
    SUB.iter().any(|s| n.contains(s))
}

// Light a block removes: 0 passes, 1 attenuates (water/leaves/ice), 15 blocks.
fn light_opacity(name: &str) -> u8 {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    if n.ends_with("leaves")
        || matches!(
            n,
            "water"
                | "ice"
                | "frosted_ice"
                | "bubble_column"
                | "kelp"
                | "kelp_plant"
                | "seagrass"
                | "tall_seagrass"
        )
    {
        return 1;
    }
    if is_light_transparent(name) {
        0
    } else {
        15
    }
}

// Per-cell light opacity and emission for a section (YZX order, 4096 cells).
fn decode_section_light_props(section: &Section) -> (Vec<u8>, Vec<u8>) {
    let palette = &section.block_states.palette;
    let pal_opacity: Vec<u8> = palette.iter().map(|p| light_opacity(&p.name)).collect();
    let pal_emission: Vec<u8> = palette
        .iter()
        .map(|p| block_light_emission(&p.name, p.properties.as_ref()))
        .collect();

    let mut opacity = vec![0u8; 4096];
    let mut emission = vec![0u8; 4096];

    match &section.block_states.data {
        None => {
            let o = pal_opacity.first().copied().unwrap_or(0);
            let e = pal_emission.first().copied().unwrap_or(0);
            if o > 0 {
                opacity.iter_mut().for_each(|v| *v = o);
            }
            if e > 0 {
                emission.iter_mut().for_each(|v| *v = e);
            }
        }
        Some(data) => {
            let mut bits = 4;
            while (1usize << bits) < palette.len() {
                bits += 1;
            }
            let vals_per_long = 64 / bits;
            let mask = (1u64 << bits) - 1;
            for i in 0..4096usize {
                let long_idx = i / vals_per_long;
                let off = (i % vals_per_long) * bits;
                if long_idx < data.len() {
                    let pi = ((data[long_idx] as u64 >> off) & mask) as usize;
                    if pi < palette.len() {
                        opacity[i] = pal_opacity[pi];
                        emission[i] = pal_emission[pi];
                    }
                }
            }
        }
    }
    (opacity, emission)
}

// Flood-fill light from the seeded queue, -1 per block, through cells light can enter.
fn propagate_light(
    light: &mut [u8],
    opacity: &[u8],
    queue: &mut std::collections::VecDeque<(usize, usize, usize, u8)>,
    height: usize,
) {
    let idx = |x: usize, y: usize, z: usize| y * 256 + z * 16 + x;
    while let Some((x, y, z, level)) = queue.pop_front() {
        if level <= 1 {
            continue;
        }
        let next = level - 1;
        let try_spread =
            |nx: usize,
             ny: usize,
             nz: usize,
             light: &mut [u8],
             queue: &mut std::collections::VecDeque<(usize, usize, usize, u8)>| {
                let g = idx(nx, ny, nz);
                if opacity[g] < 15 && light[g] < next {
                    light[g] = next;
                    queue.push_back((nx, ny, nz, next));
                }
            };
        if x > 0 {
            try_spread(x - 1, y, z, light, queue);
        }
        if x < 15 {
            try_spread(x + 1, y, z, light, queue);
        }
        if z > 0 {
            try_spread(x, y, z - 1, light, queue);
        }
        if z < 15 {
            try_spread(x, y, z + 1, light, queue);
        }
        if y > 0 {
            try_spread(x, y - 1, z, light, queue);
        }
        if y + 1 < height {
            try_spread(x, y + 1, z, light, queue);
        }
    }
}

// Pack a 0..15 value into the 4-bit-per-cell light array.
#[inline]
fn pack_light_nibble(arr: &mut [i8], index: usize, value: u8) {
    let byte = index >> 1;
    let cur = arr[byte] as u8;
    let new = if index & 1 == 0 {
        (cur & 0xF0) | (value & 0x0F)
    } else {
        (cur & 0x0F) | (value << 4)
    };
    arr[byte] = new as i8;
}

// Sky + block light per section as 2048-byte nibble arrays.
fn compute_lighting(
    sections: &[Section],
    min_section_y: i8,
    max_section_y: i8,
) -> Vec<(Vec<i8>, Vec<i8>)> {
    use std::collections::VecDeque;

    let num_sections = (max_section_y as i32 - min_section_y as i32 + 1).max(0) as usize;
    let height = num_sections * 16;
    if height == 0 {
        return Vec::new();
    }
    let idx = |x: usize, y: usize, z: usize| y * 256 + z * 16 + x;

    let mut opacity = vec![0u8; height * 256];
    let mut emission = vec![0u8; height * 256];

    let sec_by_y: HashMap<i8, &Section> = sections.iter().map(|s| (s.y, s)).collect();
    let mut htop = 0usize;
    let mut any_solid = false;
    let mut any_emitter = false;
    for sy in min_section_y..=max_section_y {
        let Some(sec) = sec_by_y.get(&sy) else {
            continue;
        };
        let base_y = ((sy as i32 - min_section_y as i32) * 16) as usize;
        let (op, em) = decode_section_light_props(sec);
        for ly in 0..16usize {
            for z in 0..16usize {
                for x in 0..16usize {
                    let local = ly * 256 + z * 16 + x;
                    let g = idx(x, base_y + ly, z);
                    opacity[g] = op[local];
                    emission[g] = em[local];
                    any_emitter |= em[local] > 0;
                    if op[local] > 0 {
                        any_solid = true;
                        htop = htop.max(base_y + ly);
                    }
                }
            }
        }
    }

    // SkyLight: open sky above the highest non-transparent block is 15; flood-fill the band below.
    let top = if any_solid { (htop + 2).min(height) } else { 0 };
    let mut sky = vec![0u8; height * 256];
    sky[top * 256..].fill(15);
    let mut sq: VecDeque<(usize, usize, usize, u8)> = VecDeque::new();
    for z in 0..16usize {
        for x in 0..16usize {
            let mut level = 15u8;
            for y in (0..top).rev() {
                let g = idx(x, y, z);
                if opacity[g] >= 15 {
                    break;
                }
                sky[g] = level;
                if level > 0 {
                    sq.push_back((x, y, z, level));
                }
                if opacity[g] > 0 {
                    level = level.saturating_sub(1);
                }
            }
        }
    }
    propagate_light(&mut sky, &opacity, &mut sq, height);

    // BlockLight: flood-fill from emitters.
    //
    // Most chunks have none. Terrain, fields, water, roads and unlit building shells emit
    // nothing, and for those the seed scan below walks every cell of the column - 98,304 of them
    // at vanilla height, more once the limit is lifted - to find that out, and then propagates a
    // field that is uniformly zero. `any_emitter` is set while the section data is decoded above,
    // at no extra cost, so the whole pass can be skipped when there is nothing to spread. The
    // output is unchanged: `block` is already all-zero, which is exactly what the flood fill
    // would have left it as.
    let mut block = vec![0u8; height * 256];
    if any_emitter {
        let mut bq: VecDeque<(usize, usize, usize, u8)> = VecDeque::new();
        for g in 0..height * 256 {
            if emission[g] > 0 {
                block[g] = emission[g];
                let rem = g % 256;
                bq.push_back((rem % 16, g / 256, rem / 16, emission[g]));
            }
        }
        propagate_light(&mut block, &opacity, &mut bq, height);
    }

    let mut out = Vec::with_capacity(num_sections);
    for s in 0..num_sections {
        let base_y = s * 16;
        let mut sl = vec![0i8; 2048];
        let mut bl = vec![0i8; 2048];
        for ly in 0..16usize {
            for z in 0..16usize {
                for x in 0..16usize {
                    let local = ly * 256 + z * 16 + x;
                    let g = idx(x, base_y + ly, z);
                    pack_light_nibble(&mut sl, local, sky[g]);
                    pack_light_nibble(&mut bl, local, block[g]);
                }
            }
        }
        out.push((sl, bl));
    }
    out
}

/// Cached air block_states value shared by all empty sections.
static AIR_BLOCK_STATES: OnceLock<Value> = OnceLock::new();

fn get_air_block_states() -> &'static Value {
    AIR_BLOCK_STATES.get_or_init(|| {
        Value::Compound(HashMap::from([(
            "palette".to_string(),
            Value::List(vec![Value::Compound(HashMap::from([(
                "Name".to_string(),
                Value::String("minecraft:air".to_string()),
            )]))]),
        )]))
    })
}

/// Cached structures value shared by all chunks.
static STRUCTURES_VALUE: OnceLock<Value> = OnceLock::new();

fn get_structures_value() -> &'static Value {
    STRUCTURES_VALUE.get_or_init(|| {
        Value::Compound(HashMap::from([
            ("References".to_string(), Value::Compound(HashMap::new())),
            ("starts".to_string(), Value::Compound(HashMap::new())),
        ]))
    })
}

/// Creates modern chunk NBT data (post-1.18 format, no Level wrapper).
///
/// Writes all required fields for server compatibility:
/// DataVersion, Status, yPos, Heightmaps, biomes, structures, etc.
/// Section range is determined dynamically: at minimum the vanilla range
/// (Y=-4 to Y=19), extended upward/downward to cover any sections with content.
fn create_chunk_nbt(
    chunk: &Chunk,
    bake_lighting: bool,
    biome_value: &Value,
) -> HashMap<String, Value> {
    // Index existing sections by Y for quick lookup
    let section_map: HashMap<i8, usize> = chunk
        .sections
        .iter()
        .enumerate()
        .map(|(i, s)| (s.y, i))
        .collect();

    // Determine section range: start with vanilla range, expand to cover content
    let mut min_section_y: i8 = -4; // vanilla min (Y=-64)
    let mut max_section_y: i8 = 19; // vanilla max (Y=319)
    for &y in section_map.keys() {
        if y < min_section_y {
            min_section_y = y;
        }
        if y > max_section_y {
            max_section_y = y;
        }
    }

    // Bake lighting only when requested; otherwise leave it for the engine to relight on load.
    let mut lighting = if bake_lighting {
        compute_lighting(&chunk.sections, min_section_y, max_section_y)
    } else {
        Vec::new()
    };

    // Build all sections in the determined range
    let sections: Vec<Value> = (min_section_y..=max_section_y)
        .enumerate()
        .map(|(off, y)| {
            let mut section_nbt = if let Some(&idx) = section_map.get(&y) {
                build_section_value(&chunk.sections[idx])
            } else {
                // Empty air section
                HashMap::from([
                    ("Y".to_string(), Value::Byte(y)),
                    ("block_states".to_string(), get_air_block_states().clone()),
                ])
            };
            section_nbt.insert("biomes".to_string(), biome_value.clone());
            if bake_lighting {
                let (sky_light, block_light) = std::mem::take(&mut lighting[off]);
                section_nbt.insert(
                    "SkyLight".to_string(),
                    Value::ByteArray(ByteArray::new(sky_light)),
                );
                // An all-zero BlockLight is exactly what the client assumes when the key is
                // absent, so writing it out is 2 KB of nothing. Measured across three real
                // worlds, 4096 chunks each: omitting it saves 481-491 compressed bytes per
                // chunk, about 7% of region-file bytes after sector quantisation.
                //
                // SkyLight is NOT symmetrical and must always be written. An absent SkyLight
                // does not read as zero or as 15: SkyLightSectionStorage.repeatFirstLayer
                // copies the bottom row of the nearest layer above into all sixteen slices, so
                // a missing array below the terrain's top section inherits the terrain's
                // shadow. With isLightOn set the client never relights, and the sky would be
                // permanently black - the crops-render-black bug at sky scale.
                if block_light.iter().any(|&b| b != 0) {
                    section_nbt.insert(
                        "BlockLight".to_string(),
                        Value::ByteArray(ByteArray::new(block_light)),
                    );
                }
            }
            Value::Compound(section_nbt)
        })
        .collect();

    // Compute heightmaps from block data
    let total_height = ((max_section_y as i32 + 1) - min_section_y as i32) * 16;
    let heightmaps = compute_heightmaps(&chunk.sections, min_section_y, total_height);

    // PostProcessing: one empty list per section
    let post_processing: Vec<Value> = (0..sections.len()).map(|_| Value::List(vec![])).collect();

    // Build root-level chunk NBT (modern format — no Level wrapper)
    let mut root = HashMap::from([
        ("DataVersion".to_string(), Value::Int(data_version())),
        ("xPos".to_string(), Value::Int(chunk.x_pos)),
        ("yPos".to_string(), Value::Int(min_section_y as i32)),
        ("zPos".to_string(), Value::Int(chunk.z_pos)),
        (
            "Status".to_string(),
            Value::String("minecraft:full".to_string()),
        ),
        ("isLightOn".to_string(), Value::Byte(bake_lighting as i8)),
        ("InhabitedTime".to_string(), Value::Long(0)),
        ("LastUpdate".to_string(), Value::Long(0)),
        ("sections".to_string(), Value::List(sections)),
        ("Heightmaps".to_string(), heightmaps),
        ("structures".to_string(), get_structures_value().clone()),
        ("PostProcessing".to_string(), Value::List(post_processing)),
        ("block_ticks".to_string(), Value::List(vec![])),
        ("fluid_ticks".to_string(), Value::List(vec![])),
        ("block_entities".to_string(), Value::List(vec![])),
    ]);

    // Merge extra chunk data (block_entities, entities, etc.)
    // This overwrites the empty defaults above when actual data exists.
    for (key, value) in &chunk.other {
        root.insert(key.clone(), value.clone());
    }

    root
}

/// Build a section Value from a Section struct.
fn build_section_value(section: &Section) -> HashMap<String, Value> {
    let mut block_states = HashMap::from([(
        "palette".to_string(),
        Value::List(
            section
                .block_states
                .palette
                .iter()
                .map(|item| {
                    let mut palette_item =
                        HashMap::from([("Name".to_string(), Value::String(item.name.clone()))]);
                    if let Some(props) = &item.properties {
                        palette_item.insert("Properties".to_string(), props.clone());
                    }
                    Value::Compound(palette_item)
                })
                .collect(),
        ),
    )]);

    // Only add the `data` attribute if it's non-empty
    // to maintain compatibility with third-party tools like Dynmap
    if let Some(data) = &section.block_states.data {
        if !data.is_empty() {
            block_states.insert("data".to_string(), Value::LongArray(data.to_owned()));
        }
    }

    HashMap::from([
        ("Y".to_string(), Value::Byte(section.y)),
        ("block_states".to_string(), Value::Compound(block_states)),
    ])
}

/// Compute heightmaps from section block data.
///
/// Returns a Value::Compound with four heightmap types (MOTION_BLOCKING,
/// MOTION_BLOCKING_NO_LEAVES, OCEAN_FLOOR, WORLD_SURFACE) as packed LongArrays.
/// Bit width is determined dynamically based on total world height.
/// Value for each column = (highest_non_air_Y - min_block_y + 1), or 0 if all air.
fn compute_heightmaps(sections: &[Section], min_section_y: i8, total_height: i32) -> Value {
    // Precompute per-section metadata to avoid redundant work in the inner loop.
    enum SectionKind<'a> {
        Uniform {
            solid: bool,
        },
        NoAir,
        Mixed {
            data: &'a LongArray,
            bits: usize,
            vals_per_long: usize,
            mask: u64,
        },
    }
    struct SectionMeta<'a> {
        y: i8,
        palette: &'a [super::common::PaletteItem],
        kind: SectionKind<'a>,
    }

    let mut metas: Vec<SectionMeta> = sections
        .iter()
        .map(|s| {
            let palette = &s.block_states.palette;
            let kind = if palette.len() == 1 {
                SectionKind::Uniform {
                    solid: palette[0].name != "minecraft:air",
                }
            } else if !palette.iter().any(|p| p.name == "minecraft:air") {
                SectionKind::NoAir
            } else if let Some(data) = &s.block_states.data {
                let mut bits = 4;
                while (1usize << bits) < palette.len() {
                    bits += 1;
                }
                SectionKind::Mixed {
                    data,
                    bits,
                    vals_per_long: 64 / bits,
                    mask: (1u64 << bits) - 1,
                }
            } else {
                SectionKind::Uniform { solid: false }
            };
            SectionMeta {
                y: s.y,
                palette,
                kind,
            }
        })
        .collect();

    // Sort by Y descending so we scan top-down
    metas.sort_by_key(|b| Reverse(b.y));

    let mut heights = [0i32; 256]; // 16x16 grid, Z-major order
    let min_block_y = min_section_y as i32 * 16;

    for z in 0..16usize {
        for x in 0..16usize {
            let col_idx = z * 16 + x;

            'outer: for meta in &metas {
                match &meta.kind {
                    SectionKind::Uniform { solid: false } => continue,
                    SectionKind::Uniform { solid: true } | SectionKind::NoAir => {
                        let abs_y = (meta.y as i32) * 16 + 15;
                        heights[col_idx] = abs_y - min_block_y + 1;
                        break 'outer;
                    }
                    SectionKind::Mixed {
                        data,
                        bits,
                        vals_per_long,
                        mask,
                    } => {
                        for local_y in (0..16usize).rev() {
                            let block_idx = local_y * 256 + z * 16 + x;
                            let long_idx = block_idx / vals_per_long;
                            let bit_offset = (block_idx % vals_per_long) * bits;

                            if long_idx < data.len() {
                                let palette_idx =
                                    ((data[long_idx] as u64 >> bit_offset) & mask) as usize;
                                if palette_idx < meta.palette.len()
                                    && meta.palette[palette_idx].name != "minecraft:air"
                                {
                                    let abs_y = (meta.y as i32) * 16 + local_y as i32;
                                    heights[col_idx] = abs_y - min_block_y + 1;
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let packed = pack_heightmap_values(&heights, total_height);
    // All four heightmap types use the same data. Vanilla differentiates them (e.g.
    // MOTION_BLOCKING includes fluids, OCEAN_FLOOR excludes them), but servers
    // recompute heightmaps on load — they just need valid structure here.
    Value::Compound(HashMap::from([
        (
            "MOTION_BLOCKING".to_string(),
            Value::LongArray(packed.clone()),
        ),
        (
            "MOTION_BLOCKING_NO_LEAVES".to_string(),
            Value::LongArray(packed.clone()),
        ),
        ("OCEAN_FLOOR".to_string(), Value::LongArray(packed.clone())),
        ("WORLD_SURFACE".to_string(), Value::LongArray(packed)),
    ]))
}

/// Pack 256 heightmap values into a LongArray with dynamic bit width.
/// Bit width = ceil(log2(total_height + 1)). Values don't span across longs.
fn pack_heightmap_values(values: &[i32; 256], total_height: i32) -> LongArray {
    // Calculate bits needed: ceil(log2(total_height + 1)), minimum 9
    let bits = ((total_height + 1) as f64).log2().ceil().max(9.0) as usize;
    let vals_per_long = 64 / bits;
    let num_longs = 256_usize.div_ceil(vals_per_long);
    let mask = (1i64 << bits) - 1;

    let mut result = Vec::with_capacity(num_longs);
    let mut current: i64 = 0;
    let mut bit_pos = 0;

    for &val in values.iter() {
        if bit_pos + bits > 64 {
            result.push(current);
            current = 0;
            bit_pos = 0;
        }
        current |= ((val as i64) & mask) << bit_pos;
        bit_pos += bits;
    }
    if bit_pos > 0 {
        result.push(current);
    }

    LongArray::new(result)
}

/// Merge compound lists (entities, block_entities) from chunk_to_modify into chunk
/// Note: Currently unused since we write directly without merging, but kept for potential future use
#[allow(dead_code)]
fn merge_compound_list(chunk: &mut Chunk, chunk_to_modify: &ChunkToModify, key: &str) {
    if let Some(existing_entities) = chunk.other.get_mut(key) {
        if let Some(new_entities) = chunk_to_modify.other.get(key) {
            if let (Value::List(existing), Value::List(new)) = (existing_entities, new_entities) {
                existing.retain(|e| {
                    if let Value::Compound(map) = e {
                        if let Some((x, y, z)) = get_entity_coords(map) {
                            return !new.iter().any(|new_e| {
                                if let Value::Compound(new_map) = new_e {
                                    get_entity_coords(new_map) == Some((x, y, z))
                                } else {
                                    false
                                }
                            });
                        }
                    }
                    true
                });
                existing.extend(new.clone());
            }
        }
    } else if let Some(new_entities) = chunk_to_modify.other.get(key) {
        chunk.other.insert(key.to_string(), new_entities.clone());
    }
}

/// Convert NBT Value to i32
/// Note: Currently unused since we write directly without merging, but kept for potential future use
#[allow(dead_code)]
fn value_to_i32(value: &Value) -> Option<i32> {
    match value {
        Value::Byte(v) => Some(i32::from(*v)),
        Value::Short(v) => Some(i32::from(*v)),
        Value::Int(v) => Some(*v),
        Value::Long(v) => i32::try_from(*v).ok(),
        Value::Float(v) => Some(*v as i32),
        Value::Double(v) => Some(*v as i32),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::common::{Chunk, ChunkToModify};
    use super::create_chunk_nbt;
    use crate::block_definitions::GRASS_BLOCK;
    use fastnbt::Value;
    use fnv::FnvHashMap;
    use std::collections::HashMap;

    fn grass_chunk() -> Chunk {
        let mut c = ChunkToModify::default();
        for x in 0..16 {
            for z in 0..16 {
                c.set_block(x, -62, z, GRASS_BLOCK);
            }
        }
        Chunk {
            sections: c.sections().collect(),
            x_pos: 0,
            z_pos: 0,
            is_light_on: 0,
            other: FnvHashMap::default(),
        }
    }

    fn sections(nbt: &HashMap<String, Value>) -> &Vec<Value> {
        match &nbt["sections"] {
            Value::List(v) => v,
            _ => panic!("sections is not a list"),
        }
    }

    fn plains_biome() -> Value {
        crate::biome::build_chunk_biome_nbt(0, 0, 0, 0, None, 0.0)
    }

    #[test]
    fn bake_lighting_writes_valid_light_arrays() {
        let nbt = create_chunk_nbt(&grass_chunk(), true, &plains_biome());
        assert_eq!(nbt["isLightOn"], Value::Byte(1));
        assert!(nbt.contains_key("Heightmaps"));
        let secs = sections(&nbt);
        assert_eq!(secs.len(), 24);
        for s in secs {
            let Value::Compound(m) = s else { panic!() };
            // SkyLight is written for EVERY section, always. Omitting it is not the inverse of
            // omitting BlockLight: repeatFirstLayer would copy the terrain's shadow upward into
            // the gap and the sky would render black for good.
            match &m["SkyLight"] {
                Value::ByteArray(b) => assert_eq!(b.len(), 2048),
                _ => panic!("SkyLight is not a byte array"),
            }
            // BlockLight only when something actually emits. grass_chunk() has no emitters, so
            // every one of these is all-zero and is left out - which the client reads
            // identically, for 481-491 fewer compressed bytes per chunk.
            assert!(
                !m.contains_key("BlockLight"),
                "an all-zero BlockLight should not be written"
            );
        }
    }

    #[test]
    fn a_chunk_with_an_emitter_keeps_its_block_light() {
        // The other half of the rule above: omission is driven by the CONTENT being zero, not by
        // baking being off, so a torch must still produce an array.
        use crate::block_definitions::GLOWSTONE;
        let mut c = ChunkToModify::default();
        for x in 0..16 {
            for z in 0..16 {
                c.set_block(x, -62, z, GRASS_BLOCK);
            }
        }
        c.set_block(8, -60, 8, GLOWSTONE);
        let chunk = Chunk {
            sections: c.sections().collect(),
            x_pos: 0,
            z_pos: 0,
            is_light_on: 0,
            other: FnvHashMap::default(),
        };
        let nbt = create_chunk_nbt(&chunk, true, &plains_biome());
        let lit = sections(&nbt)
            .iter()
            .filter(|s| matches!(s, Value::Compound(m) if m.contains_key("BlockLight")))
            .count();
        assert!(lit > 0, "an emitter must produce at least one BlockLight array");
    }

    #[test]
    fn no_bake_lighting_omits_light_arrays() {
        let nbt = create_chunk_nbt(&grass_chunk(), false, &plains_biome());
        assert_eq!(nbt["isLightOn"], Value::Byte(0));
        for s in sections(&nbt) {
            let Value::Compound(m) = s else { panic!() };
            assert!(!m.contains_key("SkyLight"));
            assert!(!m.contains_key("BlockLight"));
        }
    }

    #[test]
    fn a_crop_under_open_sky_is_baked_fully_lit() {
        // Regression: crops were absent from the light-transparency list, so the skylight
        // column scan treated wheat as opaque and stopped AT it, writing 0 into the crop's
        // own cell. With isLightOn set, the client believes that: the crop renders black
        // and Minecraft destroys it (a crop needs light 8). Reported as "the crops would be
        // black and then break"; measured as 3,211,825 of 3,211,825 crop cells at SkyLight 0.
        use crate::block_definitions::{FARMLAND, WHEAT};
        let mut c = ChunkToModify::default();
        for x in 0..16 {
            for z in 0..16 {
                c.set_block(x, -62, z, FARMLAND);
                c.set_block(x, -61, z, WHEAT);
            }
        }
        let chunk = Chunk {
            sections: c.sections().collect(),
            x_pos: 0,
            z_pos: 0,
            is_light_on: 0,
            other: FnvHashMap::default(),
        };
        let nbt = create_chunk_nbt(&chunk, true, &plains_biome());

        // Y -61 lives in section index 0 of a -64-based world, cell (0, 3, 0).
        let secs = sections(&nbt);
        let Value::Compound(m) = &secs[0] else {
            panic!("no section 0")
        };
        let Value::ByteArray(sky) = &m["SkyLight"] else {
            panic!("section 0 has no SkyLight")
        };
        let idx = 3 * 256; // y=3 (Y -61) at x=0, z=0
        let nib = (sky[idx / 2] as u8) & 0x0F;
        assert_eq!(nib, 15, "the crop cell must be baked at full skylight");
    }

    #[test]
    fn all_air_chunk_is_fully_skylit() {
        let chunk = Chunk {
            sections: vec![],
            x_pos: 0,
            z_pos: 0,
            is_light_on: 0,
            other: FnvHashMap::default(),
        };
        let nbt = create_chunk_nbt(&chunk, true, &plains_biome());
        for s in sections(&nbt) {
            let Value::Compound(m) = s else { panic!() };
            let Value::ByteArray(b) = &m["SkyLight"] else {
                panic!()
            };
            // 0xFF == two nibbles of 15 (full skylight)
            assert!(b.iter().all(|&v| v == -1));
        }
    }

    /// The container is a swap, not a transformation: the same chunk NBT handed to both
    /// sinks has to come back out of both files byte for byte. Generation itself varies
    /// palette order between processes, so this equivalence can only be established
    /// in-process, from one set of payloads.
    #[test]
    fn both_containers_store_identical_chunk_nbt() {
        use super::super::RegionContainer;
        use super::RegionSink;
        use fastanvil::Region;
        use std::fs::File;

        let dir = tempfile::tempdir().unwrap();
        let anvil_dir = dir.path().join("anvil");
        let blinear_dir = dir.path().join("blinear");

        // Payload shapes that exercise the framing: region corners, bucket boundaries,
        // a large chunk, and a tiny one.
        let payloads: Vec<(usize, usize, Vec<u8>)> = vec![
            (0, 0, b"corner-origin".to_vec()),
            (31, 0, vec![0x5A; 40_000]),
            (0, 1, b"first-of-bucket-1".to_vec()),
            (31, 1, b"last-of-bucket-1".to_vec()),
            (0, 2, b"first-of-bucket-2".to_vec()),
            (16, 17, (0u16..9000).flat_map(|v| v.to_be_bytes()).collect()),
            (31, 31, b"corner-far".to_vec()),
        ];

        for (container, out_dir) in [
            (RegionContainer::Anvil, &anvil_dir),
            (RegionContainer::BlinearV3, &blinear_dir),
        ] {
            let mut sink = RegionSink::create(out_dir, 2, -5, container, 6).unwrap();
            for (x, z, nbt) in &payloads {
                sink.write_chunk(*x, *z, nbt).unwrap();
            }
            sink.finish().unwrap();
        }

        let mut anvil = Region::from_stream(
            File::options()
                .read(true)
                .write(true)
                .open(anvil_dir.join("region/r.2.-5.mca"))
                .unwrap(),
        )
        .unwrap();
        let blinear = super::super::blinear::decode(
            &std::fs::read(blinear_dir.join("region/r.2.-5.b_linear")).unwrap(),
        );

        for (x, z, nbt) in &payloads {
            let from_anvil = anvil.read_chunk(*x, *z).unwrap().unwrap();
            let from_blinear = blinear[x + z * 32].as_deref().unwrap();
            assert_eq!(
                from_anvil.as_slice(),
                nbt.as_slice(),
                "anvil chunk ({x},{z})"
            );
            assert_eq!(from_blinear, nbt.as_slice(), "b_linear chunk ({x},{z})");
        }

        // Neither container invents chunks the caller never wrote.
        assert_eq!(
            blinear.iter().filter(|s| s.is_some()).count(),
            payloads.len()
        );
    }
}
