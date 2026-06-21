#![allow(dead_code)] // Wired into tree placement in a later step.
//! Sponge `.schem` loader. Parses a gzipped WorldEdit/Sponge schematic (v2 or v3)
//! into a list of `(dx, dy, dz, Block)` voxels for stamping trees. Reuses `fastnbt`
//! and `flate2` (already dependencies). Only logs and leaves are kept; ground cover,
//! vines, cocoa, pale-garden blocks, air, and unknown blocks are dropped.

use std::collections::HashMap;
use std::io::Read;

use fastnbt::Value;

use crate::block_definitions::{
    Block, ACACIA_LEAVES, ACACIA_LOG, AZALEA_LEAVES, BIRCH_LEAVES, BIRCH_LOG, CHERRY_LEAVES,
    CHERRY_LOG, DARK_OAK_LEAVES, DARK_OAK_LOG, JUNGLE_LEAVES, JUNGLE_LOG, MANGROVE_LEAVES,
    MANGROVE_LOG, OAK_LEAVES, OAK_LOG, SPRUCE_LEAVES, SPRUCE_LOG,
};
use crate::world_editor::WorldEditor;

/// A parsed schematic: dimensions plus the non-air log/leaf voxels. The origin is the
/// schematic's min corner (`x` in `0..width`, `y` in `0..height`, `z` in `0..length`).
pub struct Schematic {
    pub width: i32,
    pub height: i32,
    pub length: i32,
    pub voxels: Vec<(i32, i32, i32, Block)>,
}

/// Map a Minecraft block-state string (e.g. `minecraft:oak_log[axis=y]`) to one of our
/// blocks. Returns `None` for anything we intentionally drop (air, ground cover, vines,
/// cocoa, pale-garden blocks) or do not recognise, so those voxels are simply skipped.
pub fn map_block(name: &str) -> Option<Block> {
    let base = name
        .split('[')
        .next()
        .unwrap_or(name)
        .trim_start_matches("minecraft:");
    let block = match base {
        "oak_log" | "oak_wood" => OAK_LOG,
        "birch_log" | "birch_wood" => BIRCH_LOG,
        "spruce_log" | "spruce_wood" => SPRUCE_LOG,
        "dark_oak_log" | "dark_oak_wood" => DARK_OAK_LOG,
        "jungle_log" | "jungle_wood" => JUNGLE_LOG,
        "acacia_log" | "acacia_wood" => ACACIA_LOG,
        "cherry_log" | "cherry_wood" => CHERRY_LOG,
        "mangrove_log" | "mangrove_wood" => MANGROVE_LOG,
        "oak_leaves" => OAK_LEAVES,
        "birch_leaves" => BIRCH_LEAVES,
        "spruce_leaves" => SPRUCE_LEAVES,
        "dark_oak_leaves" => DARK_OAK_LEAVES,
        "jungle_leaves" => JUNGLE_LEAVES,
        "acacia_leaves" => ACACIA_LEAVES,
        "cherry_leaves" => CHERRY_LEAVES,
        "mangrove_leaves" => MANGROVE_LEAVES,
        "azalea_leaves" | "flowering_azalea_leaves" => AZALEA_LEAVES,
        _ => return None,
    };
    Some(block)
}

/// Decode a Sponge `BlockData` byte stream: LEB128 varint palette indices.
fn decode_varints(data: &[u8]) -> Vec<i32> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let mut val: i32 = 0;
        let mut shift = 0u32;
        loop {
            let byte = data[i];
            i += 1;
            val |= i32::from(byte & 0x7F) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if i >= data.len() {
                break;
            }
        }
        out.push(val);
    }
    out
}

fn as_compound(v: &Value) -> Option<&HashMap<String, Value>> {
    match v {
        Value::Compound(c) => Some(c),
        _ => None,
    }
}

fn short_field(c: &HashMap<String, Value>, k: &str) -> Result<i32, String> {
    match c.get(k) {
        Some(Value::Short(s)) => Ok(i32::from(*s)),
        Some(Value::Int(i)) => Ok(*i),
        _ => Err(format!("schem: missing short field {k}")),
    }
}

/// Load a gzipped Sponge `.schem` (format v2 or v3) and return its log/leaf voxels.
pub fn load_schem(gz_bytes: &[u8]) -> Result<Schematic, String> {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(gz_bytes)
        .read_to_end(&mut raw)
        .map_err(|e| format!("schem: gunzip failed: {e}"))?;
    let root: Value = fastnbt::from_bytes(&raw).map_err(|e| format!("schem: nbt parse: {e}"))?;
    let root_c = as_compound(&root).ok_or("schem: root not a compound")?;

    // Sponge v3 nests everything under "Schematic"; v2 keeps it at the root.
    let scm = root_c
        .get("Schematic")
        .and_then(as_compound)
        .unwrap_or(root_c);

    let width = short_field(scm, "Width")?;
    let height = short_field(scm, "Height")?;
    let length = short_field(scm, "Length")?;
    if width <= 0 || height <= 0 || length <= 0 {
        return Err("schem: non-positive dimensions".into());
    }

    // v3: Blocks { Palette, Data }; v2: root-level Palette + BlockData.
    let (palette_v, data_v) = match scm.get("Blocks").and_then(as_compound) {
        Some(blocks) => (blocks.get("Palette"), blocks.get("Data")),
        None => (scm.get("Palette"), scm.get("BlockData")),
    };
    let palette = palette_v
        .and_then(as_compound)
        .ok_or("schem: missing Palette")?;

    // Palette maps block-state string -> index; invert to index -> our Block (drops skipped).
    let mut idx_to_block: HashMap<i32, Block> = HashMap::new();
    for (name, v) in palette {
        if let Value::Int(i) = v {
            if let Some(block) = map_block(name) {
                idx_to_block.insert(*i, block);
            }
        }
    }

    let data_bytes: Vec<u8> = match data_v {
        Some(Value::ByteArray(b)) => b.iter().map(|&x| x as u8).collect(),
        _ => return Err("schem: missing BlockData".into()),
    };
    let indices = decode_varints(&data_bytes);

    let wl = width * length;
    let mut voxels = Vec::new();
    for (i, &idx) in indices.iter().enumerate() {
        let i = i as i32;
        if let Some(block) = idx_to_block.get(&idx).cloned() {
            let x = i % width;
            let z = (i / width) % length;
            let y = i / wl;
            voxels.push((x, y, z, block));
        }
    }

    Ok(Schematic {
        width,
        height,
        length,
        voxels,
    })
}

/// Rotate a schematic cell offset `(x, z)` by `k` quarter-turns clockwise (k in 0..=3)
/// around the footprint, returning the rotated `(x, z)` offset. `w`/`l` are the
/// footprint width and length. Lets one schematic render in four orientations; for a
/// 90/270 turn the result spans `0..l` by `0..w`.
pub fn rotate_xz(x: i32, z: i32, w: i32, l: i32, k: u8) -> (i32, i32) {
    match k & 3 {
        0 => (x, z),
        1 => (l - 1 - z, x),
        2 => (w - 1 - x, l - 1 - z),
        _ => (z, w - 1 - x),
    }
}

/// Stamp a schematic into the world with its footprint centred on `(anchor_x, anchor_z)`,
/// the base row (`y = 0`) at `base_y`, rotated by `rot` quarter-turns. Writes only into
/// AIR via place-if-absent, so it never overwrites buildings, roads, water, or terrain;
/// air voxels were already dropped at load time. Pure function of its inputs, so the same
/// tree renders identically from any tile (seam-safe).
pub fn place_schematic_tree(
    editor: &mut WorldEditor,
    schem: &Schematic,
    anchor_x: i32,
    anchor_z: i32,
    base_y: i32,
    rot: u8,
) {
    // A 90/270 turn swaps the footprint width/length.
    let (fw, fl) = if rot & 1 == 0 {
        (schem.width, schem.length)
    } else {
        (schem.length, schem.width)
    };
    let cx = (fw - 1) / 2;
    let cz = (fl - 1) / 2;
    for &(vx, vy, vz, block) in &schem.voxels {
        let (rx, rz) = rotate_xz(vx, vz, schem.width, schem.length, rot);
        editor.set_block_if_absent_absolute(
            block,
            anchor_x + rx - cx,
            base_y + vy,
            anchor_z + rz - cz,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_corners_and_bounds() {
        let (w, l) = (3, 5);
        assert_eq!(rotate_xz(0, 0, w, l, 0), (0, 0));
        assert_eq!(rotate_xz(0, 0, w, l, 2), (w - 1, l - 1));
        for x in 0..w {
            for z in 0..l {
                let (rx, rz) = rotate_xz(x, z, w, l, 1);
                assert!((0..l).contains(&rx) && (0..w).contains(&rz));
                let (rx3, rz3) = rotate_xz(x, z, w, l, 3);
                assert!((0..l).contains(&rx3) && (0..w).contains(&rz3));
            }
        }
    }

    #[test]
    fn varint_decode() {
        assert_eq!(decode_varints(&[0x00]), vec![0]);
        assert_eq!(decode_varints(&[0x7F]), vec![127]);
        assert_eq!(decode_varints(&[0x80, 0x01]), vec![128]);
        assert_eq!(decode_varints(&[0xAC, 0x02]), vec![300]);
        assert_eq!(decode_varints(&[0x01, 0x80, 0x01]), vec![1, 128]);
    }

    #[test]
    fn block_mapping_keeps_logs_and_leaves() {
        assert!(map_block("minecraft:oak_log[axis=y]").is_some());
        assert!(map_block("minecraft:spruce_leaves[distance=7,persistent=false]").is_some());
        assert!(map_block("oak_wood").is_some());
        assert!(map_block("minecraft:flowering_azalea_leaves").is_some());
    }

    #[test]
    fn block_mapping_drops_unwanted() {
        assert!(map_block("minecraft:air").is_none());
        assert!(map_block("minecraft:pale_oak_log").is_none());
        assert!(map_block("minecraft:creaking_heart").is_none());
        assert!(map_block("minecraft:open_eyeblossom").is_none());
        assert!(map_block("minecraft:short_grass").is_none());
        assert!(map_block("minecraft:vine").is_none());
        assert!(map_block("minecraft:cocoa[age=2]").is_none());
    }

    #[test]
    #[ignore = "set ARNIS_SCHEM_TEST to a real .schem path"]
    fn smoke_load_real_file() {
        let path = std::env::var("ARNIS_SCHEM_TEST").expect("ARNIS_SCHEM_TEST");
        let bytes = std::fs::read(&path).expect("read schem");
        let s = load_schem(&bytes).expect("parse schem");
        assert!(s.width > 0 && s.height > 0 && s.length > 0);
        assert!(!s.voxels.is_empty(), "no log/leaf voxels parsed");
        eprintln!(
            "loaded {}x{}x{} ({} log/leaf voxels) from {path}",
            s.width,
            s.height,
            s.length,
            s.voxels.len()
        );
    }
}
