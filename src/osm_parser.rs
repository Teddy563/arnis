use crate::clipping::clip_way_to_bbox;
use crate::coordinate_system::cartesian::{XZBBox, XZPoint};
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::coordinate_system::transformation::CoordTransformer;
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

// Tags Arnis never reads. Filtered at parse time to save memory.
const IGNORED_TAGS: &[&str] = &[
    "created_by",
    "note",
    "fixme",
    "FIXME",
    "todo",
    "TODO",
    "wikipedia",
    "wikimedia_commons",
    "import_uuid",
    "import",
    "old_name",
    "loc_name",
    "official_name",
    "alt_name",
    "operator",
    "phone",
    "fax",
    "email",
    "url",
    "website",
    "opening_hours",
    "description",
    "attribution",
    "start_date",
    "check_date",
    "survey:date",
    "ref:bag",
    "ref:bygningsnr",
];

// Tag-key prefixes Arnis never reads (localized names, addresses, regional import refs).
const IGNORED_PREFIXES: &[&str] = &[
    "addr:",
    "source",
    "name:",
    "alt_name:",
    "contact:",
    "is_in:",
    "operator:",
    "tiger:",
    "NHD:",
    "lacounty:",
    "nysgissam:",
    "ref:ruian:",
    "building:ruian:",
    "osak:",
    "gnis:",
    "yh:",
    "check_date:",
];

fn filter_tags(mut tags: HashMap<String, String>) -> HashMap<String, String> {
    tags.retain(|k, _| {
        !IGNORED_TAGS.contains(&k.as_str()) && !IGNORED_PREFIXES.iter().any(|p| k.starts_with(p))
    });
    tags
}

// Raw data from OSM

// Serialize: for the osm_sidecar bincode bake (phase-5 A2). Any shape change
// to OsmMember/OsmElement MUST bump osm_sidecar::OSM_SIDECAR_CODEC_VERSION.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OsmMember {
    pub(crate) r#type: String,
    pub(crate) r#ref: u64,
    pub(crate) r#role: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct OsmElement {
    pub r#type: String,
    pub id: u64,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub nodes: Option<Vec<u64>>,
    pub tags: Option<HashMap<String, String>>,
    #[serde(default)]
    pub members: Vec<OsmMember>,
}

#[derive(Debug, Deserialize)]
pub struct OsmData {
    elements: Vec<OsmElement>,
    #[serde(default)]
    pub remark: Option<String>,
}

impl OsmData {
    /// Returns true if there are no elements in the OSM data
    pub fn is_empty(&self) -> bool {
        self.elements.is_empty()
    }

    /// Object-free dataset, used by terrain-only runs that never query Overpass
    /// and never read the Meld tile dir.
    pub fn empty() -> Self {
        OsmData {
            elements: Vec::new(),
            remark: None,
        }
    }

    /// Load OSM directly from Meld's stable slippy-tile grid cache instead of a
    /// single merged `--file`. Computes the z`zoom` tiles overlapping `bbox`,
    /// reads each `osm_g1_z{zoom}_{x}_{y}.json`, and concats + dedups elements by
    /// (type, id) — producing the exact same OsmData a pre-merged clump file would,
    /// with NO per-cell merge step on Meld's side. Missing tiles are skipped
    /// (so an un-baked edge tile just contributes nothing, same as a sparse clump).
    pub fn from_tile_dir(
        dir: &str,
        bbox: LLBBox,
        zoom: u8,
    ) -> Result<OsmData, Box<dyn std::error::Error>> {
        println!("{} Loading OSM grid tiles from dir...", "[1/7]".bold());
        emit_gui_progress_update(1.0, "Loading OSM grid tiles...");

        let (xa, ya) = slippy_tile(bbox.max().lat(), bbox.min().lng(), zoom); // NW corner
        let (xb, yb) = slippy_tile(bbox.min().lat(), bbox.max().lng(), zoom); // SE corner
        let (xlo, xhi) = (xa.min(xb), xa.max(xb));
        let (ylo, yhi) = (ya.min(yb), ya.max(yb));

        let mut seen: HashSet<((u8, u8), u64)> = HashSet::new();

        /// One-byte-ish dedup key for an element type. node/way/relation - the only types
        /// the OSM API emits - get distinct tags; anything else keys on (first byte, len)
        /// so two different unknown strings cannot silently alias in practice, and the
        /// dedup no longer clones a String per element (67.6M clones per 81-cell run).
        fn kind_key(t: &str) -> (u8, u8) {
            match t {
                "node" => (1, 0),
                "way" => (2, 0),
                "relation" => (3, 0),
                other => (
                    other.as_bytes().first().copied().unwrap_or(0).max(4),
                    other.len().min(255) as u8,
                ),
            }
        }
        let mut elements: Vec<OsmElement> = Vec::new();
        let (mut tiles_read, mut tiles_missing) = (0u32, 0u32);

        for x in xlo..=xhi {
            for y in ylo..=yhi {
                let path = format!("{dir}/osm_g1_z{zoom}_{x}_{y}.json");
                let file = match File::open(&path) {
                    Ok(f) => f,
                    Err(_) => {
                        tiles_missing += 1;
                        continue;
                    }
                };
                // A1 (phase 5): from_slice, not from_reader. serde_json's IoRead path
                // tokenizes through a byte-at-a-time Read adapter; reading the file into
                // one buffer first lets the borrowed SliceRead fast path run. The
                // Deserializer form (not the free function) keeps acceptance identical:
                // the free `from_slice` calls end() and would reject tiles today's code
                // accepts with trailing bytes.
                let mut buf = Vec::new();
                {
                    use std::io::Read;
                    let mut rdr = BufReader::new(file);
                    if let Err(e) = rdr.read_to_end(&mut buf) {
                        eprintln!("  [osm-tile-dir] skip unreadable tile {path}: {e}");
                        continue;
                    }
                }
                // A3 (phase 5): per-tile bincode sidecar fast path. The source .json
                // bytes were just read above and are re-hashed inside read_verified on
                // EVERY read (content hash, never mtime); any mismatch, short file, or
                // decode error falls through silently to the JSON path below — which is
                // byte-for-byte the code A1 shipped — and re-bakes the sidecar.
                // ARNIS_OSM_SIDECARS=0 opts out of the whole sidecar system (an .osmbin
                // costs roughly two thirds of its tile's size on disk).
                let use_sidecars = crate::osm_sidecar::sidecars_enabled();
                let bin_path = crate::osm_sidecar::sidecar_path(&path);
                let cached = if use_sidecars {
                    crate::osm_sidecar::read_verified(&bin_path, &buf)
                } else {
                    None
                };
                let tile_elements = match cached {
                    Some(els) => els,
                    None => {
                        let mut de = serde_json::Deserializer::from_slice(&buf);
                        let data = match OsmData::deserialize(&mut de) {
                            Ok(d) => d,
                            Err(e) => {
                                eprintln!("  [osm-tile-dir] skip unreadable tile {path}: {e}");
                                continue;
                            }
                        };
                        if use_sidecars {
                            // Bake-on-first-miss piggybacks on the decode this cell paid
                            // anyway (verify-at-bake + atomic rename inside).
                            crate::osm_sidecar::bake(&bin_path, &buf, &data.elements);
                        }
                        data.elements
                    }
                };
                tiles_read += 1;
                for el in tile_elements {
                    // (kind_key, id): one byte instead of a cloned String per element.
                    // Same dedup semantics - node/way/relation map to distinct keys, and
                    // an unknown type string maps to its first byte + length so two
                    // different unknown strings cannot alias (checked below).
                    if seen.insert((kind_key(&el.r#type), el.id)) {
                        elements.push(el);
                    }
                }
            }
        }

        println!(
            "  [osm-tile-dir] {tiles_read} tile(s) read, {tiles_missing} missing → {} unique element(s)",
            elements.len()
        );
        Ok(OsmData {
            elements,
            remark: None,
        })
    }
}

/// Standard web-mercator slippy-tile index for (lat, lon) at `zoom`, matching
/// Meld's `survey._lat_lng_to_tile` (and so the filenames it writes). Clamped to
/// the valid `[0, 2^zoom)` tile range.
fn slippy_tile(lat: f64, lon: f64, zoom: u8) -> (i64, i64) {
    let n = 2f64.powi(zoom as i32);
    let x = ((lon + 180.0) / 360.0 * n).floor() as i64;
    let lat_rad = lat.to_radians();
    let y = ((1.0 - lat_rad.tan().asinh() / std::f64::consts::PI) / 2.0 * n).floor() as i64;
    let max_idx = n as i64 - 1;
    (x.clamp(0, max_idx), y.clamp(0, max_idx))
}

struct SplitOsmData {
    pub nodes: Vec<OsmElement>,
    pub ways: Vec<OsmElement>,
    pub relations: Vec<OsmElement>,
    #[allow(dead_code)]
    pub others: Vec<OsmElement>,
}

impl SplitOsmData {
    fn total_count(&self) -> usize {
        self.nodes.len() + self.ways.len() + self.relations.len() + self.others.len()
    }
    fn from_raw_osm_data(osm_data: OsmData) -> Self {
        let mut nodes = Vec::new();
        let mut ways = Vec::new();
        let mut relations = Vec::new();
        let mut others = Vec::new();
        for element in osm_data.elements {
            match element.r#type.as_str() {
                "node" => nodes.push(element),
                "way" => ways.push(element),
                "relation" => relations.push(element),
                _ => others.push(element),
            }
        }
        SplitOsmData {
            nodes,
            ways,
            relations,
            others,
        }
    }
}

// End raw data

// Normalized data that we can use

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedNode {
    pub id: u64,
    pub tags: HashMap<String, String>,

    // Minecraft coordinates
    pub x: i32,
    pub z: i32,
}

impl ProcessedNode {
    pub fn xz(&self) -> XZPoint {
        XZPoint {
            x: self.x,
            z: self.z,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedWay {
    pub id: u64,
    pub nodes: Vec<ProcessedNode>,
    pub tags: HashMap<String, String>,
    /// Pre-clip bounding box (min_x, max_x, min_z, max_z) computed from the
    /// full unclipped way. When the way straddles a tile bbox, `nodes`
    /// contains only the inside segment — but tile-invariant decisions (e.g.
    /// building skyscraper proportions) need the original extent so adjacent
    /// tiles render the same building identically. `None` when the source-
    /// builder didn't populate it (back-compat for non-OSM builders).
    pub unclipped_bounds: Option<(i32, i32, i32, i32)>,
    /// Pre-clip polygon area (shoelace, in cells²). Building diagonality and
    /// any other shape-aware decisions must use this rather than re-deriving
    /// from `nodes`, which contain only the in-bbox segment after clipping.
    pub unclipped_polygon_area: Option<f64>,
}

#[derive(Debug, PartialEq, Clone)]
pub enum ProcessedMemberRole {
    Outer,
    Inner,
    Part,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedMember {
    pub role: ProcessedMemberRole,
    pub way: Arc<ProcessedWay>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessedRelation {
    pub id: u64,
    pub tags: HashMap<String, String>,
    pub members: Vec<ProcessedMember>,
}

/// Returns (min_x, max_x, min_z, max_z) for a slice of unclipped nodes,
/// or `None` if empty. Used to populate `ProcessedWay::unclipped_bounds`
/// before bbox clipping so tile-invariant decisions can use full extent.
pub fn compute_node_bounds(nodes: &[ProcessedNode]) -> Option<(i32, i32, i32, i32)> {
    let first = nodes.first()?;
    let mut min_x = first.x;
    let mut max_x = first.x;
    let mut min_z = first.z;
    let mut max_z = first.z;
    for n in nodes.iter().skip(1) {
        if n.x < min_x {
            min_x = n.x;
        }
        if n.x > max_x {
            max_x = n.x;
        }
        if n.z < min_z {
            min_z = n.z;
        }
        if n.z > max_z {
            max_z = n.z;
        }
    }
    Some((min_x, max_x, min_z, max_z))
}

/// Whether a feature with these unclipped block bounds `(min_x, max_x, min_z,
/// max_z)` should render UNCLIPPED in tile-invariant (master-origin) mode.
///
/// Two conditions, shared by the standalone-way path (here) and the
/// relation-ring path (`buildings.rs`) so the guard can never drift between
/// them — if it did, trees and buildings would un-clip under different rules:
///  1. the feature overlaps this cell's bbox at all, and
///  2. its bbox area is under half the flood-fill cap, so `flood_fill_area`
///     cannot hit the cap and return EMPTY (feature vanishes) and the
///     wall-clock timeout cannot diverge per cell. Oversized features keep the
///     clip instead.
pub fn tile_unclip_within_cap(bounds: (i32, i32, i32, i32), xzbbox: &XZBBox) -> bool {
    let (bx0, bx1, bz0, bz1) = bounds;
    let overlaps = bx1 >= xzbbox.min_x()
        && bx0 <= xzbbox.max_x()
        && bz1 >= xzbbox.min_z()
        && bz0 <= xzbbox.max_z();
    let area = (bx1 as i64 - bx0 as i64 + 1) * (bz1 as i64 - bz0 as i64 + 1);
    overlaps && area < crate::floodfill::MAX_FLOOD_FILL_AREA / 2
}

/// Areas that scatter per-tile content (trees/plants/decoration) over their
/// flood-filled interior, which must render UNCLIPPED in tile mode so adjacent
/// cells match at the seam (a clipped polygon flood-fills a different interior in
/// each cell, so the per-tile scatter lands differently on each side).
///
/// Excluded on purpose: water (own ring-clip path in `water_areas.rs`), and
/// pure-ground landuse (residential/industrial/farmland — uniform fill, no
/// per-tile scatter, often huge, so kept clipped to bound flood-fill cost).
pub fn is_scatter_area_tags(tags: &HashMap<String, String>) -> bool {
    // Water is handled by the dedicated water-area ring clipper; never un-clip here.
    if is_water_element(tags) {
        return false;
    }
    // Buildings: footprint must stay whole across the seam.
    if tags.contains_key("building") || tags.contains_key("building:part") {
        return true;
    }
    // Any leisure area (park/garden/nature_reserve/…) scatters trees & plants.
    if tags.contains_key("leisure") {
        return true;
    }
    // Natural areas: only the ones whose processor already uses a POSITION-pure
    // per-tile RNG (wood / tree_row). The other natural subtypes (scrub, heath,
    // grassland, wetland, …) still consume a shared id-seeded stream in flood-fill
    // order, so un-clipping them would make that stream traverse the full polygon
    // in both cells and AMPLIFY any terrain-gate desync across the whole area at
    // every seam. Until those arms are converted to coord_rng (like leisure/landuse
    // here), keep them CLIPPED so the desync stays localized rather than cascading.
    if let Some(n) = tags.get("natural") {
        if matches!(n.as_str(), "wood" | "tree_row") {
            return true;
        }
    }
    // Vegetation / recreation landuse that scatters features. Pure-ground landuse
    // is intentionally omitted (see doc comment).
    if let Some(l) = tags.get("landuse") {
        if matches!(
            l.as_str(),
            "forest"
                | "meadow"
                | "grass"
                | "grassland"
                | "greenfield"
                | "village_green"
                | "recreation_ground"
                | "cemetery"
                | "orchard"
                | "vineyard"
                | "allotments"
                | "plant_nursery"
                | "flowerbed"
        ) {
            return true;
        }
    }
    // Cemetery as an amenity (older tagging) scatters graves/trees too.
    if tags
        .get("amenity")
        .map(|a| a == "grave_yard")
        .unwrap_or(false)
    {
        return true;
    }
    false
}

/// Shoelace polygon area (in cells²) over the unclipped node ring.
/// Returns `None` when fewer than 3 nodes are present. Used together with
/// `compute_node_bounds` so building roof-type / diagonality decisions are
/// computed from the full pre-clip shape and remain identical across tiles.
pub fn compute_polygon_area(nodes: &[ProcessedNode]) -> Option<f64> {
    if nodes.len() < 3 {
        return None;
    }
    let mut area = 0i64;
    for i in 0..nodes.len() {
        let j = (i + 1) % nodes.len();
        area += (nodes[i].x as i64) * (nodes[j].z as i64);
        area -= (nodes[j].x as i64) * (nodes[i].z as i64);
    }
    Some((area.abs() as f64) / 2.0)
}

#[derive(Debug, Clone)]
pub enum ProcessedElement {
    Node(ProcessedNode),
    Way(ProcessedWay),
    Relation(ProcessedRelation),
}

impl ProcessedElement {
    pub fn tags(&self) -> &HashMap<String, String> {
        match self {
            ProcessedElement::Node(n) => &n.tags,
            ProcessedElement::Way(w) => &w.tags,
            ProcessedElement::Relation(r) => &r.tags,
        }
    }

    pub fn id(&self) -> u64 {
        match self {
            ProcessedElement::Node(n) => n.id,
            ProcessedElement::Way(w) => w.id,
            ProcessedElement::Relation(r) => r.id,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            ProcessedElement::Node(_) => "node",
            ProcessedElement::Way(_) => "way",
            ProcessedElement::Relation(_) => "relation",
        }
    }

    pub fn nodes<'a>(&'a self) -> Box<dyn Iterator<Item = &'a ProcessedNode> + 'a> {
        match self {
            ProcessedElement::Node(node) => Box::new([node].into_iter()),
            ProcessedElement::Way(way) => Box::new(way.nodes.iter()),
            ProcessedElement::Relation(_) => Box::new([].into_iter()),
        }
    }
}

pub type OutlineSuppression = HashSet<(&'static str, u64)>;

// building:part way id -> shared style seed (containing outline id, or salted relation id)
pub type PartGroups = HashMap<u64, u64>;

// keeps relation-derived seeds out of the way-id namespace
const RELATION_SEED_BIT: u64 = 1 << 63;

// 2-bit facade-style hint packed into a part's shared seed (bits 61-62)
const STYLE_HINT_SHIFT: u64 = 61;
const STYLE_HINT_MASK: u64 = 0b11 << STYLE_HINT_SHIFT;

/// Facade-style hint derived from a building's OSM tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StyleHint {
    None = 0,
    Masonry = 1,      // historic / stone / brick
    Contemporary = 2, // concrete frame, modern
    Glass = 3,        // self-declared glass curtain
}

/// Reads the packed style hint back out of a shared seed.
pub fn style_hint_from_seed(seed: u64) -> StyleHint {
    match (seed & STYLE_HINT_MASK) >> STYLE_HINT_SHIFT {
        1 => StyleHint::Masonry,
        2 => StyleHint::Contemporary,
        3 => StyleHint::Glass,
        _ => StyleHint::None,
    }
}

/// The seed with its style-hint bits cleared (used for the random variant roll).
pub fn seed_without_hint(seed: u64) -> u64 {
    seed & !STYLE_HINT_MASK
}

fn seed_with_hint(seed: u64, hint: StyleHint) -> u64 {
    (seed & !STYLE_HINT_MASK) | ((hint as u64) << STYLE_HINT_SHIFT)
}

// lowercase, strip whitespace/_/- so art_deco, neo-gothic, "concrete masonry unit" all collapse
fn norm_tag(v: &str) -> String {
    v.chars()
        .filter(|c| !c.is_whitespace() && *c != '_' && *c != '-')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

// First 4-digit year in a date-ish value, e.g. "1911-1913" -> 1911, "1955-12-31" -> 1955.
fn first_year(v: &str) -> Option<i32> {
    let bytes = v.as_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i..i + 4].iter().all(|b| b.is_ascii_digit()) {
            return v[i..i + 4].parse().ok();
        }
        i += 1;
    }
    None
}

// Ornate pre-modern styles (heritage-grade detailing). Brutalist-family styles
// are masonry-era for the tall-building StyleHint but post-war for ArchEra.
const ORNATE_STYLES: &[&str] = &[
    "artdeco",
    "artnouveau",
    "gothic",
    "neogothic",
    "gothicrevival",
    "neoclassicism",
    "neoclassical",
    "classicism",
    "classicalrevival",
    "greekrevival",
    "baroque",
    "neobaroque",
    "rococo",
    "barocco",
    "historicism",
    "eclectic",
    "renaissance",
    "neorenaissance",
    "romanesque",
    "neoromanesque",
    "romanesquerevival",
    "victorian",
    "georgian",
    "federal",
    "italianate",
    "beauxarts",
    "wilhelminianstyle",
    "queenanne",
];
const BRUTALIST_STYLES: &[&str] = &["brutalist", "constructivism", "stalinistneoclassicism"];
const MODERN_STYLES: &[&str] = &[
    "modern",
    "contemporary",
    "modernism",
    "functionalism",
    "newobjectivity",
    "postmodern",
    "bauhaus",
];
const MASONRY_MATERIALS: &[&str] = &[
    "brick",
    "bricks",
    "redbrick",
    "silicatebrick",
    "stone",
    "naturalstone",
    "sandstone",
    "limestone",
    "masonry",
    "granite",
    "marble",
    "terracotta",
    "adobe",
    "stucco",
    "pebbledash",
];
const MASONRY_CLADDING: &[&str] = &[
    "brick",
    "brickmonolith",
    "plaster",
    "rendered",
    "rendering",
    "stone",
    "tiling",
];
const CONCRETE_MATERIALS: &[&str] = &[
    "concrete",
    "reinforcedconcrete",
    "concretereinforced",
    "concretemasonryunit",
];
const PANEL_MATERIALS: &[&str] = &["panel", "panels", "prefab", "prefabricated", "panelhouse"];

/// Picks a facade style for a building from its OSM tags, or None to leave it to the random roll.
pub fn building_style_hint(tags: &HashMap<String, String>) -> StyleHint {
    let material = tags
        .get("building:material")
        .or_else(|| tags.get("building:facade:material"))
        .or_else(|| tags.get("facade:material"))
        .map(|m| norm_tag(m));

    // Glass override wins over everything, so heritage-listed glass towers stay glass.
    if material.as_deref() == Some("glass") || material.as_deref() == Some("mirror") {
        return StyleHint::Glass;
    }
    if tags.get("roof:material").map(|r| norm_tag(r)).as_deref() == Some("glass") {
        return StyleHint::Glass;
    }

    // Masonry / historic. `no` on these keys is an explicit negation, not a signal.
    let present_and_not_no =
        |key: &str| tags.get(key).is_some_and(|v| !v.eq_ignore_ascii_case("no"));
    if present_and_not_no("historic")
        || present_and_not_no("heritage")
        || tags.contains_key("ref:nrhp")
        || present_and_not_no("listed_status")
    {
        return StyleHint::Masonry;
    }
    if material
        .as_deref()
        .is_some_and(|m| MASONRY_MATERIALS.contains(&m))
    {
        return StyleHint::Masonry;
    }
    if let Some(c) = tags.get("building:cladding") {
        if MASONRY_CLADDING.contains(&norm_tag(c).as_str()) {
            return StyleHint::Masonry;
        }
    }
    let arch = tags
        .get("building:architecture")
        .or_else(|| tags.get("architecture"))
        .map(|a| norm_tag(a));
    if let Some(a) = arch.as_deref() {
        if ORNATE_STYLES.contains(&a) || BRUTALIST_STYLES.contains(&a) {
            return StyleHint::Masonry;
        }
        if MODERN_STYLES.contains(&a) {
            return StyleHint::Contemporary;
        }
    }
    // Pre-curtain-wall era: load-bearing masonry. start_date is the best-populated source.
    for key in ["start_date", "construction_date", "year_of_construction"] {
        if let Some(y) = tags.get(key).and_then(|v| first_year(v)) {
            if y < 1945 {
                return StyleHint::Masonry;
            }
            break; // known modern year; fall through to the concrete check
        }
    }

    // Concrete frame reads as a solid facade with windows: the contemporary middle style.
    if material
        .as_deref()
        .is_some_and(|m| CONCRETE_MATERIALS.contains(&m))
    {
        return StyleHint::Contemporary;
    }
    StyleHint::None
}

#[cfg(test)]
mod style_hint_tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn material_historic_and_year_drive_the_hint() {
        assert_eq!(
            building_style_hint(&tags(&[("building:material", "glass")])),
            StyleHint::Glass
        );
        assert_eq!(
            building_style_hint(&tags(&[("building:material", "brick")])),
            StyleHint::Masonry
        );
        assert_eq!(
            building_style_hint(&tags(&[("historic", "yes")])),
            StyleHint::Masonry
        );
        assert_eq!(
            building_style_hint(&tags(&[("building:material", "concrete")])),
            StyleHint::Contemporary
        );
        assert_eq!(
            building_style_hint(&tags(&[("start_date", "1890-1892")])),
            StyleHint::Masonry
        );
        assert_eq!(
            building_style_hint(&tags(&[("building", "yes")])),
            StyleHint::None
        );
        // Glass wins over a historic tag; `no` on historic is an explicit negation.
        assert_eq!(
            building_style_hint(&tags(&[
                ("historic", "yes"),
                ("building:material", "glass")
            ])),
            StyleHint::Glass
        );
        assert_eq!(
            building_style_hint(&tags(&[("historic", "no")])),
            StyleHint::None
        );
    }

    #[test]
    fn seed_hint_round_trips_without_clobbering_the_id() {
        let seed = 1486752423u64;
        for hint in [
            StyleHint::None,
            StyleHint::Masonry,
            StyleHint::Contemporary,
            StyleHint::Glass,
        ] {
            let packed = seed_with_hint(seed, hint);
            assert_eq!(style_hint_from_seed(packed), hint);
            assert_eq!(seed_without_hint(packed), seed);
        }
    }
}

/// Packs each S3DB part's facade hint (derived from its parent outline way / relation tags)
/// into the shared part seed, so untagged parts of a tagged building inherit the same facade
/// family. Standalone buildings carry no part_groups entry and read their own tags at render
/// time instead. Keyed only on world-absolute OSM ids + tags, so it stays tile-invariant.
fn pack_part_style_hints(
    part_groups: &mut PartGroups,
    ways: &[OsmElement],
    relations: &[OsmElement],
) {
    if part_groups.is_empty() {
        return;
    }
    let way_tags: HashMap<u64, &HashMap<String, String>> = ways
        .iter()
        .filter_map(|w| w.tags.as_ref().map(|t| (w.id, t)))
        .collect();
    let rel_tags: HashMap<u64, &HashMap<String, String>> = relations
        .iter()
        .filter_map(|r| r.tags.as_ref().map(|t| (r.id, t)))
        .collect();
    for seed in part_groups.values_mut() {
        let parent_tags = if *seed & RELATION_SEED_BIT != 0 {
            rel_tags.get(&(*seed & !RELATION_SEED_BIT))
        } else {
            way_tags.get(seed)
        };
        if let Some(tags) = parent_tags {
            let hint = building_style_hint(tags);
            if hint != StyleHint::None {
                *seed = seed_with_hint(*seed, hint);
            }
        }
    }
}

/// Architectural era of a building, consumed by low-rise styling (wall
/// palettes, window frames, depth styles, weathering). Unlike `StyleHint`
/// (2 bits packed into the part seed, consulted for tall buildings only),
/// the era is recomputed from tags per building.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchEra {
    Unknown,
    /// Explicit heritage or ornate-architecture signals.
    HistoricOrnate,
    /// Pre-1945 masonry-era fabric without ornate signals — a 1900 farmhouse
    /// is traditional, not ornamented.
    TraditionalPreWar,
    /// 1945–1979: panel, prefab, brutalist.
    PostWarPanel,
    /// 1980+: concrete/glass contemporary.
    Contemporary,
}

/// Classifies a building's era from its tags; `Unknown` when nothing signals.
pub fn building_arch_era(tags: &HashMap<String, String>) -> ArchEra {
    // Explicit heritage signals → ornate. `no` is a negation, not a signal.
    let present_and_not_no =
        |key: &str| tags.get(key).is_some_and(|v| !v.eq_ignore_ascii_case("no"));
    if present_and_not_no("historic")
        || present_and_not_no("heritage")
        || tags.contains_key("ref:nrhp")
        || present_and_not_no("listed_status")
    {
        return ArchEra::HistoricOrnate;
    }

    let arch = tags
        .get("building:architecture")
        .or_else(|| tags.get("architecture"))
        .map(|a| norm_tag(a));
    if let Some(a) = arch.as_deref() {
        if ORNATE_STYLES.contains(&a) {
            return ArchEra::HistoricOrnate;
        }
        if BRUTALIST_STYLES.contains(&a) {
            return ArchEra::PostWarPanel;
        }
        if MODERN_STYLES.contains(&a) {
            return ArchEra::Contemporary;
        }
    }

    for key in ["start_date", "construction_date", "year_of_construction"] {
        if let Some(y) = tags.get(key).and_then(|v| first_year(v)) {
            return if y < 1945 {
                ArchEra::TraditionalPreWar
            } else if y < 1980 {
                ArchEra::PostWarPanel
            } else {
                ArchEra::Contemporary
            };
        }
    }

    let material = tags
        .get("building:material")
        .or_else(|| tags.get("building:facade:material"))
        .or_else(|| tags.get("facade:material"))
        .map(|m| norm_tag(m));
    if let Some(m) = material.as_deref() {
        if PANEL_MATERIALS.contains(&m) {
            return ArchEra::PostWarPanel;
        }
        if MASONRY_MATERIALS.contains(&m) {
            return ArchEra::TraditionalPreWar;
        }
        if CONCRETE_MATERIALS.contains(&m) || m == "glass" || m == "mirror" {
            return ArchEra::Contemporary;
        }
    }
    if let Some(c) = tags.get("building:cladding") {
        if MASONRY_CLADDING.contains(&norm_tag(c).as_str()) {
            return ArchEra::TraditionalPreWar;
        }
    }
    ArchEra::Unknown
}

/// Fallback era for untagged `building:part`s from the packed group-seed hint.
pub fn arch_era_from_hint(hint: StyleHint) -> ArchEra {
    match hint {
        StyleHint::Masonry => ArchEra::TraditionalPreWar,
        StyleHint::Contemporary | StyleHint::Glass => ArchEra::Contemporary,
        StyleHint::None => ArchEra::Unknown,
    }
}

pub fn parse_osm_data(
    osm_data: OsmData,
    bbox: LLBBox,
    scale: f64,
    debug: bool,
    master_origin_lat: Option<f64>,
    master_origin_lng: Option<f64>,
    tile_invariant_rendering: Option<u64>,
) -> (
    Vec<ProcessedElement>,
    XZBBox,
    OutlineSuppression,
    PartGroups,
) {
    println!("{} Parsing data...", "[2/7]".bold());
    println!("Bounding box: {bbox:?}");
    emit_gui_progress_update(5.0, "Parsing data...");

    // Deserialize the JSON data into the OSMData structure
    let data = SplitOsmData::from_raw_osm_data(osm_data);

    let (coord_transformer, xzbbox) =
        CoordTransformer::llbbox_to_xzbbox(&bbox, scale, master_origin_lat, master_origin_lng)
            .unwrap_or_else(|e| {
                eprintln!("Error in defining coordinate transformation:\n{e}");
                panic!();
            });

    if debug {
        println!("Total elements: {}", data.total_count());
        println!("Scale factor X: {}", coord_transformer.scale_factor_x());
        println!("Scale factor Z: {}", coord_transformer.scale_factor_z());
    }

    let mut part_groups = PartGroups::new();
    let mut outline_suppression =
        compute_outline_suppression(&data.relations, &data.ways, &data.nodes, &mut part_groups);
    // also catch S3DB outlines mapped without a relation
    outline_suppression.extend(compute_spatial_part_suppression(
        &data.ways,
        &data.nodes,
        &mut part_groups,
    ));
    // A relation whose every outer member is a closed building:part way draws nothing of its
    // own — those ways already render standalone — so drop the duplicated merged ring.
    outline_suppression.extend(compute_part_way_outline_suppression(
        &data.relations,
        &data.ways,
    ));
    // Propagate each building's tag-derived facade style to its untagged S3DB parts.
    pack_part_style_hints(&mut part_groups, &data.ways, &data.relations);

    let mut nodes_map: HashMap<u64, ProcessedNode> = HashMap::new();
    let mut ways_map: HashMap<u64, Arc<ProcessedWay>> = HashMap::new();

    let mut processed_elements: Vec<ProcessedElement> = Vec::new();

    // First pass: store all nodes with Minecraft coordinates and process nodes with tags
    for element in data.nodes {
        if let (Some(lat), Some(lon)) = (element.lat, element.lon) {
            let llpoint = LLPoint::new(lat, lon).unwrap_or_else(|e| {
                eprintln!("Encountered invalid node element:\n{e}");
                panic!();
            });

            let xzpoint = coord_transformer.transform_point(llpoint);

            let processed: ProcessedNode = ProcessedNode {
                id: element.id,
                tags: filter_tags(element.tags.unwrap_or_default()),
                x: xzpoint.x,
                z: xzpoint.z,
            };

            nodes_map.insert(element.id, processed.clone());

            // Only add tagged nodes to processed_elements if they're within or near the bbox
            // This significantly improves performance by filtering out distant nodes
            if !processed.tags.is_empty() && xzbbox.contains(&xzpoint) {
                processed_elements.push(ProcessedElement::Node(processed));
            }
        }
    }

    // A way whose node list is not fully resolvable is silently shortened below.
    // For a road that only bends it; for an area it breaks the ring, and an
    // unclosed ring flood-fills to nothing, so the whole polygon disappears
    // without a word. Counted here so an incomplete source is reported once
    // rather than discovered by looking at the finished world.
    let mut unresolved_node_refs: u64 = 0;
    let mut total_node_refs: u64 = 0;
    let mut ways_missing_nodes: u64 = 0;

    // Second pass: process ways and clip them to bbox
    for element in data.ways {
        let mut nodes: Vec<ProcessedNode> = vec![];
        if let Some(node_ids) = &element.nodes {
            let before = unresolved_node_refs;
            for &node_id in node_ids {
                total_node_refs += 1;
                match nodes_map.get(&node_id) {
                    Some(node) => nodes.push(node.clone()),
                    None => unresolved_node_refs += 1,
                }
            }
            if unresolved_node_refs > before {
                ways_missing_nodes += 1;
            }
        }

        // Clip the way to bbox to reduce node count dramatically
        let tags = filter_tags(element.tags.unwrap_or_default());

        // Capture pre-clip bounds + polygon area so tile-invariant decisions
        // (building category / skyscraper proportions / roof diagonality) get
        // the same answer in every tile that touches the way, regardless of
        // where its bbox cuts. Gated on `--tile-invariant-rendering`. When
        // off, both fields stay None and building decisions fall through to
        // the upstream clipped-nodes path (byte-identical to v2.7.0).
        let (unclipped_bounds, unclipped_polygon_area) = if tile_invariant_rendering.is_some() {
            (compute_node_bounds(&nodes), compute_polygon_area(&nodes))
        } else {
            (None, None)
        };

        // Store unclipped way for relation assembly (clipping happens after ring merging)
        let way = Arc::new(ProcessedWay {
            id: element.id,
            tags,
            nodes,
            unclipped_bounds,
            unclipped_polygon_area,
        });
        ways_map.insert(element.id, Arc::clone(&way));

        // Standalone way: in tile mode, render scatter areas + building footprints
        // UNCLIPPED so adjacent cells produce identical geometry at the seam (clip
        // seals each half with a wall → half a building / cut trees). set_block
        // still clamps writes to the cell bbox, so the clip was only redundant
        // safety. Cap-guarded (oversized → keep clip so it can't vanish); non-
        // scatter ways and single-world keep the upstream clip byte-for-byte.
        let tile_unclip = tile_invariant_rendering.is_some() && is_scatter_area_tags(&way.tags);
        let want_unclip =
            tile_unclip && unclipped_bounds.is_some_and(|b| tile_unclip_within_cap(b, &xzbbox));
        let nodes_for_processing = if want_unclip {
            way.nodes.clone()
        } else {
            // Clip way nodes for standalone way processing (not relations)
            let clipped_nodes = clip_way_to_bbox(&way.nodes, &xzbbox);
            // Skip ways completely outside the bbox (empty after clipping)
            if clipped_nodes.is_empty() {
                continue;
            }
            clipped_nodes
        };

        let processed: ProcessedWay = ProcessedWay {
            id: element.id,
            tags: way.tags.clone(),
            nodes: nodes_for_processing,
            unclipped_bounds,
            unclipped_polygon_area,
        };

        processed_elements.push(ProcessedElement::Way(processed));
    }

    if unresolved_node_refs > 0 {
        let percent = unresolved_node_refs as f64 / total_node_refs.max(1) as f64 * 100.0;
        eprintln!(
            "{}",
            format!(
                "Warning: {unresolved_node_refs} of {total_node_refs} way node references                  ({percent:.1}%) could not be resolved, affecting {ways_missing_nodes} ways.                  Buildings and other areas that lost a corner cannot be filled and will not                  appear in the world. Expected near the edges of a clipped local file; from                  the API it means the response was incomplete."
            )
            .yellow()
            .bold()
        );
    }

    // Third pass: process relations and clip member ways
    for element in data.relations {
        let Some(tags) = &element.tags else {
            continue;
        };

        // Process multipolygons and building relations
        let relation_type = tags.get("type").map(|x: &String| x.as_str());
        if relation_type != Some("multipolygon") && relation_type != Some("building") {
            continue;
        };

        let is_building_relation = relation_type == Some("building")
            || tags.contains_key("building")
            || tags.contains_key("building:part");

        // Water relations require unclipped ways for ring merging in water_areas.rs
        // Building multipolygon relations also need unclipped ways so that
        // open outer-way segments can be merged into closed rings before clipping
        let is_water_relation = is_water_element(tags);
        let is_building_multipolygon = (tags.contains_key("building")
            || tags.contains_key("building:part"))
            && relation_type == Some("multipolygon");
        let keep_unclipped = is_water_relation || is_building_multipolygon;

        // Scatter-area multipolygons: the way-path un-clip above never runs for
        // relation members, so they stayed clipped per-cell and didn't meld. Each
        // outer member is flood-filled individually (no ring assembly), so keep it
        // unclipped (cap-guarded per member, below). Buildings excluded — their
        // rings are handled in buildings.rs. Single-world keeps the clip.
        let scatter_unclip = relation_type == Some("multipolygon")
            && tile_invariant_rendering.is_some()
            && !is_building_multipolygon
            && is_scatter_area_tags(tags);

        let members: Vec<ProcessedMember> = element
            .members
            .iter()
            .filter_map(|mem: &OsmMember| {
                if mem.r#type != "way" {
                    if mem.r#type != "relation" && mem.r#type != "node" {
                        eprintln!("WARN: Unknown relation member type \"{}\"", mem.r#type);
                    }
                    return None;
                }

                let trimmed_role = mem.role.trim();
                let role = if trimmed_role.eq_ignore_ascii_case("outer")
                    || trimmed_role.eq_ignore_ascii_case("outline")
                {
                    ProcessedMemberRole::Outer
                } else if trimmed_role.eq_ignore_ascii_case("inner") {
                    ProcessedMemberRole::Inner
                } else if trimmed_role.eq_ignore_ascii_case("part") {
                    if relation_type == Some("building") {
                        // "part" role only applies to type=building relations.
                        ProcessedMemberRole::Part
                    } else {
                        // For multipolygon relations, "part" is not a valid role, skip.
                        return None;
                    }
                } else if is_building_relation {
                    ProcessedMemberRole::Outer
                } else {
                    return None;
                };

                // Check if the way exists in ways_map
                let way = match ways_map.get(&mem.r#ref) {
                    Some(w) => Arc::clone(w),
                    None => {
                        // Way was likely filtered out because it was completely outside the bbox
                        return None;
                    }
                };

                // Keep member ways unclipped when:
                //  - keep_unclipped (water / building multipolygons, for ring merging), or
                //  - tree-area relation in tile mode AND this member's full bbox is under
                //    the flood-fill cap (so its individual flood fill melds across cells).
                let keep_member_unclipped = keep_unclipped
                    || (scatter_unclip
                        && way
                            .unclipped_bounds
                            .is_some_and(|b| tile_unclip_within_cap(b, &xzbbox)));
                let final_way = if keep_member_unclipped {
                    way
                } else {
                    let clipped_nodes = clip_way_to_bbox(&way.nodes, &xzbbox);
                    if clipped_nodes.is_empty() {
                        return None;
                    }
                    let unclipped_bounds = way.unclipped_bounds;
                    let unclipped_polygon_area = way.unclipped_polygon_area;
                    Arc::new(ProcessedWay {
                        id: way.id,
                        tags: way.tags.clone(),
                        nodes: clipped_nodes,
                        unclipped_bounds,
                        unclipped_polygon_area,
                    })
                };

                Some(ProcessedMember {
                    role,
                    way: final_way,
                })
            })
            .collect();

        if !members.is_empty() {
            processed_elements.push(ProcessedElement::Relation(ProcessedRelation {
                id: element.id,
                members,
                tags: filter_tags(tags.clone()),
            }));
        }
    }

    emit_gui_progress_update(14.0, "");

    drop(nodes_map);
    drop(ways_map);

    (processed_elements, xzbbox, outline_suppression, part_groups)
}

// Parts replace the outline only when they cover at least this much of it.
const MIN_PART_COVERAGE: f64 = 0.5;

// A part covers the outline's ground footprint only if it starts at ground level.
// Elevated parts (min_height / building:min_level > 0) model raised roof/dome volumes
// that float above the ground (e.g. S3DB churches), so they can't stand in for the outline.
fn part_covers_ground(tags: &HashMap<String, String>) -> bool {
    // Leading number only: sign at position 0, one decimal point. So "54-60" → 54, not a parse fail.
    let leading_f64 = |s: &str| {
        let t = s.trim();
        let mut end = 0;
        let mut seen_dot = false;
        for (i, c) in t.char_indices() {
            let ok =
                c.is_ascii_digit() || (c == '.' && !seen_dot) || ((c == '-' || c == '+') && i == 0);
            if !ok {
                break;
            }
            seen_dot |= c == '.';
            end = i + c.len_utf8();
        }
        t[..end].parse::<f64>().ok()
    };
    let min_h = tags.get("min_height").and_then(|s| leading_f64(s));
    let min_lvl = tags.get("building:min_level").and_then(|s| leading_f64(s));
    min_h.unwrap_or(0.0) <= 0.0 && min_lvl.unwrap_or(0.0) <= 0.0
}

// Grid cell size (degrees) for the spatial index that buckets nearby outline bboxes.
const SUPPRESSION_GRID_CELL_DEG: f64 = 0.0005;

fn compute_outline_suppression(
    relations: &[OsmElement],
    ways: &[OsmElement],
    nodes: &[OsmElement],
    part_group: &mut PartGroups,
) -> OutlineSuppression {
    let is_outline = |r: &str| r.eq_ignore_ascii_case("outline") || r.eq_ignore_ascii_case("outer");

    let mut needed_ways: HashSet<u64> = HashSet::new();
    for rel in relations {
        let Some(tags) = &rel.tags else { continue };
        if tags.get("type").map(|t| t.as_str()) != Some("building") {
            continue;
        }
        for m in &rel.members {
            let r = m.role.trim();
            if m.r#type == "way" && (r.eq_ignore_ascii_case("part") || is_outline(r)) {
                needed_ways.insert(m.r#ref);
            }
        }
    }
    if needed_ways.is_empty() {
        return HashSet::new();
    }

    // Single pass over member ways: geometry (way_nodes) plus whether the way sits on the
    // ground (way_ground), so elevated parts don't get credited toward outline coverage.
    let mut way_nodes: HashMap<u64, &Vec<u64>> = HashMap::new();
    let mut way_ground: HashMap<u64, bool> = HashMap::new();
    for w in ways.iter().filter(|w| needed_ways.contains(&w.id)) {
        if let Some(ns) = w.nodes.as_ref() {
            way_nodes.insert(w.id, ns);
        }
        if let Some(t) = w.tags.as_ref() {
            way_ground.insert(w.id, part_covers_ground(t));
        }
    }
    let mut needed_nodes: HashSet<u64> = HashSet::new();
    for ns in way_nodes.values() {
        needed_nodes.extend(ns.iter().copied());
    }
    let node_ll: HashMap<u64, (f64, f64)> = nodes
        .iter()
        .filter(|n| needed_nodes.contains(&n.id))
        .filter_map(|n| Some((n.id, (n.lat?, n.lon?))))
        .collect();

    // Shoelace area of a closed ring; lon scaled by cos(lat) so only the ratio matters.
    let way_area = |way_ref: u64| -> Option<f64> {
        let ids = way_nodes.get(&way_ref)?;
        let pts: Vec<(f64, f64)> = ids
            .iter()
            .filter_map(|id| node_ll.get(id).copied())
            .collect();
        if pts.len() < 3 {
            return None;
        }
        let lon_scale = pts[0].0.to_radians().cos();
        let mut area = 0.0;
        for i in 0..pts.len() {
            let (lat_a, lon_a) = pts[i];
            let (lat_b, lon_b) = pts[(i + 1) % pts.len()];
            area += (lon_a * lat_b - lon_b * lat_a) * lon_scale;
        }
        Some((area / 2.0).abs())
    };

    let mut suppressed: OutlineSuppression = HashSet::new();
    for rel in relations {
        let Some(tags) = &rel.tags else { continue };
        if tags.get("type").map(|t| t.as_str()) != Some("building") {
            continue;
        }

        // Sub-relation parts carry no way geometry here, so they skip the coverage gate.
        let mut has_part = false;
        let mut has_relation_part = false;
        let mut part_area = 0.0;
        for m in &rel.members {
            if !m.role.trim().eq_ignore_ascii_case("part") {
                continue;
            }
            has_part = true;
            match m.r#type.as_str() {
                "relation" => has_relation_part = true,
                "way" => {
                    // Elevated parts (raised roof/dome volumes) float above the ground and
                    // cannot stand in for the outline, so they earn no coverage credit.
                    if way_ground.get(&m.r#ref).copied().unwrap_or(true) {
                        part_area += way_area(m.r#ref).unwrap_or(0.0);
                    }
                    part_group.insert(m.r#ref, RELATION_SEED_BIT | rel.id);
                }
                _ => {}
            }
        }
        if !has_part {
            continue;
        }

        for m in &rel.members {
            let r = m.role.trim();
            if !is_outline(r) {
                continue;
            }
            let kind: &'static str = match m.r#type.as_str() {
                "way" => "way",
                "relation" => "relation",
                _ => continue,
            };

            // Keep the outline when the parts are too sparse to stand in for it.
            if kind == "way" && !has_relation_part {
                if let Some(outline_area) = way_area(m.r#ref) {
                    if outline_area > 0.0 && part_area / outline_area < MIN_PART_COVERAGE {
                        continue;
                    }
                }
            }

            suppressed.insert((kind, m.r#ref));
        }
    }
    suppressed
}

// Shoelace area of a closed lat/lon ring; lon scaled by cos(lat) so only the ratio matters.
fn ring_area(r: &[(f64, f64)]) -> f64 {
    if r.len() < 3 {
        return 0.0;
    }
    let lon_scale = r[0].0.to_radians().cos();
    let mut a = 0.0;
    for i in 0..r.len() {
        let (lat_a, lon_a) = r[i];
        let (lat_b, lon_b) = r[(i + 1) % r.len()];
        a += (lon_a * lat_b - lon_b * lat_a) * lon_scale;
    }
    (a / 2.0).abs()
}

// Ray-cast point-in-polygon test on a lat/lon ring.
fn point_in_ring(lat: f64, lon: f64, r: &[(f64, f64)]) -> bool {
    let n = r.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (yi, xi) = r[i];
        let (yj, xj) = r[j];
        if (yi > lat) != (yj > lat) && lon < (xj - xi) * (lat - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Suppresses relation-less S3DB outlines: a building polygon that spatially contains building:part polygons.
fn compute_spatial_part_suppression(
    ways: &[OsmElement],
    nodes: &[OsmElement],
    part_group: &mut PartGroups,
) -> OutlineSuppression {
    let is_part = |tags: &HashMap<String, String>| {
        tags.get("building:part")
            .is_some_and(|v| !v.eq_ignore_ascii_case("no"))
    };

    // split building ways into candidate outlines and parts
    let mut outline_ids: Vec<u64> = Vec::new();
    let mut part_ids: Vec<u64> = Vec::new();
    let mut way_nodes: HashMap<u64, &Vec<u64>> = HashMap::new();
    let mut needed_nodes: HashSet<u64> = HashSet::new();
    for w in ways {
        let (Some(tags), Some(ns)) = (&w.tags, &w.nodes) else {
            continue;
        };
        // need a closed ring (first node repeated as last)
        if ns.len() < 4 || ns.first() != ns.last() {
            continue;
        }
        if is_part(tags) {
            part_ids.push(w.id);
        } else if tags.contains_key("building") {
            outline_ids.push(w.id);
        } else {
            continue;
        }
        way_nodes.insert(w.id, ns);
        needed_nodes.extend(ns.iter().copied());
    }
    if outline_ids.is_empty() || part_ids.is_empty() {
        return HashSet::new();
    }

    let node_ll: HashMap<u64, (f64, f64)> = nodes
        .iter()
        .filter(|n| needed_nodes.contains(&n.id))
        .filter_map(|n| Some((n.id, (n.lat?, n.lon?))))
        .collect();

    let ring = |id: u64| -> Vec<(f64, f64)> {
        way_nodes
            .get(&id)
            .map(|ids| ids.iter().filter_map(|i| node_ll.get(i).copied()).collect())
            .unwrap_or_default()
    };

    // area (shoelace) and point_in_ring are shared module-level helpers (see above).

    struct OutlineGeom {
        id: u64,
        ring: Vec<(f64, f64)>,
        area: f64,
    }

    // grid of outline bboxes so each part only tests nearby outlines
    let cell = |lat: f64, lon: f64| {
        (
            (lat / SUPPRESSION_GRID_CELL_DEG).floor() as i64,
            (lon / SUPPRESSION_GRID_CELL_DEG).floor() as i64,
        )
    };

    let mut geoms: Vec<OutlineGeom> = Vec::new();
    let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for id in outline_ids {
        let r = ring(id);
        let a = ring_area(&r);
        if a <= 0.0 {
            continue;
        }
        let (mut min_la, mut min_lo, mut max_la, mut max_lo) =
            (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(la, lo) in &r {
            min_la = min_la.min(la);
            max_la = max_la.max(la);
            min_lo = min_lo.min(lo);
            max_lo = max_lo.max(lo);
        }
        let gi = geoms.len();
        let (c0a, c0o) = cell(min_la, min_lo);
        let (c1a, c1o) = cell(max_la, max_lo);
        for ca in c0a..=c1a {
            for co in c0o..=c1o {
                grid.entry((ca, co)).or_default().push(gi);
            }
        }
        geoms.push(OutlineGeom {
            id,
            ring: r,
            area: a,
        });
    }

    // add each part area to every outline containing its centroid
    let mut covered: HashMap<usize, f64> = HashMap::new();
    for pid in part_ids {
        let r = ring(pid);
        let pa = ring_area(&r);
        if pa <= 0.0 {
            continue;
        }
        let (mut sla, mut slo) = (0.0, 0.0);
        for &(la, lo) in &r {
            sla += la;
            slo += lo;
        }
        let (cla, clo) = (sla / r.len() as f64, slo / r.len() as f64);
        let Some(cands) = grid.get(&cell(cla, clo)) else {
            continue;
        };
        // smallest containing outline (tie-break min id) is the part's building
        let mut best: Option<usize> = None;
        for &gi in cands {
            if point_in_ring(cla, clo, &geoms[gi].ring) {
                *covered.entry(gi).or_insert(0.0) += pa;
                best = Some(match best {
                    Some(b) if (geoms[b].area, geoms[b].id) <= (geoms[gi].area, geoms[gi].id) => b,
                    _ => gi,
                });
            }
        }
        if let Some(gi) = best {
            part_group.insert(pid, geoms[gi].id);
        }
    }

    let mut suppressed: OutlineSuppression = HashSet::new();
    for (gi, cov) in covered {
        let g = &geoms[gi];
        if cov / g.area >= MIN_PART_COVERAGE {
            suppressed.insert(("way", g.id));
        }
    }
    suppressed
}

/// Suppresses a relation outline whose every `outer` member way is itself a closed
/// `building:part`. Such a relation contributes no geometry of its own: the parser already
/// renders each of those part ways standalone, so also drawing the merged relation ring
/// stacks a roofed box on top of the building's real S3DB tiers.
///
/// Deliberate divergence from upstream, which runs the same `all()` over the relation's
/// already-processed members inside buildings.rs. The fork drops members whose way falls
/// outside the cell bbox, and `all()` over a shrunken set is MORE likely to hold — so a cell
/// with partial geometry would suppress while its neighbour renders, a visible seam. Keying on
/// raw world-absolute ids and REQUIRING every member way to resolve in `ways` removes that: a
/// truncated member list makes the guard not fire at all, in every cell alike.
fn compute_part_way_outline_suppression(
    relations: &[OsmElement],
    ways: &[OsmElement],
) -> OutlineSuppression {
    let way_info: HashMap<u64, (&HashMap<String, String>, &Vec<u64>)> = ways
        .iter()
        .filter_map(|w| Some((w.id, (w.tags.as_ref()?, w.nodes.as_ref()?))))
        .collect();

    // Keep this in sync with the relation type filter in parse_osm_data.
    let is_renderable = |tags: &HashMap<String, String>| {
        matches!(
            tags.get("type").map(|t| t.as_str()),
            Some("multipolygon") | Some("building")
        )
    };
    let is_outer_role =
        |r: &str| r.eq_ignore_ascii_case("outer") || r.eq_ignore_ascii_case("outline");
    let is_part = |t: &HashMap<String, String>| {
        t.get("building:part")
            .is_some_and(|v| !v.eq_ignore_ascii_case("no"))
    };

    let mut suppressed: OutlineSuppression = HashSet::new();
    for rel in relations {
        let Some(tags) = &rel.tags else { continue };
        if !is_renderable(tags) {
            continue;
        }

        let outer_refs: Vec<u64> = rel
            .members
            .iter()
            .filter(|m| m.r#type == "way" && is_outer_role(m.role.trim()))
            .map(|m| m.r#ref)
            .collect();
        if outer_refs.is_empty() {
            continue;
        }

        // An unresolved member way must make the guard NOT fire: that is the anti-truncation
        // requirement, and the reason this lives here rather than over ProcessedMembers.
        let all_closed_parts = outer_refs.iter().all(|r| {
            way_info.get(r).is_some_and(|(tags, nodes)| {
                is_part(tags) && nodes.len() >= 4 && nodes.first() == nodes.last()
            })
        });
        if all_closed_parts {
            suppressed.insert(("relation", rel.id));
        }
    }
    suppressed
}

/// Returns true if tags indicate a water element handled by water_areas.rs.
fn is_water_element(tags: &HashMap<String, String>) -> bool {
    // Check for explicit water tag
    if tags.contains_key("water") {
        return true;
    }

    // Check for natural=water or natural=bay
    if let Some(natural_val) = tags.get("natural") {
        if natural_val == "water" || natural_val == "bay" {
            return true;
        }
    }

    // Check for waterway=dock (also handled as water area)
    if let Some(waterway_val) = tags.get("waterway") {
        if waterway_val == "dock" {
            return true;
        }
    }

    false
}

const PRIORITY_ORDER: [&str; 6] = [
    "entrance", "building", "highway", "waterway", "water", "barrier",
];

// Function to determine the priority of each element
pub fn get_priority(element: &ProcessedElement) -> usize {
    // Check each tag against the priority order
    for (i, &tag) in PRIORITY_ORDER.iter().enumerate() {
        if element.tags().contains_key(tag) {
            return i;
        }
    }
    // Return a default priority if none of the tags match
    PRIORITY_ORDER.len()
}

#[cfg(test)]
mod outline_suppression_tests {
    use super::*;

    fn node(id: u64, lat: f64, lon: f64) -> OsmElement {
        OsmElement {
            r#type: "node".into(),
            id,
            lat: Some(lat),
            lon: Some(lon),
            nodes: None,
            tags: None,
            members: Vec::new(),
        }
    }

    // Axis-aligned square way with corner (0,0) and the given side length.
    fn square_way(id: u64, first_node_id: u64, side: f64) -> (OsmElement, Vec<OsmElement>) {
        let corners = [(0.0, 0.0), (0.0, side), (side, side), (side, 0.0)];
        let nodes: Vec<OsmElement> = corners
            .iter()
            .enumerate()
            .map(|(i, &(lat, lon))| node(first_node_id + i as u64, lat, lon))
            .collect();
        let mut ids: Vec<u64> = nodes.iter().map(|n| n.id).collect();
        ids.push(first_node_id);
        let way = OsmElement {
            r#type: "way".into(),
            id,
            lat: None,
            lon: None,
            nodes: Some(ids),
            tags: None,
            members: Vec::new(),
        };
        (way, nodes)
    }

    fn member(kind: &str, r#ref: u64, role: &str) -> OsmMember {
        OsmMember {
            r#type: kind.into(),
            r#ref,
            r#role: role.into(),
        }
    }

    fn building_relation(members: Vec<OsmMember>) -> OsmElement {
        OsmElement {
            r#type: "relation".into(),
            id: 1,
            lat: None,
            lon: None,
            nodes: None,
            tags: Some(HashMap::from([(
                "type".to_string(),
                "building".to_string(),
            )])),
            members,
        }
    }

    // Therme Erding: parts cover ~24% of the outline, so the outline must survive.
    #[test]
    fn sparse_way_parts_keep_the_outline() {
        let (outline, outline_nodes) = square_way(100, 1000, 1.0);
        let (part, part_nodes) = square_way(200, 2000, 0.5);
        let rel = building_relation(vec![
            member("way", 100, "outline"),
            member("way", 200, "part"),
        ]);

        let nodes: Vec<OsmElement> = outline_nodes.into_iter().chain(part_nodes).collect();
        let suppressed =
            compute_outline_suppression(&[rel], &[outline, part], &nodes, &mut PartGroups::new());

        assert!(!suppressed.contains(&("way", 100)));
    }

    // Well-tiled parts (64% coverage) stand in for the outline, so it is dropped.
    #[test]
    fn covering_way_parts_suppress_the_outline() {
        let (outline, outline_nodes) = square_way(100, 1000, 1.0);
        let (part, part_nodes) = square_way(200, 2000, 0.8);
        let rel = building_relation(vec![
            member("way", 100, "outline"),
            member("way", 200, "part"),
        ]);

        let nodes: Vec<OsmElement> = outline_nodes.into_iter().chain(part_nodes).collect();
        let suppressed =
            compute_outline_suppression(&[rel], &[outline, part], &nodes, &mut PartGroups::new());

        assert!(suppressed.contains(&("way", 100)));
    }

    // Sub-relation parts carry no way geometry, so fall back to always suppressing.
    #[test]
    fn relation_parts_suppress_the_outline() {
        let (outline, outline_nodes) = square_way(100, 1000, 1.0);
        let rel = building_relation(vec![
            member("way", 100, "outline"),
            member("relation", 300, "part"),
        ]);

        let suppressed =
            compute_outline_suppression(&[rel], &[outline], &outline_nodes, &mut PartGroups::new());

        assert!(suppressed.contains(&("way", 100)));
    }

    // No parts at all: nothing is ever suppressed.
    #[test]
    fn outline_without_parts_is_kept() {
        let (outline, outline_nodes) = square_way(100, 1000, 1.0);
        let rel = building_relation(vec![member("way", 100, "outline")]);

        let suppressed =
            compute_outline_suppression(&[rel], &[outline], &outline_nodes, &mut PartGroups::new());

        assert!(suppressed.is_empty());
    }

    fn tagged(mut w: OsmElement, k: &str, v: &str) -> OsmElement {
        w.tags = Some(HashMap::from([(k.to_string(), v.to_string())]));
        w
    }

    // A building:part covering >=50% of a relation-less outline suppresses it.
    #[test]
    fn spatial_part_covering_outline_suppresses_it() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (p, pn) = square_way(200, 2000, 0.8);
        let ways = [
            tagged(o, "building", "yes"),
            tagged(p, "building:part", "yes"),
        ];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let s = compute_spatial_part_suppression(&ways, &nodes, &mut PartGroups::new());
        assert!(s.contains(&("way", 100)));
    }

    // An open (unclosed) part ring is ignored, so it can't suppress the outline.
    #[test]
    fn spatial_open_part_is_ignored() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (mut p, pn) = square_way(200, 2000, 0.8);
        // drop the closing node so first != last
        p.nodes.as_mut().unwrap().pop();
        let ways = [
            tagged(o, "building", "yes"),
            tagged(p, "building:part", "yes"),
        ];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let s = compute_spatial_part_suppression(&ways, &nodes, &mut PartGroups::new());
        assert!(!s.contains(&("way", 100)));
    }

    // A sparse part (25% coverage) leaves the outline in place.
    #[test]
    fn spatial_sparse_part_keeps_outline() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (p, pn) = square_way(200, 2000, 0.5);
        let ways = [
            tagged(o, "building", "yes"),
            tagged(p, "building:part", "yes"),
        ];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let s = compute_spatial_part_suppression(&ways, &nodes, &mut PartGroups::new());
        assert!(!s.contains(&("way", 100)));
    }

    // building:part=no marks the outline, not a part, so nothing suppresses it.
    #[test]
    fn spatial_outline_without_parts_is_kept() {
        let (o, on) = square_way(100, 1000, 1.0);
        let ways = [tagged(o, "building", "commercial")];
        let s = compute_spatial_part_suppression(&ways, &on, &mut PartGroups::new());
        assert!(s.is_empty());
    }

    // A contained part is grouped under its outline way id (shared style seed).
    #[test]
    fn spatial_part_is_grouped_under_its_outline() {
        let (o, on) = square_way(100, 1000, 1.0);
        let (p, pn) = square_way(200, 2000, 0.5);
        let ways = [
            tagged(o, "building", "yes"),
            tagged(p, "building:part", "yes"),
        ];
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let mut groups = PartGroups::new();
        compute_spatial_part_suppression(&ways, &nodes, &mut groups);
        assert_eq!(groups.get(&200), Some(&100));
    }

    // A relation part is grouped under the salted relation id.
    #[test]
    fn relation_part_is_grouped_under_salted_relation_id() {
        let (outline, on) = square_way(100, 1000, 1.0);
        let (part, pn) = square_way(200, 2000, 0.8);
        let rel = building_relation(vec![
            member("way", 100, "outline"),
            member("way", 200, "part"),
        ]);
        let nodes: Vec<OsmElement> = on.into_iter().chain(pn).collect();
        let mut groups = PartGroups::new();
        compute_outline_suppression(&[rel], &[outline, part], &nodes, &mut groups);
        assert_eq!(groups.get(&200), Some(&(RELATION_SEED_BIT | 1)));
    }

    // building_relation hardcodes id 1 and type=building; these need both to vary.
    fn relation_with(id: u64, kind: &str, members: Vec<OsmMember>) -> OsmElement {
        OsmElement {
            r#type: "relation".into(),
            id,
            lat: None,
            lon: None,
            nodes: None,
            tags: Some(HashMap::from([("type".to_string(), kind.to_string())])),
            members,
        }
    }

    // Every outer is a closed building:part way, so the relation ring is a pure duplicate.
    #[test]
    fn part_way_outers_suppress_the_relation() {
        let (a, _) = square_way(100, 1000, 1.0);
        let (b, _) = square_way(101, 1100, 0.5);
        let ways = [
            tagged(a, "building:part", "yes"),
            tagged(b, "building:part", "yes"),
        ];
        let rel = relation_with(
            7,
            "multipolygon",
            vec![member("way", 100, "outer"), member("way", 101, "outer")],
        );

        let s = compute_part_way_outline_suppression(&[rel], &ways);

        assert!(s.contains(&("relation", 7)));
    }

    // One plain building outer means the relation still contributes real geometry.
    #[test]
    fn mixed_outers_keep_the_relation() {
        let (a, _) = square_way(100, 1000, 1.0);
        let (b, _) = square_way(101, 1100, 0.5);
        let ways = [
            tagged(a, "building:part", "yes"),
            tagged(b, "building", "yes"),
        ];
        let rel = relation_with(
            7,
            "multipolygon",
            vec![member("way", 100, "outer"), member("way", 101, "outer")],
        );

        let s = compute_part_way_outline_suppression(&[rel], &ways);

        assert!(!s.contains(&("relation", 7)));
    }

    // Anti-truncation guard: a member way clipped away by the cell bbox must not let the
    // shrunken outer set satisfy the all(), which would suppress here but not in the neighbour.
    #[test]
    fn missing_member_way_keeps_the_relation() {
        let (a, _) = square_way(100, 1000, 1.0);
        let ways = [tagged(a, "building:part", "yes")];
        let rel = relation_with(
            7,
            "multipolygon",
            vec![member("way", 100, "outer"), member("way", 101, "outer")],
        );

        let s = compute_part_way_outline_suppression(&[rel], &ways);

        assert!(!s.contains(&("relation", 7)));
    }

    // type=site is never rendered by parse_osm_data, so it has no outline to suppress.
    #[test]
    fn non_renderable_relation_is_ignored() {
        let (a, _) = square_way(100, 1000, 1.0);
        let (b, _) = square_way(101, 1100, 0.5);
        let ways = [
            tagged(a, "building:part", "yes"),
            tagged(b, "building:part", "yes"),
        ];
        let rel = relation_with(
            7,
            "site",
            vec![member("way", 100, "outer"), member("way", 101, "outer")],
        );

        let s = compute_part_way_outline_suppression(&[rel], &ways);

        assert!(!s.contains(&("relation", 7)));
    }

    // An open outer only renders as part of the merged ring, so the relation must stay.
    #[test]
    fn open_part_way_outer_keeps_the_relation() {
        let (a, _) = square_way(100, 1000, 1.0);
        let (mut b, _) = square_way(101, 1100, 0.5);
        // drop the closing node so first != last
        b.nodes.as_mut().unwrap().pop();
        let ways = [
            tagged(a, "building:part", "yes"),
            tagged(b, "building:part", "yes"),
        ];
        let rel = relation_with(
            7,
            "multipolygon",
            vec![member("way", 100, "outer"), member("way", 101, "outer")],
        );

        let s = compute_part_way_outline_suppression(&[rel], &ways);

        assert!(!s.contains(&("relation", 7)));
    }
}

#[cfg(test)]
mod arch_era_tests {
    use super::*;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn era_derivation_table() {
        use ArchEra::*;
        let cases: &[(&[(&str, &str)], ArchEra)] = &[
            (&[("historic", "yes")], HistoricOrnate),
            (&[("heritage", "2")], HistoricOrnate),
            (&[("historic", "no")], Unknown),
            (&[("building:architecture", "art_deco")], HistoricOrnate),
            (&[("building:architecture", "brutalist")], PostWarPanel),
            (&[("building:architecture", "bauhaus")], Contemporary),
            (&[("start_date", "1890")], TraditionalPreWar),
            (&[("start_date", "1965")], PostWarPanel),
            (&[("start_date", "2003-05")], Contemporary),
            (&[("building:material", "panel")], PostWarPanel),
            (&[("building:material", "brick")], TraditionalPreWar),
            (&[("building:material", "concrete")], Contemporary),
            (&[("building:cladding", "plaster")], TraditionalPreWar),
            (&[("building", "house")], Unknown),
            // heritage beats a modern start_date
            (&[("heritage", "1"), ("start_date", "1995")], HistoricOrnate),
        ];
        for (pairs, expected) in cases {
            assert_eq!(building_arch_era(&tags(pairs)), *expected, "tags {pairs:?}");
        }
    }

    #[test]
    fn part_hint_fallback_mapping() {
        assert_eq!(
            arch_era_from_hint(StyleHint::Masonry),
            ArchEra::TraditionalPreWar
        );
        assert_eq!(arch_era_from_hint(StyleHint::Glass), ArchEra::Contemporary);
        assert_eq!(arch_era_from_hint(StyleHint::None), ArchEra::Unknown);
    }
}
