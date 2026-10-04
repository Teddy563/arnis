//! Live option previews: a sample area built with one settings group's
//! current flags (stock defaults for the rest), drawn top-down whole for the
//! group's picture in the window. The same areas and base flags as the
//! shipped pictures (work/previews/final_render.py,
//! docs/advanced_features.md), so a live picture and a shipped one show the
//! same place. Results are cached on disk per group, flags and Arnis version.

use image::{imageops, Rgb, RgbImage};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The largest picture; a bigger map is scaled down to fit, aspect kept.
const W: u32 = 1280;
const H: u32 = 1280;

/// One group's sample: its area, flags every render of it carries, and the
/// flags the group's controls send.
struct Sample {
    bbox: &'static str,
    base: &'static [&'static str],
    flags: &'static [&'static str],
    /// Water shaded by depth: a river bed cannot be seen from above.
    depth: bool,
}

fn sample(group: &str) -> Option<Sample> {
    let s = |bbox, flags| Sample {
        bbox,
        base: &[],
        flags,
        depth: false,
    };
    Some(match group {
        // Baragan plain: open cropland.
        "fields" => s(
            "44.5500,25.9995,44.5550,26.0085",
            &["field-mix", "farm-crops", "field-scale"],
        ),
        // Englischer Garten, Munich: meadow with single trees.
        "trees" => s(
            "48.15593,11.59211,48.15693,11.59361",
            &[
                "tree-realm",
                "tree-size-weights",
                "tree-pack-dir",
                "tree-pack-mode",
            ],
        ),
        // Eiger, Moench and Kleine Scheidegg, across the 3000 m line.
        "snow" => Sample {
            base: &["--scale=0.45"],
            ..s(
                "46.545,7.955,46.595,8.025",
                &["snow-mode", "snow-percent", "snow-y"],
            )
        },
        // Grindelwald: open meadow.
        "scatter" => Sample {
            base: &["--seed=1"],
            ..s(
                "46.61489,8.03016,46.61640,8.03235",
                &["rocks", "rock-density", "bushes", "bush-density"],
            )
        },
        // Piata Romana, Bucharest: boulevard crossroads.
        "roads" => s("44.4448,26.0948,44.4474,26.0984", &["road-detail"]),
        // The Isar at the Flaucher, Munich: gravel and wooded banks.
        "water" => Sample {
            depth: true,
            ..s(
                "48.1050,11.5550,48.1140,11.5645",
                &["river-bed", "water-detail"],
            )
        },
        // Grindelwald meadow.
        "grass" => s(
            "46.62002,8.04012,46.62398,8.04588",
            &["grass-texture", "grass-mix"],
        ),
        // Rural Wallachia.
        "land" => s(
            "44.59981,25.70021,44.60519,25.70779",
            &["land-texture", "land-mix"],
        ),
        _ => return None,
    })
}

impl Sample {
    /// `flags` if every one belongs to this group, sorted, so one setting
    /// combination is one cache entry.
    fn check(&self, flags: &[String]) -> Result<Vec<String>, String> {
        let mut out = Vec::with_capacity(flags.len());
        for f in flags {
            let name = f
                .strip_prefix("--")
                .map(|n| n.split('=').next().unwrap_or_default())
                .unwrap_or_default();
            if !self.flags.contains(&name) {
                return Err(format!("{f} is not part of this preview"));
            }
            out.push(f.clone());
        }
        out.sort();
        Ok(out)
    }
}

/// Where a render of `group` with `flags` is kept.
// ponytail: never pruned (a card is ~40 KB, one per combination tried); give
// it an age cap like the tile caches if it ever grows past that.
fn cache_path(root: &Path, group: &str, flags: &[String]) -> PathBuf {
    let key = format!(
        "{}\0{group}\0{}",
        env!("CARGO_PKG_VERSION"),
        flags.join("\0")
    );
    let digest = Sha256::digest(key);
    let name: String = digest[..12].iter().map(|b| format!("{b:02x}")).collect();
    root.join("arnis")
        .join("option-previews")
        .join(format!("{name}.png"))
}

/// One render at a time: they are short, and side by side they would only
/// slow each other down.
static RENDER: Mutex<()> = Mutex::new(());

/// What a render failed on. `NeedsData`: offline, and the caches lack the
/// sample area.
#[derive(Debug, PartialEq)]
pub enum Failure {
    NeedsData,
    Other(String),
}

/// The card for `group` with `flags`, as PNG bytes: from the cache under
/// `root`, or built by `exe` (this executable, as a CLI run) in a temporary
/// folder. `offline` builds from the caches only.
pub fn render(
    root: &Path,
    exe: &Path,
    group: &str,
    flags: &[String],
    offline: bool,
) -> Result<Vec<u8>, Failure> {
    let sample = sample(group).ok_or_else(|| Failure::Other(format!("no preview {group}")))?;
    let flags = sample.check(flags).map_err(Failure::Other)?;
    // A tree folder's contents are part of the combination, not only its name.
    let mut key = flags.clone();
    if let Some(dir) = flags
        .iter()
        .find_map(|f| f.strip_prefix("--tree-pack-dir="))
    {
        key.push(crate::trees::pack_dir::stamp(Path::new(dir)));
    }
    let cached = cache_path(root, group, &key);
    if let Ok(png) = std::fs::read(&cached) {
        return Ok(png);
    }
    let _one = RENDER.lock().unwrap_or_else(|e| e.into_inner());
    // A render of the same combination may have finished while this one waited.
    if let Ok(png) = std::fs::read(&cached) {
        return Ok(png);
    }
    // Under the lock, so one folder name per process is enough.
    let tmp = std::env::temp_dir().join(format!("arnis-option-preview-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| Failure::Other(e.to_string()))?;
    let map = build(&sample, exe, &tmp, &flags, offline);
    let _ = std::fs::remove_dir_all(&tmp);
    let png = encode(&fit(map?)).map_err(Failure::Other)?;
    if let Some(dir) = cached.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    crate::overture::write_atomic(&cached, &png);
    Ok(png)
}

/// Runs the sample in `tmp` and returns its top-down map.
fn build(
    sample: &Sample,
    exe: &Path,
    tmp: &Path,
    flags: &[String],
    offline: bool,
) -> Result<RgbImage, Failure> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg(format!("--bbox={}", sample.bbox))
        .arg("--output-dir")
        .arg(tmp)
        .args(["--map-preview", "--no-3d"])
        .args(sample.base)
        .args(flags)
        .args(offline.then_some("--offline"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::scale::child::prepare(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| Failure::Other(e.to_string()))?;
    let _ = crate::scale::child::adopt(&child);
    let status = child.wait().map_err(|e| Failure::Other(e.to_string()))?;
    if !status.success() {
        return Err(if offline {
            Failure::NeedsData
        } else {
            Failure::Other(format!("the sample run failed ({status})"))
        });
    }
    let world = std::fs::read_dir(tmp)
        .ok()
        .and_then(|mut d| d.find_map(|e| e.ok().map(|e| e.path())))
        .ok_or_else(|| Failure::Other("the sample run wrote no world".into()))?;
    let mut map = image::open(world.join("arnis_world_map.png"))
        .map_err(|e| Failure::Other(e.to_string()))?
        .to_rgb8();
    if sample.depth {
        shade_water(&world, &mut map);
    }
    Ok(map)
}

/// The whole map, scaled down to fit W x H when it is bigger.
fn fit(map: RgbImage) -> RgbImage {
    let k = f64::from(W) / f64::from(map.width()).max(1.0);
    let k = k.min(f64::from(H) / f64::from(map.height()).max(1.0));
    if k >= 1.0 {
        return map;
    }
    let w = ((f64::from(map.width()) * k).round() as u32).max(1);
    let h = ((f64::from(map.height()) * k).round() as u32).max(1);
    imageops::resize(&map, w, h, imageops::FilterType::Triangle)
}

fn encode(img: &RgbImage) -> Result<Vec<u8>, String> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// Water pixels coloured by the depth of their water column, light to dark
/// over one to five blocks. The map starts at block (0, 0).
fn shade_water(world: &Path, map: &mut RgbImage) {
    use fastanvil::{Chunk, JavaChunk, Region};
    let Ok(files) = std::fs::read_dir(world.join("region")) else {
        return;
    };
    for path in files.filter_map(|e| e.ok().map(|e| e.path())) {
        let Ok(file) = std::fs::File::open(&path) else {
            continue;
        };
        let Ok(mut region) = Region::from_stream(file) else {
            continue;
        };
        for data in region.iter().filter_map(|c| c.ok()) {
            let Ok(chunk) = JavaChunk::from_bytes(&data.data) else {
                continue;
            };
            let (cx, cz) = chunk_pos(&path, data.x, data.z);
            let ys = chunk.y_range();
            for x in 0..16 {
                for z in 0..16 {
                    let (px, pz) = (cx * 16 + x as i64, cz * 16 + z as i64);
                    if px < 0 || pz < 0 || px >= map.width() as i64 || pz >= map.height() as i64 {
                        continue;
                    }
                    let mut depth = 0u32;
                    for y in ys.clone().rev() {
                        match chunk.block(x, y, z).map(|b| b.name()) {
                            Some("minecraft:water") => depth += 1,
                            Some("minecraft:air") | None if depth == 0 => {}
                            _ => break,
                        }
                    }
                    if depth > 0 {
                        let t = (f64::from(depth - 1) / 4.0).min(1.0);
                        let c = |a: f64, b: f64| (a - b * t) as u8;
                        map.put_pixel(
                            px as u32,
                            pz as u32,
                            Rgb([c(120.0, 100.0), c(190.0, 150.0), c(235.0, 120.0)]),
                        );
                    }
                }
            }
        }
    }
}

/// A chunk's world position from its region file name and slot.
fn chunk_pos(path: &Path, x: usize, z: usize) -> (i64, i64) {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let mut parts = name.split('.').skip(1);
    let rx: i64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let rz: i64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    (rx * 32 + x as i64, rz * 32 + z as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_outside_the_group_are_refused_and_order_does_not_matter() {
        let trees = sample("trees").unwrap();
        let a = trees
            .check(&[
                "--tree-size-weights=small=0".into(),
                "--tree-realm=eur".into(),
            ])
            .unwrap();
        let b = trees
            .check(&[
                "--tree-realm=eur".into(),
                "--tree-size-weights=small=0".into(),
            ])
            .unwrap();
        assert_eq!(a, b);
        let root = Path::new("root");
        assert_eq!(cache_path(root, "trees", &a), cache_path(root, "trees", &b));
        assert_ne!(
            cache_path(root, "trees", &a),
            cache_path(root, "fields", &a)
        );
        assert!(trees.check(&["--output-dir=x".into()]).is_err());
        assert!(trees.check(&["tree-realm=eur".into()]).is_err());
        assert!(sample("nope").is_none());
    }

    #[test]
    fn the_picture_is_the_whole_map_fitted() {
        let small = fit(RgbImage::new(700, 300));
        assert_eq!(small.dimensions(), (700, 300));
        let wide = fit(RgbImage::new(2560, 640));
        assert_eq!(wide.dimensions(), (W, 320));
        let tall = fit(RgbImage::new(1000, 2560));
        assert_eq!(tall.dimensions(), (500, H));
        // Every group's area parses as four numbers.
        for g in [
            "fields", "trees", "snow", "scatter", "roads", "water", "grass", "land",
        ] {
            let b: Vec<f64> = sample(g)
                .unwrap()
                .bbox
                .split(',')
                .map(|v| v.parse().unwrap())
                .collect();
            assert!(b.len() == 4 && b[0] < b[2] && b[1] < b[3], "{g}");
        }
    }

    #[test]
    fn a_cached_render_is_returned_without_a_run() {
        let root = tempfile::tempdir().unwrap();
        let flags = vec!["--road-detail=clean".to_string()];
        let path = cache_path(root.path(), "roads", &flags);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"png").unwrap();
        // No executable at all: a run would fail.
        let got = render(
            root.path(),
            Path::new("missing.exe"),
            "roads",
            &flags,
            false,
        );
        assert_eq!(got, Ok(b"png".to_vec()));
    }
}
