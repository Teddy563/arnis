//! `--climate-map <PREFIX>`: render the Koppen CLIMATE layout for `--bbox` and exit, without
//! generating a world. One top-down PNG (`<PREFIX>.png`) colours every sample by the grouped
//! `Climate` the world would actually get (the same 10-way grouping that drives biome tint and
//! arid/polar surface blocks), so the preview IS the climate the world will use. A JSON line
//! with the measured share of every group goes to stdout (prefix `CLIMATEMAP `).
//!
//! Sampling is done in lat/lon space directly over `--bbox` (the climate grid is lat/lon
//! native), so this is a pure function of the bounding box: no master-origin, scale, or terrain
//! fetch is needed, and the image lines up with a geographic map overlay. Pixel dimensions are
//! chosen so the thumbnail keeps the on-ground aspect (longitude compressed by cos(lat)).

use crate::args::Args;
use crate::climate::Climate;
use image::{Rgba, RgbaImage};

const MAX_SIDE: u32 = 512;

/// (color, name) per grouped climate. Opaque, since this renders to a dedicated preview canvas
/// (a full legend) rather than a transparent-over-map overlay.
fn style(c: Climate) -> (Rgba<u8>, &'static str) {
    match c {
        Climate::Temperate => (Rgba([120, 180, 90, 255]), "temperate"),
        Climate::TropicalSavanna => (Rgba([170, 190, 80, 255]), "tropical_savanna"),
        Climate::HotDesert => (Rgba([237, 201, 120, 255]), "hot_desert"),
        Climate::HotSteppe => (Rgba([214, 178, 110, 255]), "hot_steppe"),
        Climate::ColdDesert => (Rgba([200, 190, 160, 255]), "cold_desert"),
        Climate::ColdSteppe => (Rgba([190, 185, 140, 255]), "cold_steppe"),
        Climate::DryContinental => (Rgba([200, 140, 80, 255]), "dry_continental"),
        Climate::Boreal => (Rgba([80, 150, 120, 255]), "boreal"),
        Climate::Tundra => (Rgba([170, 200, 205, 255]), "tundra"),
        Climate::IceCap => (Rgba([240, 248, 255, 255]), "ice_cap"),
    }
}

pub fn render(args: &Args) -> Result<(), String> {
    let prefix = args
        .climate_map
        .as_ref()
        .expect("render() is only called when --climate-map is set");

    let min_lat = args.bbox.min().lat();
    let max_lat = args.bbox.max().lat();
    let min_lng = args.bbox.min().lng();
    let max_lng = args.bbox.max().lng();
    let lat_span = (max_lat - min_lat).max(f64::EPSILON);
    let lon_span = (max_lng - min_lng).max(f64::EPSILON);
    let mid_lat = (min_lat + max_lat) / 2.0;

    // On-ground extents (metres are proportional to these); longitude is compressed by cos(lat).
    let ground_w = lon_span * mid_lat.to_radians().cos();
    let ground_h = lat_span;
    let max_dim = ground_w.max(ground_h);
    let w = ((MAX_SIDE as f64) * ground_w / max_dim).round().max(1.0) as u32;
    let h = ((MAX_SIDE as f64) * ground_h / max_dim).round().max(1.0) as u32;

    let mut img = RgbaImage::new(w, h);
    let mut counts: std::collections::HashMap<&'static str, u64> = Default::default();
    let mut total: u64 = 0;
    for pz in 0..h {
        // north (max_lat) at the top of the image, matching a map overlay
        let lat = max_lat - (pz as f64 + 0.5) / (h as f64) * lat_span;
        for px in 0..w {
            let lon = min_lng + (px as f64 + 0.5) / (w as f64) * lon_span;
            let (color, name) = style(Climate::at(lat, lon));
            *counts.entry(name).or_insert(0) += 1;
            total += 1;
            img.put_pixel(px, pz, color);
        }
    }

    let path = format!("{}.png", prefix.display());
    img.save(&path).map_err(|e| format!("write {path}: {e}"))?;

    let mut out = serde_json::Map::new();
    let mut stats = serde_json::Map::new();
    for (name, n) in counts {
        let pct = (n as f64) * 100.0 / (total.max(1) as f64);
        stats.insert(
            name.to_string(),
            serde_json::json!((pct * 10.0).round() / 10.0),
        );
    }
    out.insert("shares".into(), serde_json::Value::Object(stats));
    out.insert("_file".into(), serde_json::json!(path));
    out.insert(
        "_bbox_latlon".into(),
        serde_json::json!([min_lat, min_lng, max_lat, max_lng]),
    );
    out.insert("_size".into(), serde_json::json!([w, h]));
    println!("CLIMATEMAP {}", serde_json::Value::Object(out));
    Ok(())
}
