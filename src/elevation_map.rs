//! `--elevation-map <PREFIX>`: render the elevation heightmap for `--bbox` to `<PREFIX>.png` and
//! exit, without generating a world. Uses the REAL provider stack generation uses
//! (`select_provider` -> Mapterhorn / regional high-res / AWS), so the preview matches the terrain
//! the world will actually get. Prints an `ELEVMAP {json}` line (min/max metres, bbox, provider)
//! for a geographic map overlay in Meld.
//!
//! The preview grid is capped to `MAX_SIDE` on its long edge (keeping the on-ground aspect, with
//! longitude compressed by cos(lat)), so the provider's zoom stays coarse and the fetch is fast
//! while still being genuine provider data.

use crate::args::Args;
use crate::elevation::selector::select_provider;
use image::{Rgba, RgbaImage};

const MAX_SIDE: u32 = 1024;

pub fn render(args: &Args) -> Result<(), String> {
    let prefix = args
        .elevation_map
        .as_ref()
        .expect("render() is only called when --elevation-map is set");
    let hillshade = args.elevation_map_mode.as_deref() != Some("grayscale");

    let min_lat = args.bbox.min().lat();
    let max_lat = args.bbox.max().lat();
    let min_lng = args.bbox.min().lng();
    let max_lng = args.bbox.max().lng();
    let lat_span = (max_lat - min_lat).max(f64::EPSILON);
    let lon_span = (max_lng - min_lng).max(f64::EPSILON);
    let mid_lat = (min_lat + max_lat) / 2.0;

    // On-ground aspect (longitude compressed by cos(lat)); cap the long edge at MAX_SIDE.
    let ground_w = lon_span * mid_lat.to_radians().cos();
    let ground_h = lat_span;
    let max_dim = ground_w.max(ground_h);
    let w =
        (((MAX_SIDE as f64) * ground_w / max_dim).round().max(2.0) as usize).min(MAX_SIDE as usize);
    let h =
        (((MAX_SIDE as f64) * ground_h / max_dim).round().max(2.0) as usize).min(MAX_SIDE as usize);

    // Fetch real provider heights (row 0 = north / max lat, col 0 = west) — north-up, west-left,
    // exactly the orientation a geographic overlay wants.
    let provider = select_provider(&args.bbox, args.aws_only_elevation);
    let name = provider.name();
    let raw = provider
        .fetch_raw(&args.bbox, w, h)
        .map_err(|e| format!("elevation fetch failed: {e}"))?;
    let heights = &raw.heights_meters;
    if heights.is_empty() || heights[0].is_empty() {
        return Err("provider returned an empty grid".to_string());
    }
    let gh = heights.len();
    let gw = heights[0].len();

    // Finite min/max for normalisation (ocean tiles come back NaN).
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for row in heights {
        for &v in row {
            if v.is_finite() {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        // All-ocean / no data: emit a fully transparent image + zeroed stats rather than error.
        lo = 0.0;
        hi = 0.0;
    }
    let span = (hi - lo).max(1.0);

    let mut img = RgbaImage::new(gw as u32, gh as u32);
    for gy in 0..gh {
        for gx in 0..gw {
            let v = heights[gy][gx];
            if !v.is_finite() {
                img.put_pixel(gx as u32, gy as u32, Rgba([0, 0, 0, 0])); // ocean -> transparent
                continue;
            }
            let base = ((v - lo) / span * 255.0).clamp(0.0, 255.0);
            let px = if hillshade {
                // central-difference slope (one-sided at edges), like np.gradient
                let sample = |x: usize, y: usize| -> f64 {
                    let s = heights[y.min(gh - 1)][x.min(gw - 1)];
                    if s.is_finite() {
                        s
                    } else {
                        v
                    }
                };
                let dzdx = sample(gx + 1, gy) - sample(gx.saturating_sub(1), gy);
                let dzdy = sample(gx, gy + 1) - sample(gx, gy.saturating_sub(1));
                let shade = (0.6 + 0.36 * (dzdx + dzdy)).clamp(0.0, 1.0);
                let val = (base * (0.5 + 0.5 * shade)).clamp(0.0, 255.0) as u8;
                Rgba([val, val, val, 170])
            } else {
                Rgba([base as u8, base as u8, base as u8, 150])
            };
            img.put_pixel(gx as u32, gy as u32, px);
        }
    }

    let path = format!("{}.png", prefix.display());
    img.save(&path).map_err(|e| format!("write {path}: {e}"))?;

    let out = serde_json::json!({
        "min_m": (lo * 10.0).round() / 10.0,
        "max_m": (hi * 10.0).round() / 10.0,
        "bbox": [min_lat, min_lng, max_lat, max_lng],
        "w": gw,
        "h": gh,
        "provider": name,
        "mode": if hillshade { "hillshade" } else { "grayscale" },
        "_file": path,
    });
    println!("ELEVMAP {out}");
    Ok(())
}
