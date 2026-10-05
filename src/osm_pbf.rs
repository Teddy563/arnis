//! Reads OSM data from an `.osm.pbf` extract instead of the tile archive (`--osm-pbf`).
//!
//! The extract is a local file, or the smallest Geofabrik region whose border holds the
//! selection, downloaded once. A run cuts its area out of it the way the Overpass query
//! would and keeps the cut on disk (the bake), so a repeat run, and every piece of a job,
//! reads the bake instead of the extract.

use crate::args::Args;
use crate::coordinate_system::geographic::LLBBox;
use crate::osm_parser::{OsmData, OsmElement, OsmMember};
use crate::progress::emit_gui_progress_update;
use colored::Colorize;
use osmpbf::{BlobDecode, BlobReader, Element, PrimitiveBlock, RelMemberType};
use rayon::prelude::*;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// `--osm-pbf geofabrik`: the extract is picked from the selection and downloaded.
pub const GEOFABRIK: &str = "geofabrik";
const GEOFABRIK_INDEX_URL: &str = "https://download.geofabrik.de/index-v1.json";
/// Extract borders move over years; a week-old index names the same files.
const INDEX_MAX_AGE: Duration = Duration::from_secs(7 * 86_400);

type Result<T> = std::result::Result<T, String>;

/// What `--osm-pbf` and `--osm-pbf-url` ask for.
pub struct Source {
    spec: String,
    url: Option<String>,
    /// Kept past the bbox: the One World clip pad, in metres at this scale.
    pad_m: f64,
}

impl Source {
    pub fn from_args(args: &Args) -> Option<Self> {
        args.osm_pbf.as_ref().map(|spec| Source {
            spec: spec.clone(),
            url: args.osm_pbf_url.clone(),
            pad_m: f64::from(crate::one_world::CLIP_PAD_BLOCKS) / args.scale.max(1e-6),
        })
    }
}

/// The OSM data of `bbox`, cut from a bake that covers it, or from the extract.
pub fn fetch_data_from_pbf(src: &Source, bbox: LLBBox) -> Result<OsmData> {
    println!("{} Reading data from the .pbf extract...", "[1/7]".bold());
    emit_gui_progress_update(1.0, "Reading the .pbf extract...");
    let want = E7Box::around(&bbox, src.pad_m);
    let bake = bake(src, want)?;
    emit_gui_progress_update(5.0, "");
    Ok(cut(bake.elements(), want))
}

/// Bakes a whole job before its pieces start, so each piece cuts its own area from the
/// bake instead of reading the extract again.
pub fn bake_for_job(src: &Source, selection: LLBBox) -> Result<()> {
    // Twice the pad: a piece first snaps outward to the chunk grid (under 16 blocks), then
    // adds its own pad.
    bake(src, E7Box::around(&selection, 2.0 * src.pad_m)).map(drop)
}

fn cache_root() -> Result<PathBuf> {
    crate::elevation::cache::user_cache_dir()
        .map(|d| cache_root_in(&d))
        .ok_or_else(|| "no cache directory for the .pbf bake".to_string())
}

/// The extract cache under the cache root `root`.
fn cache_root_in(root: &Path) -> PathBuf {
    root.join("arnis").join("osm-pbf")
}

/// What `--osm-pbf` needs for a selection, as the caches hold it.
#[derive(serde::Serialize, Debug, Default, PartialEq)]
pub struct ExtractPlan {
    /// The Geofabrik region, or the file's name; `None` when no cached index
    /// names one.
    pub name: Option<String>,
    /// Size on disk, once downloaded.
    pub bytes: Option<u64>,
    pub downloaded: bool,
    /// A bake holding the selection is on disk.
    pub baked: bool,
}

/// [`ExtractPlan`] for `selection` from the caches under `root` alone: the
/// Geofabrik index is read only from disk, and nothing is downloaded.
pub fn plan(root: &Path, src: &Source, selection: LLBBox) -> ExtractPlan {
    let cache = cache_root_in(root);
    let want = E7Box::around(&selection, src.pad_m);
    let (name, pbf) = if src.spec == GEOFABRIK {
        let index = std::fs::read(cache.join("geofabrik-index.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
        let picked = match (&src.url, &index) {
            (Some(url), _) => Some((None, url.clone())),
            (None, Some(index)) => geofabrik_feature(index, &want).ok().map(|f| {
                let name = f["properties"]["name"].as_str().map(str::to_string);
                let url = f["properties"]["urls"]["pbf"].as_str().unwrap_or_default();
                (name, url.to_string())
            }),
            (None, None) => None,
        };
        let Some((region, url)) = picked else {
            return ExtractPlan::default();
        };
        let Some(file) = download_name(&url) else {
            return ExtractPlan::default();
        };
        let region = region.or_else(|| Some(file.to_string()));
        (region, cache.join("downloads").join(file))
    } else {
        let p = PathBuf::from(&src.spec);
        (p.file_name().map(|n| n.to_string_lossy().into_owned()), p)
    };
    let bytes = std::fs::metadata(&pbf)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len());
    let baked = bytes.is_some()
        && pbf_key(&pbf)
            .is_ok_and(|key| find_bake(&cache.join("bakes").join(key), &want).is_some());
    ExtractPlan {
        name,
        bytes,
        downloaded: bytes.is_some(),
        baked,
    }
}

/// The bake of `want`, read from disk when an earlier one covers it.
fn bake(src: &Source, want: E7Box) -> Result<OsmData> {
    let pbf = locate(src, &want)?;
    let dir = cache_root()?.join("bakes").join(pbf_key(&pbf)?);
    if let Some(hit) = find_bake(&dir, &want) {
        println!("Reading the bake {}", hit.display());
        return read_bake(&hit);
    }
    let t = Instant::now();
    let data = cut_pbf(&pbf, want)?;
    println!(
        "Baked {} elements from {} in {:.1}s on {} threads",
        data.elements().len(),
        pbf.display(),
        t.elapsed().as_secs_f64(),
        rayon::current_num_threads()
    );
    let mut enc = zstd::stream::Encoder::new(Vec::new(), 3).map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut enc, &data).map_err(|e| e.to_string())?;
    let bytes = enc.finish().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    crate::world_utils::replace_file_atomically(&dir.join(want.file_name()), &bytes)?;
    Ok(data)
}

fn read_bake(path: &Path) -> Result<OsmData> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let dec = zstd::stream::Decoder::new(file).map_err(|e| e.to_string())?;
    serde_json::from_reader(std::io::BufReader::new(dec))
        .map_err(|e| format!("bad bake {}: {e}", path.display()))
}

/// The smallest bake in `dir` holding all of `want`.
fn find_bake(dir: &Path, want: &E7Box) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let b = E7Box::from_file_name(&name)?;
            b.contains(want).then(|| (b.area(), e.path()))
        })
        .min_by_key(|(area, _)| *area)
        .map(|(_, p)| p)
}

/// Names one version of an extract: a re-download gets a new size or time, and its bakes.
fn pbf_key(pbf: &Path) -> Result<String> {
    let md = std::fs::metadata(pbf).map_err(|e| format!("{}: {e}", pbf.display()))?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let name = pbf.file_name().map(|n| n.to_string_lossy().into_owned());
    let digest = Sha256::digest(format!("{name:?}|{}|{mtime}", md.len()));
    Ok(digest[..8].iter().map(|b| format!("{b:02x}")).collect())
}

// ── the extract ───────────────────────────────────────────────────────────────

/// The extract on disk, downloaded first for `geofabrik`.
fn locate(src: &Source, want: &E7Box) -> Result<PathBuf> {
    if src.spec != GEOFABRIK {
        let p = PathBuf::from(&src.spec);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!("--osm-pbf: {} is not a file", p.display()))
        };
    }
    let url = match &src.url {
        Some(url) => url.clone(),
        None => geofabrik_url(&fetch_index()?, want)?,
    };
    let name = download_name(&url).ok_or_else(|| format!("not a .pbf url: {url}"))?;
    let dest = cache_root()?.join("downloads").join(name);
    if !dest.is_file() {
        download(&url, &dest)?;
    }
    Ok(dest)
}

/// The file name a downloaded extract is kept under: the url's last part,
/// when that is a plain `.pbf` name.
fn download_name(url: &str) -> Option<&str> {
    url.rsplit('/')
        .next()
        .filter(|n| n.ends_with(".pbf") && !n.contains(['\\', ':']) && !n.starts_with('.'))
}

fn client() -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        // Per read, not per transfer: a country is hundreds of megabytes.
        .timeout(Duration::from_secs(120))
        .user_agent(crate::retrieve_data::OSM_USER_AGENT)
        .build()
        .map_err(|e| e.to_string())
}

/// Streams `url` to `<dest>.part`, then renames: a half file never wears the real name.
fn download(url: &str, dest: &Path) -> Result<()> {
    crate::net::ensure_online("OpenStreetMap extract (.pbf)")?;
    let _permit = crate::net::request_permit();
    println!("Downloading {url}");
    let mut resp = client()?
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("{url}: {e}"))?;
    let total = resp.content_length().unwrap_or(0);
    let dir = dest.parent().ok_or("download has no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let part = dest.with_extension("pbf.part");
    let result = (|| {
        let mut out = std::fs::File::create(&part).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 1 << 20];
        let (mut done, mut shown) = (0u64, 0u64);
        loop {
            let n = resp.read(&mut buf).map_err(|e| format!("{url}: {e}"))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            if done - shown >= 32 << 20 {
                shown = done;
                println!("  {:.0} / {:.0} MB", done as f64 / 1e6, total as f64 / 1e6);
                if total > 0 {
                    emit_gui_progress_update(1.0 + 3.0 * done as f64 / total as f64, "");
                }
            }
        }
        // A dropped connection can end as a clean EOF.
        if total > 0 && done < total {
            return Err(format!(
                "{url}: connection dropped at {done} of {total} bytes"
            ));
        }
        out.sync_all().map_err(|e| e.to_string())?;
        drop(out);
        std::fs::rename(&part, dest).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// The Geofabrik index, from a cached copy younger than a week (any age offline).
fn fetch_index() -> Result<serde_json::Value> {
    let path = cache_root()?.join("geofabrik-index.json");
    let age = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok());
    if age.is_some_and(|a| a < INDEX_MAX_AGE || crate::net::offline()) {
        let cached = std::fs::read(&path).ok();
        if let Some(v) = cached.and_then(|b| serde_json::from_slice(&b).ok()) {
            return Ok(v);
        }
    }
    crate::net::ensure_online("Geofabrik extract index")?;
    let body = {
        let _permit = crate::net::request_permit();
        client()?
            .get(GEOFABRIK_INDEX_URL)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.bytes())
            .map_err(|e| format!("Geofabrik index unreachable: {e}"))?
    };
    let v = serde_json::from_slice(&body).map_err(|e| format!("bad Geofabrik index: {e}"))?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = crate::world_utils::replace_file_atomically(&path, &body);
    Ok(v)
}

/// The .pbf url of the smallest extract whose border holds the whole selection. A bbox
/// test would lie (a country's bbox spans its neighbours), so a 5x5 grid of points across
/// the selection is tested against the border polygon, ranked by polygon area.
fn geofabrik_url(index: &serde_json::Value, want: &E7Box) -> Result<String> {
    geofabrik_feature(index, want).and_then(|f| {
        f["properties"]["urls"]["pbf"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "bad Geofabrik index".to_string())
    })
}

/// The index entry of the smallest extract holding the selection.
fn geofabrik_feature<'a>(
    index: &'a serde_json::Value,
    want: &E7Box,
) -> Result<&'a serde_json::Value> {
    const N: i64 = 5;
    let points: Vec<(f64, f64)> = (0..N * N)
        .map(|k| {
            let (i, j) = (k / N, k % N);
            let lon = want.min_lon + (want.max_lon - want.min_lon) * j / (N - 1);
            let lat = want.min_lat + (want.max_lat - want.min_lat) * i / (N - 1);
            (lon as f64 / 1e7, lat as f64 / 1e7)
        })
        .collect();
    let features = index["features"].as_array().ok_or("bad Geofabrik index")?;
    features
        .iter()
        .filter_map(|f| {
            f["properties"]["urls"]["pbf"].as_str()?;
            let rings = rings(&f["geometry"]);
            (!rings.is_empty() && points.iter().all(|&(x, y)| in_rings(x, y, &rings)))
                .then(|| (rings_area(&rings), f))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, f)| f)
        .ok_or_else(|| {
            "no single Geofabrik extract holds this selection; pass a .pbf file with --osm-pbf"
                .to_string()
        })
}

type Ring = Vec<(f64, f64)>;

/// A GeoJSON (Multi)Polygon as a flat ring list; even-odd needs no outer/hole bookkeeping.
fn rings(geom: &serde_json::Value) -> Vec<Ring> {
    let ring = |r: &serde_json::Value| -> Ring {
        r.as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| Some((p[0].as_f64()?, p[1].as_f64()?)))
            .collect()
    };
    let polys: Vec<&serde_json::Value> = match geom["type"].as_str() {
        Some("Polygon") => vec![&geom["coordinates"]],
        Some("MultiPolygon") => geom["coordinates"]
            .as_array()
            .into_iter()
            .flatten()
            .collect(),
        _ => Vec::new(),
    };
    polys
        .into_iter()
        .flat_map(|p| p.as_array().into_iter().flatten().map(ring))
        .filter(|r| r.len() > 2)
        .collect()
}

fn in_rings(x: f64, y: f64, rings: &[Ring]) -> bool {
    let mut inside = false;
    for ring in rings {
        let mut j = ring.len() - 1;
        for i in 0..ring.len() {
            let ((xi, yi), (xj, yj)) = (ring[i], ring[j]);
            if (yi > y) != (yj > y) && x < xj + (y - yj) * (xi - xj) / (yi - yj) {
                inside = !inside;
            }
            j = i;
        }
    }
    inside
}

/// Sum of ring areas, deg². A ranking key: a bbox area would rank France (overseas
/// territories) above Europe.
fn rings_area(rings: &[Ring]) -> f64 {
    rings
        .iter()
        .map(|r| {
            let mut a = 0.0;
            let mut j = r.len() - 1;
            for i in 0..r.len() {
                a += (r[j].0 + r[i].0) * (r[j].1 - r[i].1);
                j = i;
            }
            a.abs() / 2.0
        })
        .sum()
}

// ── the cut ───────────────────────────────────────────────────────────────────

/// A lat/lon box in degrees x 1e7, the .pbf's own unit, so containment is exact.
#[derive(Clone, Copy, Debug, PartialEq)]
struct E7Box {
    min_lat: i64,
    min_lon: i64,
    max_lat: i64,
    max_lon: i64,
}

impl E7Box {
    fn around(bbox: &LLBBox, pad_m: f64) -> Self {
        let mid = (bbox.min().lat() + bbox.max().lat()) / 2.0;
        let dlat = pad_m / 111_320.0;
        let dlon = pad_m / (111_320.0 * mid.to_radians().cos().max(0.01));
        E7Box {
            min_lat: ((bbox.min().lat() - dlat) * 1e7).floor().max(-90e7) as i64,
            min_lon: ((bbox.min().lng() - dlon) * 1e7).floor().max(-180e7) as i64,
            max_lat: ((bbox.max().lat() + dlat) * 1e7).ceil().min(90e7) as i64,
            max_lon: ((bbox.max().lng() + dlon) * 1e7).ceil().min(180e7) as i64,
        }
    }

    fn point(lat: i64, lon: i64) -> Self {
        E7Box {
            min_lat: lat,
            min_lon: lon,
            max_lat: lat,
            max_lon: lon,
        }
    }

    fn union(self, o: Self) -> Self {
        E7Box {
            min_lat: self.min_lat.min(o.min_lat),
            min_lon: self.min_lon.min(o.min_lon),
            max_lat: self.max_lat.max(o.max_lat),
            max_lon: self.max_lon.max(o.max_lon),
        }
    }

    fn intersects(&self, o: &Self) -> bool {
        self.min_lat <= o.max_lat
            && o.min_lat <= self.max_lat
            && self.min_lon <= o.max_lon
            && o.min_lon <= self.max_lon
    }

    fn contains(&self, o: &Self) -> bool {
        self.min_lat <= o.min_lat
            && self.min_lon <= o.min_lon
            && self.max_lat >= o.max_lat
            && self.max_lon >= o.max_lon
    }

    fn holds(&self, lat: i64, lon: i64) -> bool {
        self.contains(&E7Box::point(lat, lon))
    }

    fn area(&self) -> i128 {
        i128::from(self.max_lat - self.min_lat) * i128::from(self.max_lon - self.min_lon)
    }

    fn file_name(&self) -> String {
        format!(
            "{}_{}_{}_{}.json.zst",
            self.min_lat, self.min_lon, self.max_lat, self.max_lon
        )
    }

    fn from_file_name(name: &str) -> Option<Self> {
        let v: Vec<i64> = name
            .strip_suffix(".json.zst")?
            .split('_')
            .map(|s| s.parse().ok())
            .collect::<Option<_>>()?;
        match v[..] {
            [min_lat, min_lon, max_lat, max_lon] => Some(E7Box {
                min_lat,
                min_lon,
                max_lat,
                max_lon,
            }),
            _ => None,
        }
    }
}

fn e7(deg: f64) -> i64 {
    (deg * 1e7).round() as i64
}

/// Whether the Overpass query asks for an element with these tags (see
/// `fetch_data_from_overpass`). It decides which nodes keep their tags and which
/// relations come along; every way is wanted (`way;`).
fn wanted<'a>(tag: impl Fn(&str) -> Option<&'a str>) -> bool {
    const ANY_VALUE: &[&str] = &[
        "building",
        "building:part",
        "highway",
        "leisure",
        "amenity",
        "tourism",
        "bridge",
        "railway",
        "roller_coaster",
        "barrier",
        "entrance",
        "door",
        "power",
        "historic",
        "emergency",
        "advertising",
        "man_made",
        "aeroway",
        "3dmr",
        "shop",
        "office",
    ];
    ANY_VALUE.iter().any(|k| tag(k).is_some())
        || tag("type") == Some("building")
        || tag("landuse").is_some_and(|v| v != "salt_pond")
        || tag("natural").is_some_and(|v| !matches!(v, "coastline" | "bay" | "strait"))
        || (tag("water").is_some_and(|v| !matches!(v, "bay" | "ocean" | "sea"))
            && tag("tidal") != Some("yes"))
        || tag("waterway").is_some_and(|v| v != "tidal_channel")
}

fn wanted_map(tags: &Option<HashMap<String, String>>) -> bool {
    tags.as_ref()
        .is_some_and(|t| wanted(|k| t.get(k).map(String::as_str)))
}

/// What the Overpass query returns for `b`, from elements that hold at least that:
/// every way whose extent meets `b` (an enclosing lake too, as with the tile archive),
/// every wanted relation whose member ways' extent does, with all of its member ways,
/// the nodes of those ways, and the wanted tagged nodes inside `b`. A node keeps its tags
/// only as such a node. Sorted by id, so the bake and any cut of it are deterministic,
/// and a cut of a bake equals the cut of the extract.
fn cut(elements: &[OsmElement], b: E7Box) -> OsmData {
    let mut nodes: HashMap<u64, &OsmElement> = HashMap::new();
    let mut ways: HashMap<u64, &OsmElement> = HashMap::new();
    let mut rels: Vec<&OsmElement> = Vec::new();
    for e in elements {
        match e.r#type.as_str() {
            "node" => {
                nodes.entry(e.id).or_insert(e);
            }
            "way" => {
                ways.entry(e.id).or_insert(e);
            }
            "relation" => rels.push(e),
            _ => {}
        }
    }
    let at = |id: &u64| {
        let n = nodes.get(id)?;
        Some((e7(n.lat?), e7(n.lon?)))
    };
    let extent: HashMap<u64, E7Box> = ways
        .iter()
        .filter_map(|(&id, w)| {
            let e = w
                .nodes
                .iter()
                .flatten()
                .filter_map(at)
                .map(|(lat, lon)| E7Box::point(lat, lon))
                .reduce(E7Box::union)?;
            Some((id, e))
        })
        .collect();

    let mut keep_ways: HashSet<u64> = extent
        .iter()
        .filter(|(_, e)| e.intersects(&b))
        .map(|(&id, _)| id)
        .collect();
    let mut keep_rels: Vec<&OsmElement> = rels
        .into_iter()
        .filter(|r| wanted_map(&r.tags))
        .filter(|r| {
            r.members
                .iter()
                .filter(|m| m.r#type == "way")
                .filter_map(|m| extent.get(&m.r#ref).copied())
                .reduce(E7Box::union)
                .is_some_and(|e| e.intersects(&b))
        })
        .collect();
    keep_rels.sort_unstable_by_key(|r| r.id);
    keep_rels.dedup_by_key(|r| r.id);
    for r in &keep_rels {
        keep_ways.extend(
            r.members
                .iter()
                .filter(|m| m.r#type == "way" && ways.contains_key(&m.r#ref))
                .map(|m| m.r#ref),
        );
    }

    let tagged_inside = |n: &OsmElement| {
        wanted_map(&n.tags)
            && matches!((n.lat, n.lon), (Some(lat), Some(lon)) if b.holds(e7(lat), e7(lon)))
    };
    let mut keep_nodes: HashSet<u64> = nodes
        .values()
        .filter(|n| tagged_inside(n))
        .map(|n| n.id)
        .collect();
    for id in &keep_ways {
        keep_nodes.extend(
            ways[id]
                .nodes
                .iter()
                .flatten()
                .filter(|n| nodes.contains_key(n)),
        );
    }

    let mut node_ids: Vec<u64> = keep_nodes.into_iter().collect();
    node_ids.sort_unstable();
    let mut way_ids: Vec<u64> = keep_ways.into_iter().collect();
    way_ids.sort_unstable();
    let mut out: Vec<OsmElement> = Vec::with_capacity(node_ids.len() + way_ids.len());
    out.extend(node_ids.iter().map(|id| {
        let n = nodes[id];
        OsmElement {
            r#type: "node".into(),
            id: n.id,
            lat: n.lat,
            lon: n.lon,
            nodes: None,
            tags: n.tags.clone().filter(|_| tagged_inside(n)),
            members: Vec::new(),
        }
    }));
    out.extend(way_ids.iter().map(|id| ways[id].clone()));
    out.extend(keep_rels.into_iter().cloned());
    OsmData::from_elements(out)
}

/// Runs `f` over every data block of the extract on the rayon pool, so `--threads` and
/// the piece's thread share decide how many blocks decode at once.
fn par_blocks<T: Send>(
    pbf: &Path,
    f: impl Fn(&PrimitiveBlock) -> T + Sync + Send,
) -> Result<Vec<T>> {
    let reader = BlobReader::from_path(pbf).map_err(|e| format!("{}: {e}", pbf.display()))?;
    reader
        .par_bridge()
        .filter_map(|blob| {
            let decoded = blob.and_then(|b| match b.decode()? {
                BlobDecode::OsmData(block) => Ok(Some(f(&block))),
                _ => Ok(None),
            });
            decoded.transpose()
        })
        .collect::<std::result::Result<Vec<T>, _>>()
        .map_err(|e| format!("{}: {e}", pbf.display()))
}

fn tag_map<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> HashMap<String, String> {
    tags.map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn node_element(id: i64, lat: i32, lon: i32, tags: Option<HashMap<String, String>>) -> OsmElement {
    OsmElement {
        r#type: "node".into(),
        id: id as u64,
        lat: Some(f64::from(lat) / 1e7),
        lon: Some(f64::from(lon) / 1e7),
        nodes: None,
        tags,
        members: Vec::new(),
    }
}

/// [`cut`] of the extract. The first pass keeps every node's location (16 bytes a node, as
/// osmium's own index) and the wanted relations; the second the ways that meet the area,
/// and only the extent of other relation members; a third, when needed, the members of the
/// relations that meet the area.
fn cut_pbf(pbf: &Path, want: E7Box) -> Result<OsmData> {
    type Pass1 = (Vec<(i64, i32, i32)>, Vec<OsmElement>, Vec<OsmElement>);
    let pass1: Vec<Pass1> = par_blocks(pbf, |block| {
        // A dense block holds 8000 nodes: reserved, then trimmed, it never doubles past that.
        let (mut locs, mut tagged, mut rels) = (Vec::with_capacity(8000), Vec::new(), Vec::new());
        let mut node = |id: i64, lat: i32, lon: i32, tags: Vec<(&str, &str)>| {
            locs.push((id, lat, lon));
            let find = |k: &str| tags.iter().find(|t| t.0 == k).map(|t| t.1);
            if want.holds(lat.into(), lon.into()) && wanted(find) {
                tagged.push(node_element(id, lat, lon, Some(tag_map(tags.into_iter()))));
            }
        };
        for el in block.elements() {
            match el {
                Element::Node(n) => node(
                    n.id(),
                    n.decimicro_lat(),
                    n.decimicro_lon(),
                    n.tags().collect(),
                ),
                Element::DenseNode(n) => node(
                    n.id(),
                    n.decimicro_lat(),
                    n.decimicro_lon(),
                    n.tags().collect(),
                ),
                Element::Relation(r) => {
                    let tags: Vec<(&str, &str)> = r.tags().collect();
                    if !wanted(|k| tags.iter().find(|t| t.0 == k).map(|t| t.1)) {
                        continue;
                    }
                    rels.push(OsmElement {
                        r#type: "relation".into(),
                        id: r.id() as u64,
                        lat: None,
                        lon: None,
                        nodes: None,
                        tags: Some(tag_map(tags.into_iter())),
                        members: r
                            .members()
                            .map(|m| OsmMember {
                                r#type: match m.member_type {
                                    RelMemberType::Node => "node",
                                    RelMemberType::Way => "way",
                                    RelMemberType::Relation => "relation",
                                }
                                .into(),
                                r#ref: m.member_id as u64,
                                role: m.role().unwrap_or_default().to_string(),
                            })
                            .collect(),
                    });
                }
                Element::Way(_) => {}
            }
        }
        locs.shrink_to_fit();
        (locs, tagged, rels)
    })?;
    // Sized up front: growing by doubling would hold up to twice the index at once.
    let mut locs: Vec<(i64, i32, i32)> = Vec::with_capacity(pass1.iter().map(|p| p.0.len()).sum());
    let (mut tagged, mut rels): (Vec<OsmElement>, Vec<OsmElement>) = (Vec::new(), Vec::new());
    for (l, t, r) in pass1 {
        locs.extend(l);
        tagged.extend(t);
        rels.extend(r);
    }
    locs.par_sort_unstable_by_key(|n| n.0);
    let loc = |id: i64| {
        locs.binary_search_by_key(&id, |n| n.0)
            .ok()
            .map(|i| (locs[i].1, locs[i].2))
    };
    let extent = |refs: &[i64]| {
        refs.iter()
            .filter_map(|&id| loc(id))
            .map(|(lat, lon)| E7Box::point(lat.into(), lon.into()))
            .reduce(E7Box::union)
    };
    let way_ids = |r: &OsmElement| -> Vec<i64> {
        r.members
            .iter()
            .filter(|m| m.r#type == "way")
            .map(|m| m.r#ref as i64)
            .collect()
    };
    let members: HashSet<i64> = rels.iter().flat_map(way_ids).collect();

    type Pass2 = (Vec<OsmElement>, Vec<(i64, E7Box)>);
    let pass2: Vec<Pass2> = par_blocks(pbf, |block| {
        let (mut full, mut outside) = (Vec::new(), Vec::new());
        for el in block.elements() {
            let Element::Way(w) = el else { continue };
            let refs: Vec<i64> = w.refs().collect();
            match extent(&refs) {
                Some(e) if e.intersects(&want) => full.push(way_element(&w, refs)),
                Some(e) if members.contains(&w.id()) => outside.push((w.id(), e)),
                _ => {}
            }
        }
        (full, outside)
    })?;
    let mut ways: Vec<OsmElement> = Vec::new();
    let mut outside: HashMap<i64, E7Box> = HashMap::new();
    for (f, o) in pass2 {
        ways.extend(f);
        outside.extend(o);
    }
    let inside: HashSet<i64> = ways.iter().map(|w| w.id as i64).collect();
    // As in the cut: a member meeting the area, or members that together enclose it.
    rels.retain(|r| {
        let ids = way_ids(r);
        ids.iter().any(|id| inside.contains(id))
            || ids
                .iter()
                .filter_map(|id| outside.get(id).copied())
                .reduce(E7Box::union)
                .is_some_and(|e| e.intersects(&want))
    });
    let missing: HashSet<i64> = rels
        .iter()
        .flat_map(way_ids)
        .filter(|id| outside.contains_key(id))
        .collect();
    drop(outside);
    if !missing.is_empty() {
        let pass3: Vec<Vec<OsmElement>> = par_blocks(pbf, |block| {
            block
                .elements()
                .filter_map(|el| match el {
                    Element::Way(w) if missing.contains(&w.id()) => {
                        Some(way_element(&w, w.refs().collect()))
                    }
                    _ => None,
                })
                .collect()
        })?;
        ways.extend(pass3.into_iter().flatten());
    }

    // Tagged nodes first: the cut keeps the first copy of a node.
    let mut elements = tagged;
    let vertices: HashSet<i64> = ways
        .iter()
        .flat_map(|w| w.nodes.iter().flatten())
        .map(|&id| id as i64)
        .collect();
    for id in vertices {
        if let Some((lat, lon)) = loc(id) {
            elements.push(node_element(id, lat, lon, None));
        }
    }
    elements.extend(ways);
    elements.extend(rels);
    Ok(cut(&elements, want))
}

fn way_element(w: &osmpbf::Way, refs: Vec<i64>) -> OsmElement {
    let tags = tag_map(w.tags());
    OsmElement {
        r#type: "way".into(),
        id: w.id() as u64,
        lat: None,
        lon: None,
        nodes: Some(refs.into_iter().map(|r| r as u64).collect()),
        tags: (!tags.is_empty()).then_some(tags),
        members: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64, lat: f64, lon: f64, tags: &[(&str, &str)]) -> OsmElement {
        OsmElement {
            r#type: "node".into(),
            id,
            lat: Some(lat),
            lon: Some(lon),
            nodes: None,
            tags: (!tags.is_empty()).then(|| {
                tags.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            }),
            members: Vec::new(),
        }
    }

    fn way(id: u64, nodes: &[u64], tags: &[(&str, &str)]) -> OsmElement {
        OsmElement {
            r#type: "way".into(),
            id,
            lat: None,
            lon: None,
            nodes: Some(nodes.to_vec()),
            tags: Some(
                tags.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            members: Vec::new(),
        }
    }

    fn ids(d: &OsmData, kind: &str) -> Vec<u64> {
        d.elements()
            .iter()
            .filter(|e| e.r#type == kind)
            .map(|e| e.id)
            .collect()
    }

    fn bbox(min_lat: f64, min_lon: f64, max_lat: f64, max_lon: f64) -> E7Box {
        E7Box::around(
            &LLBBox::new(min_lat, min_lon, max_lat, max_lon).unwrap(),
            0.0,
        )
    }

    #[test]
    fn cut_keeps_what_overpass_would_and_a_cut_of_a_cut_agrees() {
        let lake = way(20, &[1, 2, 3, 4, 1], &[("natural", "water")]);
        let elements = vec![
            // A lake whose corners are all outside the area, around it.
            node(1, 0.0, 0.0, &[]),
            node(2, 0.0, 1.0, &[]),
            node(3, 1.0, 1.0, &[]),
            node(4, 1.0, 0.0, &[]),
            lake,
            // A road crossing into the area, and a bench in it; a name-only node is not asked for.
            node(5, 0.5, 0.5, &[("highway", "crossing")]),
            node(6, 0.6, 0.9, &[]),
            way(21, &[5, 6], &[("highway", "residential")]),
            node(7, 0.51, 0.51, &[("amenity", "bench")]),
            node(8, 0.52, 0.52, &[("name", "x")]),
            // Far away: a building, and a route relation (not asked for) holding it.
            node(9, 5.0, 5.0, &[]),
            node(10, 5.0, 5.1, &[]),
            way(22, &[9, 10], &[("building", "yes")]),
            OsmElement {
                r#type: "relation".into(),
                id: 30,
                lat: None,
                lon: None,
                nodes: None,
                tags: Some([("route".to_string(), "bus".to_string())].into()),
                members: vec![
                    OsmMember {
                        r#type: "way".into(),
                        r#ref: 21,
                        role: String::new(),
                    },
                    OsmMember {
                        r#type: "way".into(),
                        r#ref: 22,
                        role: String::new(),
                    },
                ],
            },
        ];
        let big = bbox(0.4, 0.4, 0.6, 0.6);
        let d = cut(&elements, big);
        assert_eq!(ids(&d, "way"), vec![20, 21]);
        assert_eq!(ids(&d, "node"), vec![1, 2, 3, 4, 5, 6, 7]);
        assert!(ids(&d, "relation").is_empty());
        let crossing = d.elements().iter().find(|e| e.id == 5).unwrap();
        assert!(crossing.tags.is_some());

        // A smaller area inside: from the first cut, or from the source, the same answer.
        let small = bbox(0.505, 0.505, 0.515, 0.515);
        let twice = cut(d.elements(), small);
        let once = cut(&elements, small);
        let key = |d: &OsmData| -> Vec<(String, u64, bool)> {
            d.elements()
                .iter()
                .map(|e| (e.r#type.clone(), e.id, e.tags.is_some()))
                .collect()
        };
        assert_eq!(key(&twice), key(&once));
        // The crossing node lies outside the small area: still a vertex, without its tags.
        assert!(twice
            .elements()
            .iter()
            .any(|e| e.id == 5 && e.tags.is_none()));
        assert!(twice
            .elements()
            .iter()
            .any(|e| e.id == 7 && e.tags.is_some()));
    }

    #[test]
    fn a_pbf_cut_reads_ways_and_skips_unasked_relations() {
        // osmpbf's own fixture: three nodes, one building way, one relation tagged rel_key.
        let pbf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/osmpbf-test.osm.pbf");
        let d = cut_pbf(&pbf, bbox(52.11, 11.62, 52.13, 11.64)).unwrap();
        assert_eq!(ids(&d, "node"), vec![105, 106, 108]);
        assert_eq!(ids(&d, "way"), vec![107]);
        assert!(ids(&d, "relation").is_empty());
        let w = &d.elements()[3];
        assert_eq!(w.nodes.as_deref(), Some(&[105, 106, 108, 105][..]));
        assert_eq!(w.tags.as_ref().unwrap()["building"], "yes");
        // Elsewhere, nothing.
        assert!(cut_pbf(&pbf, bbox(10.0, 10.0, 10.1, 10.1))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn geofabrik_picks_the_smallest_extract_holding_the_selection() {
        let square = |x0: f64, y0: f64, s: f64| {
            serde_json::json!({"type": "Polygon", "coordinates": [[[x0, y0], [x0 + s, y0],
                [x0 + s, y0 + s], [x0, y0 + s], [x0, y0]]]})
        };
        let feature = |url: &str, geom| serde_json::json!({"properties": {"urls": {"pbf": url}}, "geometry": geom});
        let index = serde_json::json!({"features": [
            feature("https://x/europe-latest.osm.pbf", square(0.0, 0.0, 50.0)),
            feature("https://x/romania-latest.osm.pbf", square(20.0, 40.0, 10.0)),
            feature("https://x/half-latest.osm.pbf", square(26.05, 44.4, 0.1)),
        ]});
        let want = bbox(44.445, 26.095, 44.448, 26.103);
        assert_eq!(
            geofabrik_url(&index, &want).unwrap(),
            "https://x/half-latest.osm.pbf"
        );
        // One that only holds part of it is passed over.
        let wide = bbox(44.0, 26.05, 44.2, 26.2);
        assert_eq!(
            geofabrik_url(&index, &wide).unwrap(),
            "https://x/romania-latest.osm.pbf"
        );
        assert!(geofabrik_url(&index, &bbox(70.0, 70.0, 70.1, 70.1)).is_err());
    }

    #[test]
    fn bakes_are_found_by_containment() {
        let a = bbox(1.0, 1.0, 2.0, 2.0);
        assert_eq!(E7Box::from_file_name(&a.file_name()), Some(a));
        assert!(a.contains(&bbox(1.2, 1.2, 1.8, 1.8)));
        assert!(!a.contains(&bbox(1.2, 1.2, 2.1, 1.8)));
        assert_eq!(E7Box::from_file_name("x.json.zst"), None);
    }
}
