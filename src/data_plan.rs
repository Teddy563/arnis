//! What a run of the selected area would download, read off the caches on
//! disk alone: no request is made. The window shows it before an offline run.

use crate::args::Args;
use crate::coordinate_system::geographic::LLBBox;
use std::path::{Path, PathBuf};

/// One data source of the run.
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct Item {
    /// `osm`, `elevation`, `land_cover`, `canopy` or `overture`.
    pub source: &'static str,
    pub cached: usize,
    /// Files the run reads; 0 when nothing can be counted (Overpass, an area
    /// past what the source reads tile by tile, or no index on disk yet).
    pub total: usize,
    /// Estimated download for the files not cached yet.
    pub missing_bytes: Option<u64>,
}

#[derive(serde::Serialize, Debug, Default)]
pub struct DataPlan {
    pub items: Vec<Item>,
    /// The `.pbf` extract, with Region Download or a `.pbf` file.
    pub extract: Option<crate::osm_pbf::ExtractPlan>,
    /// The local tile archive folder's archives, with Local Archive.
    pub local_archive: Option<crate::osm_tiles::LocalCoverage>,
}

/// Typical cached file per source, measured over a working cache (2026-10),
/// for the estimate while none of the area is cached yet; afterwards the
/// area's own files set it.
const TYPICAL_OSM_TILE: u64 = 240_000;
const TYPICAL_ELEVATION_TILE: u64 = 72_000;
const TYPICAL_LAND_COVER_TILE: u64 = 38_000;
const TYPICAL_CANOPY_BLOCK: u64 = 1_000_000;
const TYPICAL_OVERTURE_TILE: u64 = 142_000;

fn count(source: &'static str, files: Option<Vec<PathBuf>>, typical: u64) -> Item {
    let files = files.unwrap_or_default();
    let (mut cached, mut bytes) = (0usize, 0u64);
    for f in &files {
        if let Ok(m) = std::fs::metadata(f) {
            cached += 1;
            bytes += m.len();
        }
    }
    let per = if cached > 0 {
        bytes / cached as u64
    } else {
        typical
    };
    Item {
        source,
        cached,
        total: files.len(),
        missing_bytes: (!files.is_empty()).then(|| per * (files.len() - cached) as u64),
    }
}

/// The plan for a run of `bbox` with `args`, from the caches under `root`.
pub fn plan(root: &Path, args: &Args, bbox: LLBBox) -> DataPlan {
    let (_, _, grid_w, grid_h) = crate::elevation::compute_grid_dims(&bbox, args.scale);
    let earth = args.body.is_earth();
    let mut out = DataPlan::default();
    if !args.skip_objects() {
        if let Some(src) = crate::osm_pbf::Source::from_args(args) {
            let e = crate::osm_pbf::plan(root, &src, bbox);
            out.items.push(Item {
                source: "osm",
                cached: usize::from(e.downloaded) + usize::from(e.baked),
                total: 2,
                // A bake is cut locally; only the extract downloads, and its
                // size is known once it is on disk.
                missing_bytes: e.downloaded.then_some(0),
            });
            out.extract = Some(e);
        } else if let Some(file) = &args.file {
            let item = count("osm", Some(vec![PathBuf::from(file)]), 0);
            out.items.push(Item {
                missing_bytes: None,
                ..item
            });
        } else if args.no_tile_archive {
            out.items.push(count("osm", None, 0));
        } else if let Some(dir) = crate::overture::pmtiles::local_path(&args.osm_tiles_url) {
            // A local archive is read in place: a tile is there when an
            // archive in the folder holds its cell, and nothing downloads.
            let cov = crate::osm_tiles::local_coverage(&dir, &bbox);
            out.items.push(Item {
                source: "osm",
                cached: cov.as_ref().map_or(0, |c| c.covered),
                total: cov.as_ref().map_or(0, |c| c.tiles),
                missing_bytes: None,
            });
            out.local_archive = cov;
        } else {
            let files = crate::osm_tiles::cache_files(root, &args.osm_tiles_url, &bbox);
            out.items.push(count("osm", files, TYPICAL_OSM_TILE));
        }
    }
    if args.terrain() && earth {
        let files = if args.aws_only_elevation {
            let dir = crate::elevation::cache::provider_cache_dir(root, "aws");
            crate::elevation::providers::aws_terrain::cache_files(&dir, &bbox)
        } else {
            let dir = crate::elevation::cache::provider_cache_dir(root, "mapterhorn");
            crate::elevation::providers::mapterhorn::cache_files(&dir, &bbox, grid_w, grid_h)
        };
        out.items
            .push(count("elevation", Some(files), TYPICAL_ELEVATION_TILE));
    }
    if earth {
        let files = crate::land_cover::cache_files(root, &bbox);
        out.items
            .push(count("land_cover", Some(files), TYPICAL_LAND_COVER_TILE));
    }
    if earth && args.canopy_height {
        let files = crate::canopy::cache_files(root, &bbox, grid_h);
        out.items
            .push(count("canopy", Some(files), TYPICAL_CANOPY_BLOCK));
    }
    if args.overture && !args.skip_objects() {
        let files = crate::overture::tile_cache_files(root, &bbox);
        out.items
            .push(count("overture", files, TYPICAL_OVERTURE_TILE));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    const BBOX: &str = "44.55,26.00,44.555,26.008";

    fn args(extra: &[&str]) -> Args {
        let bbox = format!("--bbox={BBOX}");
        Args::try_parse_from(["arnis", bbox.as_str()].iter().chain(extra)).unwrap()
    }

    fn item<'a>(p: &'a DataPlan, source: &str) -> &'a Item {
        p.items.iter().find(|i| i.source == source).unwrap()
    }

    fn touch(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; bytes]).unwrap();
    }

    #[test]
    fn counts_what_a_temp_cache_root_holds() {
        let root = std::env::temp_dir().join(format!("arnis-data-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bbox = LLBBox::from_str(BBOX).unwrap();
        let a = args(&["--canopy-height=true"]);

        let empty = plan(&root, &a, bbox);
        let elev = item(&empty, "elevation");
        assert!(elev.total > 0 && elev.cached == 0);
        assert_eq!(
            elev.missing_bytes,
            Some(TYPICAL_ELEVATION_TILE * elev.total as u64)
        );
        assert!(item(&empty, "land_cover").total >= 2, "header and a tile");
        assert!(item(&empty, "canopy").total >= 2, "strip table and a block");
        // No archive index on disk yet: nothing to count, nothing claimed.
        assert_eq!(item(&empty, "osm").total, 0);
        assert_eq!(item(&empty, "overture").total, 0, "no release on record");

        // One of each source's files cached: partly cached, and the estimate
        // follows the cached file's size.
        let (_, _, gw, gh) = crate::elevation::compute_grid_dims(&bbox, a.scale);
        let dir = crate::elevation::cache::provider_cache_dir(&root, "mapterhorn");
        let elev_files = crate::elevation::providers::mapterhorn::cache_files(&dir, &bbox, gw, gh);
        touch(&elev_files[0], 1000);
        for f in crate::land_cover::cache_files(&root, &bbox) {
            touch(&f, 10);
        }
        let partly = plan(&root, &a, bbox);
        let elev = item(&partly, "elevation");
        assert_eq!(elev.cached, 1);
        assert_eq!(elev.missing_bytes, Some(1000 * (elev.total as u64 - 1)));
        let lc = item(&partly, "land_cover");
        assert_eq!((lc.cached, lc.missing_bytes), (lc.total, Some(0)));

        // Region Download: the extract named by the cached index, then on disk.
        let pbf = args(&["--osm-pbf=geofabrik"]);
        let index = serde_json::json!({"features": [{
            "properties": {"name": "Test Region",
                "urls": {"pbf": "https://example.invalid/test-latest.osm.pbf"}},
            "geometry": {"type": "Polygon",
                "coordinates": [[[25.0, 44.0], [27.0, 44.0], [27.0, 45.0], [25.0, 45.0], [25.0, 44.0]]]}
        }]});
        let osm_pbf = root.join("arnis").join("osm-pbf");
        touch(&osm_pbf.join("geofabrik-index.json"), 0);
        std::fs::write(osm_pbf.join("geofabrik-index.json"), index.to_string()).unwrap();
        let e = plan(&root, &pbf, bbox).extract.unwrap();
        assert_eq!(e.name.as_deref(), Some("Test Region"));
        assert!(!e.downloaded && !e.baked && e.bytes.is_none());
        touch(&osm_pbf.join("downloads").join("test-latest.osm.pbf"), 4321);
        let p = plan(&root, &pbf, bbox);
        assert_eq!(p.extract.as_ref().unwrap().bytes, Some(4321));
        assert_eq!(item(&p, "osm").cached, 1, "downloaded, not baked");

        // Local Archive: the folder's cells, nothing to download.
        let local = root.join("local-archive");
        let flag = format!("--osm-tiles-url={}", local.display());
        let empty = plan(&root, &args(&[flag.as_str()]), bbox);
        assert_eq!(item(&empty, "osm").cached, 0);
        assert!(item(&empty, "osm").total > 0);
        assert!(empty.local_archive.unwrap().archives.is_empty());

        // Terrain-only builds no objects, so OSM is not part of the plan.
        let terrain_only = plan(&root, &args(&["--mode=terrain-only"]), bbox);
        assert!(terrain_only.items.iter().all(|i| i.source != "osm"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
