use crate::coordinate_system::geographic::LLBBox;
use crate::retrieve_data;
use fastnbt::Value;
use flate2::read::GzDecoder;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::{fs, io::Write};

/// Returns the Desktop directory for Bedrock .mcworld file output.
/// Falls back to home directory, then current directory.
pub fn get_bedrock_output_directory() -> PathBuf {
    dirs::desktop_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns Luanti's worlds directory for the current OS.
/// Windows: %APPDATA%\Minetest\worlds
/// macOS:   ~/Library/Application Support/minetest/worlds
/// Linux:   ~/.minetest/worlds
/// Falls back to Desktop/Arnis Luanti Worlds if no path can be resolved.
pub fn get_luanti_worlds_directory() -> PathBuf {
    let base = if cfg!(target_os = "windows") {
        dirs::data_dir().map(|p| p.join("Minetest"))
    } else if cfg!(target_os = "macos") {
        dirs::data_dir().map(|p| p.join("minetest"))
    } else {
        dirs::home_dir().map(|p| p.join(".minetest"))
    };

    base.map(|p| p.join("worlds")).unwrap_or_else(|| {
        dirs::desktop_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Arnis Luanti Worlds")
    })
}

/// Gets the area name for a given bounding box using the center point.
pub fn get_area_name_for_bedrock(bbox: &LLBBox) -> String {
    let center_lat = (bbox.min().lat() + bbox.max().lat()) / 2.0;
    let center_lon = (bbox.min().lng() + bbox.max().lng()) / 2.0;

    match retrieve_data::fetch_area_name(center_lat, center_lon) {
        Ok(Some(name)) => name,
        _ => "Unknown Location".to_string(),
    }
}

/// Sanitizes an area name for safe use in filesystem paths.
/// Replaces characters that are invalid on Windows/macOS/Linux, trims whitespace,
/// and limits length to prevent excessively long filenames.
pub fn sanitize_for_filename(name: &str) -> String {
    let invalid_chars = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];
    let mut sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_control() || invalid_chars.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    sanitized = sanitized.trim().to_string();

    // Limit length to avoid excessively long filenames
    const MAX_LEN: usize = 64;
    if sanitized.len() > MAX_LEN {
        // Find a valid UTF-8 char boundary at or before MAX_LEN bytes
        let cutoff = sanitized
            .char_indices()
            .take_while(|(idx, _)| *idx < MAX_LEN)
            .last()
            .map(|(idx, ch)| idx + ch.len_utf8())
            .unwrap_or(0);
        sanitized.truncate(cutoff);
        sanitized = sanitized.trim_end().to_string();
    }

    if sanitized.is_empty() {
        "Unknown Location".to_string()
    } else {
        sanitized
    }
}

/// Builds the Bedrock output path and level name for a given bounding box.
/// Combines area name lookup, sanitization, and path construction.
pub fn build_bedrock_output(bbox: &LLBBox, output_dir: PathBuf) -> (PathBuf, String) {
    let area_name = get_area_name_for_bedrock(bbox);
    let safe_name = sanitize_for_filename(&area_name);
    let filename = format!("Arnis {safe_name}.mcworld");
    let lvl_name = format!("Arnis World: {safe_name}");
    (output_dir.join(&filename), lvl_name)
}

/// Creates a new Java Edition world in the given base directory.
///
/// Generates a unique "Arnis World N" name, creates the directory structure
/// (with a `region/` subdirectory), writes the region template, level.dat
/// (with updated name, timestamp, and spawn position), and icon.png.
///
/// Returns the full path to the newly created world directory.
pub fn create_new_world(base_path: &Path, data_version: i32) -> Result<String, String> {
    // Generate a unique world name with proper counter
    // Check for both "Arnis World X" and "Arnis World X: Location" patterns
    let mut counter: i32 = 1;
    let unique_name: String = loop {
        let candidate_name: String = format!("Arnis World {counter}");
        let candidate_path: PathBuf = base_path.join(&candidate_name);

        // Check for exact match (no location suffix)
        let exact_match_exists = candidate_path.exists();

        // Check for worlds with location suffix (Arnis World X: Location)
        let location_pattern = format!("Arnis World {counter}: ");
        let location_match_exists = fs::read_dir(base_path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .any(|name| name.starts_with(&location_pattern))
            })
            .unwrap_or(false);

        if !exact_match_exists && !location_match_exists {
            break candidate_name;
        }
        counter += 1;
    };

    let new_world_path: PathBuf = base_path.join(&unique_name);
    scaffold_world(&new_world_path, &unique_name, data_version)
}

/// Scaffold a Java world DIRECTLY at `world_path` — no "Arnis World N" subfolder, no uniqueness
/// counter. Region files land straight in `world_path/region/`. For CLI/scripted generation where
/// the caller's `--output-dir` (e.g. a version-named folder like `Bucharest-v9`) already IS the
/// intended world folder, so nesting another auto-numbered world inside it is unwanted — the caller
/// owns naming/versioning via the directory it passes in. Overwrites any existing content at that
/// path (the caller is expected to pick a fresh/intentional directory per generation).
pub fn create_world_at(world_path: &Path, data_version: i32) -> Result<String, String> {
    let level_name = world_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Arnis World")
        .to_string();
    scaffold_world(world_path, &level_name, data_version)
}

/// Shared world-scaffold body (region template, level.dat, icon.png) used by both
/// `create_new_world` (nested, auto-numbered) and `create_world_at` (direct path).
fn scaffold_world(
    new_world_path: &Path,
    unique_name: &str,
    data_version: i32,
) -> Result<String, String> {
    // Create the new world directory structure
    fs::create_dir_all(new_world_path.join("region"))
        .map_err(|e| format!("Failed to create world directory: {e}"))?;

    // Copy the region template file, restamped to this run's DataVersion.
    //
    // The template is a full 1024-chunk region baked at one fixed DataVersion. Any of its
    // chunks the generator does not overwrite survive into the finished world, so copying
    // it verbatim leaves a world holding TWO different DataVersions — the writer's and the
    // template's. Minecraft then runs its DataFixer over half the world, which is exactly
    // the "loads and then quietly misbehaves" outcome the version table exists to prevent.
    const REGION_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/region.template");
    let region_path = new_world_path.join("region").join("r.0.0.mca");
    let stamped = restamp_region_data_version(REGION_TEMPLATE, data_version)
        .unwrap_or_else(|| REGION_TEMPLATE.to_vec());
    fs::write(&region_path, stamped).map_err(|e| format!("Failed to create region file: {e}"))?;

    // Add the level.dat file
    const LEVEL_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/level.dat");

    // Decompress the gzipped level.template
    let mut decoder = GzDecoder::new(LEVEL_TEMPLATE);
    let mut decompressed_data = Vec::new();
    decoder
        .read_to_end(&mut decompressed_data)
        .map_err(|e| format!("Failed to decompress level.template: {e}"))?;

    // Parse the decompressed NBT data
    let mut level_data: Value = fastnbt::from_bytes(&decompressed_data)
        .map_err(|e| format!("Failed to parse level.dat template: {e}"))?;

    // Modify the LevelName, LastPlayed and player position fields
    if let Value::Compound(ref mut root) = level_data {
        if let Some(Value::Compound(ref mut data)) = root.get_mut("Data") {
            // Update LevelName
            data.insert(
                "LevelName".to_string(),
                Value::String(unique_name.to_string()),
            );

            // level.dat's version is deliberately NOT restamped.
            //
            // It describes the format this FILE is written in, not the version we are
            // targeting, and the bundled template is a 1.21.x-era level.dat. Claiming a
            // newer version made Minecraft skip the DataFixer that migrates it — 26.1.2
            // then looked for worldgen settings in the newer location, did not find them,
            // and the world failed to load ("invalid or corrupted save data"). Leaving the
            // template's own version lets the game upgrade the file properly.

            // Update LastPlayed to the current Unix time in milliseconds
            let current_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| format!("Failed to get current time: {e}"))?;
            let current_time_millis = current_time.as_millis() as i64;
            data.insert("LastPlayed".to_string(), Value::Long(current_time_millis));

            // Update player position and rotation
            if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
                if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
                    if pos.len() < 3 {
                        return Err(
                            "Invalid level.dat template: Player Pos list has fewer than 3 elements"
                                .to_string(),
                        );
                    }
                    if let Value::Double(ref mut x) = pos[0] {
                        *x = -5.0;
                    }
                    if let Value::Double(ref mut y) = pos[1] {
                        *y = -61.0;
                    }
                    if let Value::Double(ref mut z) = pos[2] {
                        *z = -5.0;
                    }
                }

                if let Some(Value::List(ref mut rot)) = player.get_mut("Rotation") {
                    if rot.is_empty() {
                        return Err(
                            "Invalid level.dat template: Player Rotation list is empty".to_string()
                        );
                    }
                    if let Value::Float(ref mut x) = rot[0] {
                        *x = -45.0;
                    }
                }
            }
        }
    }

    // Serialize the updated NBT data back to bytes
    let serialized_level_data: Vec<u8> = fastnbt::to_bytes(&level_data)
        .map_err(|e| format!("Failed to serialize updated level.dat: {e}"))?;

    // Compress the serialized data back to gzip
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized_level_data)
        .map_err(|e| format!("Failed to compress updated level.dat: {e}"))?;
    let compressed_level_data = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize compression for level.dat: {e}"))?;

    // Write the level.dat file
    fs::write(new_world_path.join("level.dat"), compressed_level_data)
        .map_err(|e| format!("Failed to create level.dat file: {e}"))?;

    // Add the icon.png file
    const ICON_TEMPLATE: &[u8] = include_bytes!("../assets/minecraft/icon.png");
    fs::write(new_world_path.join("icon.png"), ICON_TEMPLATE)
        .map_err(|e| format!("Failed to create icon.png file: {e}"))?;

    Ok(new_world_path.display().to_string())
}

/// Rewrite every chunk's `DataVersion` in a raw region file.
///
/// Patches the 4 bytes after the `DataVersion` NBT tag rather than round-tripping the
/// whole chunk through a parser: the tag is `TAG_Int` (0x03) + a 2-byte name length +
/// the name, so the value sits at a known offset once the name is found. Chunks are
/// zlib-framed individually, so each is decompressed, patched and recompressed in place.
///
/// Returns `None` if the region does not look like one we can safely patch, so the caller
/// falls back to the template as shipped rather than writing something malformed.
fn restamp_region_data_version(region: &[u8], data_version: i32) -> Option<Vec<u8>> {
    use flate2::read::ZlibDecoder;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;

    const HEADER: usize = 8192;
    const TAG: &[u8] = b" DataVersion";
    if region.len() < HEADER {
        return None;
    }
    // Rebuild the file: the header is rewritten as chunks move, since a patched chunk can
    // change length (compression is not size-stable).
    let mut header = region[..HEADER].to_vec();
    let mut body: Vec<u8> = Vec::with_capacity(region.len());
    let mut next_sector = (HEADER / 4096) as u32;

    for i in 0..1024 {
        let e = i * 4;
        let offset = u32::from_be_bytes([0, region[e], region[e + 1], region[e + 2]]) as usize;
        let sectors = region[e + 3];
        if offset == 0 || sectors == 0 {
            continue;
        }
        let start = offset * 4096;
        if start + 5 > region.len() {
            return None;
        }
        let len = u32::from_be_bytes([
            region[start],
            region[start + 1],
            region[start + 2],
            region[start + 3],
        ]) as usize;
        let compression = region[start + 4];
        if compression != 2 || len == 0 || start + 4 + len > region.len() {
            return None; // only zlib chunks are handled; anything else stays untouched
        }
        let payload = &region[start + 5..start + 4 + len];

        let mut raw = Vec::new();
        std::io::Read::read_to_end(&mut ZlibDecoder::new(payload), &mut raw).ok()?;
        let pos = raw.windows(TAG.len()).position(|w| w == TAG)?;
        let v = pos + TAG.len();
        if v + 4 > raw.len() {
            return None;
        }
        raw[v..v + 4].copy_from_slice(&data_version.to_be_bytes());

        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut enc, &raw).ok()?;
        let packed = enc.finish().ok()?;

        // Append at the next free sector and update this entry.
        let chunk_len = packed.len() + 1;
        let total = 4 + chunk_len;
        let used_sectors = total.div_ceil(4096);
        body.extend_from_slice(&(chunk_len as u32).to_be_bytes());
        body.push(2);
        body.extend_from_slice(&packed);
        body.resize(body.len() + (used_sectors * 4096 - total), 0);

        let off = next_sector.to_be_bytes();
        header[e] = off[1];
        header[e + 1] = off[2];
        header[e + 2] = off[3];
        header[e + 3] = u8::try_from(used_sectors).ok()?;
        next_sector += used_sectors as u32;
    }

    let mut out = header;
    out.extend_from_slice(&body);
    Some(out)
}

/// Name of the bundled Java datapack that extends the Overworld build height.
pub const TALL_DATAPACK_NAME: &str = "arnis_tall";

/// Install the extended-height datapack into a Java world and register it in
/// `level.dat`'s `Data.DataPacks.Enabled` so it auto-activates on first load.
///
/// The dimension geometry comes from the [`HeightProfile`] — the world declares exactly
/// the range its terrain needs, not a fixed 4064-tall preset. The three bundled JSON
/// templates are kept because they encode schema differences that must not be invented:
/// the base `data/` tree uses the flat dimension_type schema (formats 61-88, i.e.
/// 1.21.4-1.21.10) while the overlays carry the attributes schema for the 1.21.11 era
/// (formats 90-100) and 26.1.x (format 101.x), which are mutually incompatible. Only
/// `min_y` / `height` / `logical_height` are rewritten from the profile.
///
/// A vanilla profile installs nothing and returns `Ok(false)`: vanilla geometry needs no
/// datapack, and shipping one would saddle the world with an experimental-features prompt
/// and a pack it can never remove for no gain.
///
/// MUST be called before the first chunk is written. Chunks are serialised against this
/// geometry; applying it afterwards means the existing chunks were written at a different
/// height.
pub fn install_height_datapack(
    world_path: &Path,
    profile: &crate::height_profile::HeightProfile,
    caps: &crate::mc_version::VersionCaps,
) -> Result<bool, String> {
    profile.validate().map_err(|e| {
        format!("refusing to install a datapack for an invalid height profile: {e}")
    })?;
    if profile.is_vanilla() {
        return Ok(false);
    }
    if caps.datapack_schema == crate::mc_version::DatapackSchema::Modern
        && caps.datapack_format.is_none()
    {
        return Err(format!(
            "Minecraft {} needs the 26.x datapack schema, but no VERIFIED pack_format for              it is recorded in assets/mc_versions.json. Read pack_format out of a data              pack that version loads and add it, rather than guessing: a wrong value makes              the world refuse to open.",
            caps.id
        ));
    }
    install_datapack_files(world_path, profile, caps)?;
    Ok(true)
}

fn install_datapack_files(
    world_path: &Path,
    profile: &crate::height_profile::HeightProfile,
    caps: &crate::mc_version::VersionCaps,
) -> Result<(), String> {
    let schema = caps.datapack_schema;
    const PACK_MCMETA: &[u8] = include_bytes!("../assets/minecraft/datapack_tall/pack.mcmeta");
    const OVERWORLD_JSON: &[u8] = include_bytes!(
        "../assets/minecraft/datapack_tall/data/minecraft/dimension_type/overworld.json"
    );
    const OVERLAY_ATTRIBUTES_JSON: &[u8] = include_bytes!(
        "../assets/minecraft/datapack_tall/overlay_attributes/data/minecraft/dimension_type/overworld.json"
    );
    const OVERLAY_2601_JSON: &[u8] = include_bytes!(
        "../assets/minecraft/datapack_tall/overlay_2601/data/minecraft/dimension_type/overworld.json"
    );

    let dp_root = world_path.join("datapacks").join(TALL_DATAPACK_NAME);

    // WHICH FILES TO WRITE depends on the target's datapack schema.
    //
    // The legacy tree is one base dimension_type plus two overlays, selected by the
    // format ranges in pack.mcmeta — the 1.21.4-1.21.10 shape. 26.x changed the metadata
    // schema itself (decimal pack_format), so that whole structure is rejected there with
    // "Failed to read pack metadata" and the world will not open at all. For a modern
    // target we write ONE dimension_type, already in that era's schema, and no overlays.
    let dim_files: &[(&str, &[u8])] = match schema {
        crate::mc_version::DatapackSchema::Modern => &[("", OVERLAY_2601_JSON)],
        crate::mc_version::DatapackSchema::Legacy => &[
            ("", OVERWORLD_JSON),
            ("overlay_attributes", OVERLAY_ATTRIBUTES_JSON),
            ("overlay_2601", OVERLAY_2601_JSON),
        ],
    };
    for &(overlay, bytes) in dim_files {
        let mut dim_dir = dp_root.clone();
        if !overlay.is_empty() {
            dim_dir.push(overlay);
        }
        let dim_dir = dim_dir
            .join("data")
            .join("minecraft")
            .join("dimension_type");
        fs::create_dir_all(&dim_dir)
            .map_err(|e| format!("Failed to create datapack directories: {e}"))?;
        let json = dimension_json_for(bytes, profile)?;
        fs::write(dim_dir.join("overworld.json"), json)
            .map_err(|e| format!("Failed to write overworld.json: {e}"))?;
    }

    fs::write(
        dp_root.join("pack.mcmeta"),
        pack_mcmeta_for(PACK_MCMETA, profile, caps)?,
    )
    .map_err(|e| format!("Failed to write pack.mcmeta: {e}"))?;

    register_tall_datapack_in_level_dat(world_path)?;

    Ok(())
}

/// Rewrite the bundled `pack.mcmeta` for this profile.
///
/// The format range and the overlay entries come from the template, because they encode
/// which schema era each overlay serves — values we have verified by shipping them, and
/// must not invent. Only the human-readable description is rewritten, so a user opening
/// the world's datapack list can see what range it declares. The range is asserted
/// against the checked-in envelope so template and table can never silently diverge.
///
/// One conditional edit: at pack format 82+ the overlay `formats` key is deprecated, so for
/// a target verified to be at or above that it is dropped (see below).
fn pack_mcmeta_for(
    template: &[u8],
    profile: &crate::height_profile::HeightProfile,
    caps: &crate::mc_version::VersionCaps,
) -> Result<Vec<u8>, String> {
    let description = format!(
        "Arnis build height Y {}..{} ({} blocks)",
        profile.min_y,
        profile.max_y(),
        profile.height
    );
    // 26.x: a single DECIMAL format, no supported_formats block, no overlays. Written
    // from scratch rather than patched, because the legacy template's extra keys are
    // exactly what that version's codec rejects.
    if caps.datapack_schema == crate::mc_version::DatapackSchema::Modern {
        let fmt = caps
            .datapack_format
            .ok_or_else(|| format!("no verified pack_format for {}", caps.id))?;
        let doc = serde_json::json!({
            "pack": {
                "description": description,
                "pack_format": fmt,
                "min_format": fmt,
                "max_format": fmt,
            }
        });
        return serde_json::to_vec_pretty(&doc)
            .map_err(|e| format!("failed to serialise pack.mcmeta: {e}"));
    }
    let mut doc: serde_json::Value = serde_json::from_slice(template)
        .map_err(|e| format!("bundled pack.mcmeta is not valid JSON: {e}"))?;
    let (fmt, lo, hi) = crate::mc_version::pack_format_envelope();
    if let Some(pack) = doc.get_mut("pack").and_then(|p| p.as_object_mut()) {
        let declared = pack.get("pack_format").and_then(|v| v.as_i64());
        if declared != Some(fmt as i64) {
            return Err(format!(
                "bundled pack.mcmeta declares pack_format {declared:?} but the version \
                 table's envelope says {fmt} ({lo}..={hi}) — they must agree"
            ));
        }
        pack.insert("description".into(), description.into());
    }
    // Pack format 82 deprecated the overlay `formats` key in favour of
    // `min_format`/`max_format` — a 1.21.9+ server logs
    //   Overlay "overlay_attributes" key formats is deprecated starting from pack format 82
    // on every start. The template carries BOTH spellings, so for a target we know sits at
    // 82 or above we drop the old one and the warning goes with it. Targets below 82 (and
    // any target whose format we have not verified) keep `formats`, because that is the
    // only overlay selector those versions understand.
    if caps.datapack_format.is_some_and(|f| f >= 82.0) {
        if let Some(entries) = doc
            .get_mut("overlays")
            .and_then(|o| o.get_mut("entries"))
            .and_then(|e| e.as_array_mut())
        {
            for entry in entries.iter_mut() {
                if let Some(obj) = entry.as_object_mut() {
                    obj.remove("formats");
                }
            }
        }
    }
    serde_json::to_vec_pretty(&doc).map_err(|e| format!("failed to serialise pack.mcmeta: {e}"))
}

/// Rewrite a bundled `dimension_type` template with this profile's geometry.
///
/// Only `min_y`, `height` and `logical_height` are touched — every other key in the
/// template encodes schema details for its Minecraft era (the attributes block, the
/// timelines tag, the monster-spawn rules) that must not be invented here.
fn dimension_json_for(
    template: &[u8],
    profile: &crate::height_profile::HeightProfile,
) -> Result<Vec<u8>, String> {
    let mut doc: serde_json::Value = serde_json::from_slice(template)
        .map_err(|e| format!("bundled dimension_type template is not valid JSON: {e}"))?;
    let obj = doc
        .as_object_mut()
        .ok_or_else(|| "bundled dimension_type template is not a JSON object".to_string())?;
    obj.insert("min_y".into(), profile.min_y.into());
    obj.insert("height".into(), profile.height.into());
    // logical_height caps where the dimension lets you build/teleport; the terrain range
    // is the whole point here, so it tracks height.
    obj.insert("logical_height".into(), profile.height.into());
    serde_json::to_vec_pretty(&doc).map_err(|e| format!("failed to serialise dimension_type: {e}"))
}

/// Appends the pack entry if missing. Expected to run on a fresh level.dat
/// template whose Enabled list starts with `["vanilla"]`, so the appended
/// entry naturally lands after vanilla and our dimension_type override wins.
fn register_tall_datapack_in_level_dat(world_path: &Path) -> Result<(), String> {
    let level_path = world_path.join("level.dat");
    if !level_path.exists() {
        return Err(format!("level.dat not found at {level_path:?}"));
    }

    let raw = fs::read(&level_path).map_err(|e| format!("Failed to read level.dat: {e}"))?;
    let mut decoder = GzDecoder::new(raw.as_slice());
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| format!("Failed to decompress level.dat: {e}"))?;

    let mut root: Value = fastnbt::from_bytes(&decompressed)
        .map_err(|e| format!("Failed to parse level.dat NBT: {e}"))?;

    let entry = format!("file/{TALL_DATAPACK_NAME}");

    {
        let data = match root {
            Value::Compound(ref mut r) => match r.get_mut("Data") {
                Some(Value::Compound(ref mut d)) => d,
                _ => return Err("level.dat missing Data compound".to_string()),
            },
            _ => return Err("level.dat root is not a compound".to_string()),
        };

        let data_packs = data
            .entry("DataPacks".to_string())
            .or_insert_with(|| Value::Compound(Default::default()));
        let Value::Compound(ref mut dp) = data_packs else {
            return Err("level.dat Data.DataPacks is not a compound".to_string());
        };

        let enabled = dp
            .entry("Enabled".to_string())
            .or_insert_with(|| Value::List(Vec::new()));
        let Value::List(ref mut list) = enabled else {
            return Err("level.dat Data.DataPacks.Enabled is not a list".to_string());
        };

        let already_enabled = list
            .iter()
            .any(|v| matches!(v, Value::String(s) if s == &entry));
        if !already_enabled {
            list.push(Value::String(entry));
        }
    }

    let serialized =
        fastnbt::to_bytes(&root).map_err(|e| format!("Failed to serialize level.dat: {e}"))?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized)
        .map_err(|e| format!("Failed to compress level.dat: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize level.dat compression: {e}"))?;
    fs::write(&level_path, compressed).map_err(|e| format!("Failed to write level.dat: {e}"))?;

    Ok(())
}

/// Sets the player spawn point in an existing Java Edition level.dat file.
///
/// Updates both the world spawn point (SpawnX/SpawnY/SpawnZ) and the player
/// position if a Player compound exists. Callers derive `spawn_y` from the
/// generated terrain so the player spawns above ground even in extended-height
/// worlds where terrain may reach Y≈2000.
pub fn set_spawn_in_level_dat(
    world_path: &Path,
    spawn_x: i32,
    spawn_y: i32,
    spawn_z: i32,
) -> Result<(), String> {
    let level_path = world_path.join("level.dat");
    if !level_path.exists() {
        return Err(format!("level.dat not found at {level_path:?}"));
    }

    // Read and decompress
    let level_data = fs::read(&level_path).map_err(|e| format!("Failed to read level.dat: {e}"))?;

    let mut decoder = GzDecoder::new(level_data.as_slice());
    let mut decompressed_data = Vec::new();
    decoder
        .read_to_end(&mut decompressed_data)
        .map_err(|e| format!("Failed to decompress level.dat: {e}"))?;

    let mut nbt_data: Value = fastnbt::from_bytes(&decompressed_data)
        .map_err(|e| format!("Failed to parse level.dat NBT data: {e}"))?;

    // Update spawn point
    let data = match nbt_data {
        Value::Compound(ref mut root) => match root.get_mut("Data") {
            Some(Value::Compound(ref mut data)) => data,
            _ => {
                return Err(
                    "Invalid level.dat structure: missing or non-compound \"Data\" section"
                        .to_string(),
                );
            }
        },
        _ => {
            return Err(
                "Invalid level.dat structure: root NBT value is not a compound".to_string(),
            );
        }
    };

    data.insert("SpawnX".to_string(), Value::Int(spawn_x));
    data.insert("SpawnY".to_string(), Value::Int(spawn_y));
    data.insert("SpawnZ".to_string(), Value::Int(spawn_z));

    // Update player position if Player compound exists
    if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
        if let Some(Value::List(ref mut pos)) = player.get_mut("Pos") {
            if pos.len() >= 3 {
                if let Some(Value::Double(ref mut pos_x)) = pos.get_mut(0) {
                    *pos_x = spawn_x as f64;
                }
                if let Some(Value::Double(ref mut pos_y)) = pos.get_mut(1) {
                    *pos_y = spawn_y as f64;
                }
                if let Some(Value::Double(ref mut pos_z)) = pos.get_mut(2) {
                    *pos_z = spawn_z as f64;
                }
            }
        }
    }

    // Serialize, compress, and write back
    let serialized_data = fastnbt::to_bytes(&nbt_data)
        .map_err(|e| format!("Failed to serialize updated level.dat: {e}"))?;

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized_data)
        .map_err(|e| format!("Failed to compress updated level.dat: {e}"))?;
    let compressed_data = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize compression for level.dat: {e}"))?;

    fs::write(&level_path, compressed_data)
        .map_err(|e| format!("Failed to write updated level.dat: {e}"))?;

    Ok(())
}

/// Writes the player game mode and initial time of day into an existing Java
/// Edition level.dat (GameType + DayTime, plus the Player's playerGameType).
pub fn apply_java_world_settings(
    world_path: &Path,
    game_mode: crate::args::GameMode,
    world_time: i64,
) -> Result<(), String> {
    let level_path = world_path.join("level.dat");
    if !level_path.exists() {
        return Err(format!("level.dat not found at {level_path:?}"));
    }

    let raw = fs::read(&level_path).map_err(|e| format!("Failed to read level.dat: {e}"))?;
    let mut decoder = GzDecoder::new(raw.as_slice());
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| format!("Failed to decompress level.dat: {e}"))?;

    let mut root: Value = fastnbt::from_bytes(&decompressed)
        .map_err(|e| format!("Failed to parse level.dat NBT: {e}"))?;

    {
        let data = match root {
            Value::Compound(ref mut r) => match r.get_mut("Data") {
                Some(Value::Compound(ref mut d)) => d,
                _ => return Err("level.dat missing Data compound".to_string()),
            },
            _ => return Err("level.dat root is not a compound".to_string()),
        };

        let game_type = game_mode.java_game_type();
        data.insert("GameType".to_string(), Value::Int(game_type));
        data.insert("DayTime".to_string(), Value::Long(world_time));
        if let Some(Value::Compound(ref mut player)) = data.get_mut("Player") {
            player.insert("playerGameType".to_string(), Value::Int(game_type));
        }
    }

    let serialized =
        fastnbt::to_bytes(&root).map_err(|e| format!("Failed to serialize level.dat: {e}"))?;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(&serialized)
        .map_err(|e| format!("Failed to compress level.dat: {e}"))?;
    let compressed = encoder
        .finish()
        .map_err(|e| format!("Failed to finalize level.dat compression: {e}"))?;
    fs::write(&level_path, compressed).map_err(|e| format!("Failed to write level.dat: {e}"))?;

    Ok(())
}
