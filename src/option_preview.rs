//! Live option previews: a tiny sample area built with one settings group's
//! current flags (stock defaults for the rest), drawn top-down for the
//! group's card in the window. The same areas and crops as the shipped
//! cards (docs/advanced_features.md), so a live card and a shipped one
//! line up. Results are cached on disk per group, flags and Arnis version.

use image::{imageops, Rgb, RgbImage};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Card size.
const W: u32 = 256;
const H: u32 = 160;

/// One group's sample: the area's centre, blocks per card pixel, flags every
/// render of it carries, and the flags the group's controls send.
struct Sample {
    lat: f64,
    lon: f64,
    zoom: u32,
    base: &'static [&'static str],
    flags: &'static [&'static str],
    /// Water shaded by depth: a river bed cannot be seen from above.
    depth: bool,
}

fn sample(group: &str) -> Option<Sample> {
    let s = |lat, lon, zoom, flags| Sample {
        lat,
        lon,
        zoom,
        base: &[],
        flags,
        depth: false,
    };
    Some(match group {
        "fields" => s(
            44.5525,
            26.004,
            2,
            &["field-mix", "farm-crops", "field-scale"],
        ),
        "trees" => s(44.2025, 25.904, 1, &["tree-realm", "tree-size-weights"]),
        "snow" => s(46.535, 7.9575, 4, &["snow-mode", "snow-percent", "snow-y"]),
        // Rocks never go on tilled farmland, so the fields are pasture.
        "scatter" => Sample {
            base: &["--field-mix=pasture"],
            ..s(
                44.5525,
                26.004,
                1,
                &["rocks", "rock-density", "bushes", "bush-density"],
            )
        },
        "roads" => s(44.446, 26.0965, 1, &["road-detail"]),
        "water" => Sample {
            depth: true,
            ..s(44.43256, 26.09, 2, &["river-bed", "water-detail"])
        },
        "grass" => s(46.62077, 8.04167, 1, &["grass-texture", "grass-mix"]),
        "land" => s(44.6025, 25.704, 2, &["land-texture", "land-mix"]),
        _ => return None,
    })
}

impl Sample {
    /// The card's ground at scale 1, one block per metre.
    fn bbox(&self) -> String {
        let half_h = f64::from(H * self.zoom) / 2.0 / 111_320.0;
        let half_w = f64::from(W * self.zoom) / 2.0 / (111_320.0 * self.lat.to_radians().cos());
        format!(
            "{:.6},{:.6},{:.6},{:.6}",
            self.lat - half_h,
            self.lon - half_w,
            self.lat + half_h,
            self.lon + half_w
        )
    }

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
    let cached = cache_path(root, group, &flags);
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
    let png = encode(&card(&map?, sample.zoom)).map_err(Failure::Other)?;
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
    cmd.arg(format!("--bbox={}", sample.bbox()))
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

/// The middle of `map`, at most `zoom` blocks per pixel, cut to the card's
/// aspect and scaled to the card.
fn card(map: &RgbImage, zoom: u32) -> RgbImage {
    let (mut cw, mut ch) = (map.width().min(W * zoom), map.height().min(H * zoom));
    if cw * H > ch * W {
        cw = (ch * W / H).max(1);
    } else {
        ch = (cw * H / W).max(1);
    }
    let crop =
        imageops::crop_imm(map, (map.width() - cw) / 2, (map.height() - ch) / 2, cw, ch).to_image();
    imageops::resize(&crop, W, H, imageops::FilterType::Triangle)
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
    fn the_card_keeps_its_aspect_and_size() {
        let map = RgbImage::new(700, 300);
        let c = card(&map, 2);
        assert_eq!(c.dimensions(), (W, H));
        // The sample area is sized for the card at its zoom.
        let s = sample("fields").unwrap();
        let b: Vec<f64> = s.bbox().split(',').map(|v| v.parse().unwrap()).collect();
        assert!((b[2] - b[0]) * 111_320.0 - f64::from(H * s.zoom) < 1.0);
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
