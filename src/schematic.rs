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
    MANGROVE_LOG, OAK_LEAVES, OAK_LOG, SPRUCE_LEAVES, SPRUCE_LOG, WATER,
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
        // Stripped trunks: the regional schems use stripped logs as trunk wood. We have no
        // stripped block ids, so fall back to the matching base log (fills the trunk; the
        // bark/strip tint is lost). Remapping here keeps trunks solid instead of holed.
        "stripped_oak_log" | "stripped_oak_wood" => OAK_LOG,
        "stripped_birch_log" | "stripped_birch_wood" => BIRCH_LOG,
        "stripped_spruce_log" | "stripped_spruce_wood" => SPRUCE_LOG,
        "stripped_dark_oak_log" | "stripped_dark_oak_wood" => DARK_OAK_LOG,
        "stripped_jungle_log" | "stripped_jungle_wood" => JUNGLE_LOG,
        "stripped_acacia_log" | "stripped_acacia_wood" => ACACIA_LOG,
        "stripped_cherry_log" | "stripped_cherry_wood" => CHERRY_LOG,
        "stripped_mangrove_log" | "stripped_mangrove_wood" => MANGROVE_LOG,
        // Pale-oak wood is used as a pale trunk/foliage in real regional trees (baobab,
        // eucalyptus, birch-likes), NOT the pale garden. The pale-garden *trees* are excluded
        // at the file level (curation); the pale *blocks* map to the palest stand-ins we have.
        "pale_oak_log" | "pale_oak_wood" | "stripped_pale_oak_log" | "stripped_pale_oak_wood" => {
            BIRCH_LOG
        }
        "pale_oak_leaves" => BIRCH_LEAVES,
        // Mangrove prop-roots / propagules.
        "mangrove_roots" | "muddy_mangrove_roots" => MANGROVE_LOG,
        "mangrove_propagule" => MANGROVE_LEAVES,
        // Bamboo clusters: no bamboo id, use the closest woody stand-in.
        "bamboo_block" | "stripped_bamboo_block" => JUNGLE_LOG,
        // Rare warped trunks (a few desert/exotic schems) -> dark trunk stand-in.
        "warped_stem" | "stripped_warped_stem" | "warped_hyphae" | "stripped_warped_hyphae" => {
            SPRUCE_LOG
        }
        // Everything else (air, ground cover, vines, cocoa, fences, and the pale-garden markers
        // creaking_heart / eyeblossom / pale_moss / pale_hanging_moss) is intentionally dropped.
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

    // Normalise so the lowest log/leaf sits at y=0. Schematics often include air padding
    // below the trunk; without this shift the tree stamps a few blocks above the ground
    // (the "floating on grass" artifact).
    if let Some(min_y) = voxels.iter().map(|v| v.1).min() {
        if min_y != 0 {
            for v in &mut voxels {
                v.1 -= min_y;
            }
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

/// Trunk-spacing grid. Snap `(x, z)` to the single designated trunk slot of its 3-block cell.
/// Every tree Arnis tries to spawn inside a cell collapses onto that one slot, so wide schematic
/// trees never stack trunks: at most one trunk per cell and any two trunks stay >= 1 empty block
/// apart (canopies still overlap, so a forest still closes). The jitter is restricted to `{0,1}`
/// so neighbouring slots stay >= 2 blocks apart. Pure function of `(x, z)` => identical from any
/// tile, so it never introduces a seam.
pub fn trunk_slot(x: i32, z: i32, scale: f64) -> (i32, i32) {
    // Wider spacing on small maps so trees aren't crowded; tighter at full scale (cities).
    let s: i32 = if scale < 0.3 {
        5
    } else if scale < 0.7 {
        4
    } else {
        3
    };
    trunk_slot_s(x, z, s)
}

/// Trunk slot with an explicit spacing `s` (blocks). The region pack uses this so spacing can be
/// density-aware per community. Quantizes (x,z) to the `s`-grid with a 1-bit hash jitter, so every
/// spawn in a cell collapses to one idempotent, seam-safe slot.
pub fn trunk_slot_s(x: i32, z: i32, s: i32) -> (i32, i32) {
    let s = s.max(1);
    let cx = x.div_euclid(s);
    let cz = z.div_euclid(s);
    let h =
        crate::land_cover::coord_hash(cx.wrapping_mul(0x1f1f) + 17, cz.wrapping_mul(0x2b2b) + 91);
    let jx = (h & 1) as i32;
    let jz = ((h >> 1) & 1) as i32;
    (cx * s + jx, cz * s + jz)
}

/// One of the eight trunk log types `map_block` can emit. Used to decide which footprint cells
/// get a downward "root" (only actual trunk columns, not leaf columns).
fn is_log(b: Block) -> bool {
    matches!(
        b,
        OAK_LOG
            | BIRCH_LOG
            | SPRUCE_LOG
            | DARK_OAK_LOG
            | JUNGLE_LOG
            | ACACIA_LOG
            | CHERRY_LOG
            | MANGROVE_LOG
    )
}

/// A log column counts as a ground-rooted TRUNK BASE (eligible for the downward root pass) only if
/// its lowest log is within this many blocks of the schem floor (y=0). Columns whose lowest log
/// sits higher are branch/canopy tips - rooting them would drop a log pillar from the canopy to the
/// ground (the screenshot "floating/pillar" artifact), so they are never rooted.
const ROOT_BASE_VY: i32 = 2;
/// Safety bound on how far a real trunk root may reach down (only bites pathological tile-seam
/// elevation deltas). Genuine trunk bases otherwise anchor straight to their own local ground.
const ROOT_MAX: i32 = 64;

/// Stamp a schematic into the world with its footprint centred on `(anchor_x, anchor_z)`,
/// the base row (`y = 0`) at `base_y`, rotated by `rot` quarter-turns. Writes only into
/// AIR via place-if-absent, so it never overwrites buildings, roads, water, or terrain;
/// air voxels were already dropped at load time. `y_offset` is the same offset used to build
/// `base_y` (so per-column ground can be recomputed for roots). Pure function of its inputs, so
/// the same tree renders identically from any tile (seam-safe).
#[allow(clippy::too_many_arguments)]
pub fn place_schematic_tree(
    editor: &mut WorldEditor,
    schem: &Schematic,
    anchor_x: i32,
    anchor_z: i32,
    base_y: i32,
    rot: u8,
    blacklist: &[Block],
    footprints: Option<&crate::floodfill_cache::BuildingFootprintBitmap>,
    y_offset: i32,
) {
    // A 90/270 turn swaps the footprint width/length.
    let (fw, fl) = if rot & 1 == 0 {
        (schem.width, schem.length)
    } else {
        (schem.length, schem.width)
    };
    let cx = (fw - 1) / 2;
    let cz = (fl - 1) / 2;
    // Lowest log row in the schem (the floor for the root-level test below).
    let min_log_vy = schem
        .voxels
        .iter()
        .filter(|&&(_, _, _, b)| is_log(b))
        .map(|&(_, vy, _, _)| vy)
        .min()
        .unwrap_or(0);
    // Per trunk column: the world Y of its lowest log + the log type, for the root pass below.
    let mut trunk_bottom: HashMap<(i32, i32), (i32, Block)> = HashMap::new();
    for &(vx, vy, vz, block) in &schem.voxels {
        let (rx, rz) = rotate_xz(vx, vz, schem.width, schem.length, rot);
        let wx = anchor_x + rx - cx;
        let wz = anchor_z + rz - cz;
        // Never put a tree block (trunk or canopy) inside a building footprint.
        if footprints.is_some_and(|f| f.contains(wx, wz)) {
            continue;
        }
        // Water handling for logs. A log on ACTUAL placed water is always skipped (it would sit in
        // the lake). For the ESA water MASK (carved later), skip ONLY a low root-level log - those
        // are the prop-roots that would float once the cell is carved. A HIGH trunk/canopy log that
        // merely crosses a mask cell is KEPT, otherwise the trunk gets a 1-block hole punched in it
        // (the "missing block" on bank/mangrove trees). Leaves always overhang freely.
        if is_log(block) {
            let root_level = vy <= min_log_vy + ROOT_BASE_VY;
            let over_water = editor.check_for_block(wx, 0, wz, Some(&[WATER]))
                || (root_level && crate::element_processing::tree::in_water_mask(wx, wz));
            if over_water {
                continue;
            }
        }
        // Overwrite terrain/grass (so grass does not poke through the trunk), but the
        // blacklist (buildings, water) is never overwritten.
        editor.set_block_absolute(block, wx, base_y + vy, wz, None, Some(blacklist));
        // Track the lowest log per column (roots anchor only real trunk columns).
        if is_log(block) {
            let wy = base_y + vy;
            trunk_bottom
                .entry((wx, wz))
                .and_modify(|e| {
                    if wy < e.0 {
                        *e = (wy, block);
                    }
                })
                .or_insert((wy, block));
        }
    }
    // Root pass: anchor ONLY genuine ground-rooted trunk columns (lowest log within ROOT_BASE_VY of
    // the schem floor) straight down to their OWN local ground, so off-center trunks / buttress legs
    // / mangrove prop-roots on a slope or seam are anchored instead of floating. High branch-tip log
    // columns are skipped entirely - extending them would drop a tall log pillar from the canopy to
    // the ground. The range is naturally EMPTY on flat/uphill ground, so a flat trunk never buries
    // its surface block. ROOT_MAX only caps pathological seam deltas.
    // The schem's lowest LOG row: a tree's logs can sit several blocks above the y=0 voxel when the
    // normalize floor is a hanging leaf/frond/root tip. A genuine ground-rooted trunk base is judged
    // relative to this lowest log, NOT the lowest voxel - otherwise bushy/drooping trees (trunk log
    // at vy>2) would be wrongly treated as "all branch" and never rooted (-> floating).
    let min_log_vy = trunk_bottom
        .values()
        .map(|(top, _)| top - base_y)
        .min()
        .unwrap_or(0);
    for ((wx, wz), (top, log)) in trunk_bottom {
        if top - base_y > min_log_vy + ROOT_BASE_VY {
            continue; // a higher branch / canopy log column, not a ground trunk base
        }
        if crate::element_processing::tree::in_water_mask(wx, wz) {
            continue; // never extend a root onto a cell the water carve will flood
        }
        let gy = editor.get_absolute_y(wx, y_offset, wz); // local terrain Y + same offset as base_y
        let from = (top - 1 - ROOT_MAX).max(gy);
        let to = top - 1;
        for wy in from..=to {
            editor.set_block_absolute(log, wx, wy, wz, None, Some(blacklist));
        }
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
        // Regional trunk vocabulary now maps to stand-ins instead of leaving holes.
        assert!(map_block("minecraft:stripped_jungle_log").is_some());
        assert!(map_block("minecraft:pale_oak_log").is_some()); // pale *wood* in real trees
        assert!(map_block("minecraft:mangrove_roots").is_some());
        assert!(map_block("minecraft:bamboo_block").is_some());
    }

    #[test]
    fn block_mapping_drops_unwanted() {
        assert!(map_block("minecraft:air").is_none());
        // Pale-garden *markers* are still dropped (only the pale wood blocks remap).
        assert!(map_block("minecraft:creaking_heart").is_none());
        assert!(map_block("minecraft:open_eyeblossom").is_none());
        assert!(map_block("minecraft:pale_hanging_moss").is_none());
        assert!(map_block("minecraft:short_grass").is_none());
        assert!(map_block("minecraft:vine").is_none());
        assert!(map_block("minecraft:cocoa[age=2]").is_none());
    }

    #[test]
    fn trunk_slots_stay_at_least_one_block_apart() {
        // Collect the distinct slots over a patch and assert no two are within 1 block
        // (Chebyshev) of each other -> trunks always have a >= 1 block gap.
        let mut slots = std::collections::HashSet::new();
        for x in -30..30 {
            for z in -30..30 {
                slots.insert(trunk_slot(x, z, 1.0));
            }
        }
        let v: Vec<(i32, i32)> = slots.into_iter().collect();
        for i in 0..v.len() {
            for j in (i + 1)..v.len() {
                let (dx, dz) = ((v[i].0 - v[j].0).abs(), (v[i].1 - v[j].1).abs());
                assert!(dx.max(dz) >= 2, "slots {:?} and {:?} touch", v[i], v[j]);
            }
        }
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
