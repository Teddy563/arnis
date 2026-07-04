//! Dump an ASCII vertical cross-section of a real generated world's blocks, to ground-truth cave
//! geometry (stripes? breaches? fluids?). Usage:
//!   cargo run --release --example cavedump -- <region.mca> <worldZ> <x0> <x1> <y0> <y1>
//! All queried (x,z) must lie inside the ONE region file passed.
#![allow(
    dead_code,
    unused_mut,
    clippy::unnecessary_sort_by,
    clippy::manual_range_contains,
    clippy::type_complexity,
    clippy::identity_op,
    clippy::needless_range_loop,
    clippy::too_many_arguments
)]
// ^ diagnostic tooling: kept readable over lint-perfect; dead_code covers the shared
//   src modules compiled into this standalone example target.

use fastanvil::{Chunk, Region};
use std::collections::HashMap;
use std::fs::File;

fn rgb_for(name: &str) -> [u8; 3] {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    match n {
        "air" | "cave_air" | "void_air" => [210, 225, 238], // open = light (so caves are visible)
        "water" => [40, 95, 205],
        "lava" => [232, 112, 20],
        s if s.contains("deepslate") || s == "cobbled_deepslate" => [62, 62, 68],
        s if s.ends_with("_ore") => [202, 162, 44],
        "dirt" | "gravel" | "coarse_dirt" | "rooted_dirt" | "clay" | "sand" => [132, 100, 70],
        "grass_block" | "moss_block" | "moss_carpet" => [82, 150, 62],
        s if s.contains("amethyst") || s.contains("dripstone") => [150, 92, 182],
        "obsidian" | "magma_block" | "blackstone" => [28, 20, 40],
        "stone" | "andesite" | "granite" | "diorite" | "tuff" | "calcite" | "cobblestone" => {
            [122, 122, 122]
        }
        _ => [180, 70, 180], // other placed/decoration
    }
}

fn ch_for(name: &str) -> char {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    match n {
        "air" | "cave_air" | "void_air" => ' ',
        "water" => '~',
        "lava" => 'L',
        "dirt" | "gravel" | "coarse_dirt" | "rooted_dirt" | "clay" | "sand" => ':',
        "grass_block" | "moss_block" => 'm',
        s if s.ends_with("_ore") => 'o',
        s if s.contains("deepslate") => '%',
        "stone" | "andesite" | "granite" | "diorite" | "tuff" | "calcite" | "cobblestone" => '#',
        s if s.contains("amethyst") || s.contains("dripstone") => 'A',
        "obsidian" | "magma_block" | "blackstone" => '&',
        _ => '+', // any other placed/decoration block
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path: &str = &a[1];
    let file = File::open(path).expect("open region");
    let mut region = Region::from_stream(file).expect("parse region");
    let mut cache: HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>> = HashMap::new();

    // RAY mode: cavedump <region> ray <camx> <camy> <camz> <yawDeg> <pitchDeg> <out.png>
    // First-person raycast straight from the blocks — renders the player's EXACT view (multi-region).
    if a.get(2).map(|s| s == "ray").unwrap_or(false) {
        let (camx, camy, camz, yaw_d, pitch_d): (f64, f64, f64, f64, f64) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
            a[7].parse().unwrap(),
        );
        let out = &a[8];
        // region dir + base coords so we can load ANY region the rays cross
        let rpath = std::path::Path::new(path);
        let rdir = rpath.parent().unwrap().to_path_buf();
        let mut regions: HashMap<(i32, i32), Option<Region<std::fs::File>>> = HashMap::new();
        let mut chunkc: HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>> = HashMap::new();
        let mut block_solid = |x: i32,
                               y: i32,
                               z: i32,
                               regions: &mut HashMap<(i32, i32), Option<Region<std::fs::File>>>,
                               chunkc: &mut HashMap<
            (i32, i32),
            Option<fastanvil::CurrentJavaChunk>,
        >|
         -> Option<[u8; 3]> {
            if y < -64 || y > 320 {
                return None;
            }
            let (cx, cz) = (x.div_euclid(16), z.div_euclid(16));
            let ck = chunkc.entry((cx, cz)).or_insert_with(|| {
                let (rx, rz) = (cx.div_euclid(32), cz.div_euclid(32));
                let reg = regions.entry((rx, rz)).or_insert_with(|| {
                    let p = rdir.join(format!("r.{rx}.{rz}.mca"));
                    std::fs::File::open(&p)
                        .ok()
                        .and_then(|f| Region::from_stream(f).ok())
                });
                match reg {
                    Some(r) => match r
                        .read_chunk(cx.rem_euclid(32) as usize, cz.rem_euclid(32) as usize)
                    {
                        Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                        _ => None,
                    },
                    None => None,
                }
            });
            let c = ck.as_ref()?;
            let nm = c
                .block(
                    x.rem_euclid(16) as usize,
                    y as isize,
                    z.rem_euclid(16) as usize,
                )
                .map(|b| b.name())
                .unwrap_or("air");
            let n = nm.strip_prefix("minecraft:").unwrap_or(nm);
            if matches!(n, "air" | "cave_air" | "void_air") {
                None
            } else {
                Some(rgb_for(n))
            }
        };
        const W: u32 = 640;
        const H: u32 = 480;
        const MAXD: i32 = 320;
        let (yaw, pitch) = (yaw_d.to_radians(), pitch_d.to_radians());
        // MC facing vector
        let fwd = [
            -(yaw.sin()) * pitch.cos(),
            -(pitch.sin()),
            yaw.cos() * pitch.cos(),
        ];
        // right = normalize(fwd x worldUp), up = right x fwd
        let cross = |a: [f64; 3], b: [f64; 3]| {
            [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ]
        };
        let norm = |v: [f64; 3]| {
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / l, v[1] / l, v[2] / l]
        };
        let right = norm(cross(fwd, [0.0, 1.0, 0.0]));
        let up = cross(right, fwd);
        let vfov = 70f64.to_radians();
        let th = (vfov * 0.5).tan();
        let aspect = W as f64 / H as f64;
        let mut img = image::RgbImage::new(W, H);
        for py in 0..H {
            for px in 0..W {
                let sx = (px as f64 / W as f64 - 0.5) * 2.0 * th * aspect;
                let sy = (0.5 - py as f64 / H as f64) * 2.0 * th;
                let dir = norm([
                    fwd[0] + sx * right[0] + sy * up[0],
                    fwd[1] + sx * right[1] + sy * up[1],
                    fwd[2] + sx * right[2] + sy * up[2],
                ]);
                // DDA voxel march
                let (mut ix, mut iy, mut iz) = (
                    camx.floor() as i32,
                    camy.floor() as i32,
                    camz.floor() as i32,
                );
                let step = [
                    if dir[0] > 0.0 { 1 } else { -1 },
                    if dir[1] > 0.0 { 1 } else { -1 },
                    if dir[2] > 0.0 { 1 } else { -1 },
                ];
                let inv = [
                    1.0 / dir[0].abs().max(1e-9),
                    1.0 / dir[1].abs().max(1e-9),
                    1.0 / dir[2].abs().max(1e-9),
                ];
                let frac = |c: f64, s: i32| {
                    if s > 0 {
                        (c.floor() + 1.0) - c
                    } else {
                        c - c.floor()
                    }
                };
                let mut tmax = [
                    frac(camx, step[0]) * inv[0],
                    frac(camy, step[1]) * inv[1],
                    frac(camz, step[2]) * inv[2],
                ];
                let mut color = [180u8, 205, 235]; // sky
                let mut hitaxis = 0;
                for _ in 0..MAXD {
                    let axis = if tmax[0] < tmax[1] && tmax[0] < tmax[2] {
                        0
                    } else if tmax[1] < tmax[2] {
                        1
                    } else {
                        2
                    };
                    match axis {
                        0 => {
                            ix += step[0];
                            tmax[0] += inv[0];
                        }
                        1 => {
                            iy += step[1];
                            tmax[1] += inv[1];
                        }
                        _ => {
                            iz += step[2];
                            tmax[2] += inv[2];
                        }
                    }
                    if let Some(c) = block_solid(ix, iy, iz, &mut regions, &mut chunkc) {
                        color = c;
                        hitaxis = axis;
                        break;
                    }
                }
                // simple face shading
                let shade = match hitaxis {
                    1 => 1.0,
                    0 => 0.78,
                    _ => 0.62,
                };
                let c = [
                    (color[0] as f64 * shade) as u8,
                    (color[1] as f64 * shade) as u8,
                    (color[2] as f64 * shade) as u8,
                ];
                img.put_pixel(px, py, image::Rgb(c));
            }
        }
        img.save(out).expect("save png");
        eprintln!("WROTE {out} (raycast from {camx},{camy},{camz} yaw={yaw_d} pitch={pitch_d})");
        return;
    }

    // NAME mode: cavedump <region> name <x> <y> <z>  — print raw block name + a 5x5x5 sample around it.
    if a.get(2).map(|s| s == "name").unwrap_or(false) {
        let (nx, ny, nz): (i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
        );
        let mut nm = |x: i32,
                      y: i32,
                      z: i32,
                      cache: &mut HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>>|
         -> String {
            let (cx, czz) = (x.div_euclid(16), z.div_euclid(16));
            let e = cache.entry((cx, czz)).or_insert_with(|| {
                match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize) {
                    Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                    _ => None,
                }
            });
            match e {
                Some(c) => c
                    .block(
                        x.rem_euclid(16) as usize,
                        y as isize,
                        z.rem_euclid(16) as usize,
                    )
                    .map(|b| b.name().to_string())
                    .unwrap_or("OUT_OF_RANGE".into()),
                None => "NO_CHUNK".into(),
            }
        };
        eprintln!(
            "EXACT block at ({nx},{ny},{nz}) = {}",
            nm(nx, ny, nz, &mut cache)
        );
        eprintln!(
            "chunk: cx={} cz={} -> region-local ({},{})",
            nx.div_euclid(16),
            nz.div_euclid(16),
            nx.div_euclid(16).rem_euclid(32),
            nz.div_euclid(16).rem_euclid(32)
        );
        eprintln!(
            "vertical column at ({nx},*,{nz}) y={}..{}:",
            ny + 8,
            ny - 12
        );
        for y in (ny - 12..=ny + 8).rev() {
            eprintln!("  y={y:>4} {}", nm(nx, y, nz, &mut cache));
        }
        return;
    }

    // FLOATDECOR mode: cavedump <region> floatdecor <x0> <x1> <z0> <z1> <y0> <y1>
    // Find SOLID non-rock/non-fluid blocks (decoration: moss/clay/dirt/dripstone/sculk/amethyst/...) that
    // have AIR directly below them = floating placed blocks. Histogram by name + sample coords.
    if a.get(2).map(|s| s == "floatdecor").unwrap_or(false) {
        let (fx0, fx1, fz0, fz1, fy0, fy1): (i32, i32, i32, i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
            a[7].parse().unwrap(),
            a[8].parse().unwrap(),
        );
        let mut nm = |x: i32,
                      y: i32,
                      z: i32,
                      cache: &mut HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>>|
         -> String {
            let (cx, czz) = (x.div_euclid(16), z.div_euclid(16));
            let e = cache.entry((cx, czz)).or_insert_with(|| {
                match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize) {
                    Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                    _ => None,
                }
            });
            match e {
                Some(c) => {
                    let n = c
                        .block(
                            x.rem_euclid(16) as usize,
                            y as isize,
                            z.rem_euclid(16) as usize,
                        )
                        .map(|b| b.name())
                        .unwrap_or("air");
                    n.strip_prefix("minecraft:").unwrap_or(n).to_string()
                }
                None => "air".to_string(),
            }
        };
        let rocky = |s: &str| {
            matches!(
                s,
                "air"
                    | "cave_air"
                    | "void_air"
                    | "water"
                    | "lava"
                    | "stone"
                    | "deepslate"
                    | "cobbled_deepslate"
                    | "tuff"
                    | "andesite"
                    | "granite"
                    | "diorite"
                    | "calcite"
                    | "bedrock"
                    | "gravel"
                    | "dirt"
                    | "coarse_dirt"
            )
        };
        let is_air = |s: &str| matches!(s, "air" | "cave_air" | "void_air");
        let mut counts: HashMap<String, (u64, i32, i32, i32)> = HashMap::new(); // name -> (count, sample x,y,z)
        for x in fx0..=fx1 {
            for z in fz0..=fz1 {
                for y in fy0..=fy1 {
                    let s = nm(x, y, z, &mut cache);
                    if rocky(&s) {
                        continue;
                    } // only DECORATION/placed blocks
                      // TRUE isolated floater: air directly below AND directly above (not floor- nor ceiling-attached).
                    if is_air(&nm(x, y - 1, z, &mut cache)) && is_air(&nm(x, y + 1, z, &mut cache))
                    {
                        let e = counts.entry(s).or_insert((0, x, y, z));
                        e.0 += 1;
                    }
                }
            }
        }
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        eprintln!("FLOATING DECORATION (placed block with AIR directly below) in box:");
        for (name, (c, sx, sy, sz)) in v.into_iter().take(25) {
            eprintln!("  {:>6}  {:24} e.g. ({sx},{sy},{sz})", c, name);
        }
        return;
    }

    // WATERMAP mode: cavedump <region> watermap <x0> <x1> <z0> <z1> <y0> <y1> <png>
    // Top-down X-Z map: blue where a column has WATER (brightness = water depth), brown where CLAY,
    // grey rock. Shows pool/river SHAPE from above (blob vs winding line) + where clay barriers sit.
    if a.get(2).map(|s| s == "watermap").unwrap_or(false) {
        let (mx0, mx1, mz0, mz1, my0, my1): (i32, i32, i32, i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
            a[7].parse().unwrap(),
            a[8].parse().unwrap(),
        );
        let png = &a[9];
        let mut nm = |x: i32,
                      y: i32,
                      z: i32,
                      cache: &mut HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>>|
         -> String {
            let (cx, czz) = (x.div_euclid(16), z.div_euclid(16));
            let e = cache.entry((cx, czz)).or_insert_with(|| {
                match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize) {
                    Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                    _ => None,
                }
            });
            match e {
                Some(c) => {
                    let n = c
                        .block(
                            x.rem_euclid(16) as usize,
                            y as isize,
                            z.rem_euclid(16) as usize,
                        )
                        .map(|b| b.name())
                        .unwrap_or("air");
                    n.strip_prefix("minecraft:").unwrap_or(n).to_string()
                }
                None => "air".to_string(),
            }
        };
        const SCALE: u32 = 5;
        let w = (mx1 - mx0 + 1) as u32;
        let h = (mz1 - mz0 + 1) as u32;
        let mut img = image::RgbImage::new(w * SCALE, h * SCALE);
        for (ri, z) in (mz0..=mz1).enumerate() {
            for (ci, x) in (mx0..=mx1).enumerate() {
                let mut water = 0;
                let mut clay = false;
                for y in my0..=my1 {
                    let s = nm(x, y, z, &mut cache);
                    if s == "water" {
                        water += 1;
                    }
                    if s == "clay" {
                        clay = true;
                    }
                }
                let rgb = if water > 0 {
                    let b = (120 + water * 25).min(255) as u8;
                    [30, 90, b]
                } else if clay {
                    [150, 110, 70]
                } else {
                    [70, 70, 74]
                };
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        img.put_pixel(
                            ci as u32 * SCALE + dx,
                            ri as u32 * SCALE + dy,
                            image::Rgb(rgb),
                        );
                    }
                }
            }
        }
        img.save(png).expect("save png");
        eprintln!("WROTE {png} (top-down water=blue clay=brown rock=grey, x[{mx0}..{mx1}] z[{mz0}..{mz1}])");
        return;
    }

    // AIRMAP mode: cavedump <region> airmap <x0> <x1> <z0> <z1> <y0> <y1> <png>
    // Top-down X-Z heatmap: pixel = how many AIR cells in that (x,z) Y-column. Straight air sheets show as streaks.
    if a.get(2).map(|s| s == "airmap").unwrap_or(false) {
        let (mx0, mx1, mz0, mz1, my0, my1): (i32, i32, i32, i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
            a[7].parse().unwrap(),
            a[8].parse().unwrap(),
        );
        let png = &a[9];
        let mut isair = |x: i32,
                         y: i32,
                         z: i32,
                         cache: &mut HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>>|
         -> bool {
            let (cx, czz) = (x.div_euclid(16), z.div_euclid(16));
            let e = cache.entry((cx, czz)).or_insert_with(|| {
                match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize) {
                    Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                    _ => None,
                }
            });
            match e {
                Some(c) => {
                    let nm = c
                        .block(
                            x.rem_euclid(16) as usize,
                            y as isize,
                            z.rem_euclid(16) as usize,
                        )
                        .map(|b| b.name())
                        .unwrap_or("air");
                    matches!(
                        nm.strip_prefix("minecraft:").unwrap_or(nm),
                        "air" | "cave_air" | "void_air"
                    )
                }
                None => false,
            }
        };
        const SCALE: u32 = 5;
        let yspan = (my1 - my0 + 1).max(1) as f64;
        let w = (mx1 - mx0 + 1) as u32;
        let h = (mz1 - mz0 + 1) as u32;
        let mut img = image::RgbImage::new(w * SCALE, h * SCALE);
        for (ri, z) in (mz0..=mz1).enumerate() {
            for (ci, x) in (mx0..=mx1).enumerate() {
                let mut n = 0;
                for y in my0..=my1 {
                    if isair(x, y, z, &mut cache) {
                        n += 1;
                    }
                }
                let frac = (n as f64 / yspan).min(1.0);
                // dark stone -> bright cyan as air-fraction rises
                let rgb = [
                    (40.0 + frac * 40.0) as u8,
                    (40.0 + frac * 200.0) as u8,
                    (48.0 + frac * 207.0) as u8,
                ];
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        img.put_pixel(
                            ci as u32 * SCALE + dx,
                            ri as u32 * SCALE + dy,
                            image::Rgb(rgb),
                        );
                    }
                }
            }
        }
        img.save(png).expect("save png");
        eprintln!("WROTE {png} (top-down air-column map x[{mx0}..{mx1}] z[{mz0}..{mz1}] over y[{my0}..{my1}])");
        return;
    }

    // AXISPROF mode: cavedump <region> axisprof <x0> <x1> <z0> <z1> <y0> <y1>
    // air% per X (summed over y,z) and per Z — reveals the X-period / Z-period of the streaks.
    if a.get(2).map(|s| s == "axisprof").unwrap_or(false) {
        let (qx0, qx1, qz0, qz1, qy0, qy1): (i32, i32, i32, i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
            a[7].parse().unwrap(),
            a[8].parse().unwrap(),
        );
        let mut isair = |x: i32,
                         y: i32,
                         z: i32,
                         cache: &mut HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>>|
         -> bool {
            let (cx, czz) = (x.div_euclid(16), z.div_euclid(16));
            let e = cache.entry((cx, czz)).or_insert_with(|| {
                match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize) {
                    Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                    _ => None,
                }
            });
            match e {
                Some(c) => {
                    let nm = c
                        .block(
                            x.rem_euclid(16) as usize,
                            y as isize,
                            z.rem_euclid(16) as usize,
                        )
                        .map(|b| b.name())
                        .unwrap_or("air");
                    matches!(
                        nm.strip_prefix("minecraft:").unwrap_or(nm),
                        "air" | "cave_air" | "void_air"
                    )
                }
                None => false,
            }
        };
        let areax = ((qz1 - qz0 + 1) * (qy1 - qy0 + 1)) as f64;
        eprintln!("=== air% per X  (col period => stripe spacing).  x | chunk-local x(x&15) ===");
        for x in qx0..=qx1 {
            let mut n = 0;
            for z in qz0..=qz1 {
                for y in qy0..=qy1 {
                    if isair(x, y, z, &mut cache) {
                        n += 1;
                    }
                }
            }
            let pct = n as f64 / areax * 100.0;
            let bar = "#".repeat((pct * 2.0) as usize);
            eprintln!("x={x:>4} (x&15={:>2}) {:5.1}% {bar}", x.rem_euclid(16), pct);
        }
        let areaz = ((qx1 - qx0 + 1) * (qy1 - qy0 + 1)) as f64;
        eprintln!("=== air% per Z ===");
        for z in qz0..=qz1 {
            let mut n = 0;
            for x in qx0..=qx1 {
                for y in qy0..=qy1 {
                    if isair(x, y, z, &mut cache) {
                        n += 1;
                    }
                }
            }
            let pct = n as f64 / areaz * 100.0;
            let bar = "#".repeat((pct * 2.0) as usize);
            eprintln!("z={z:>4} (z&15={:>2}) {:5.1}% {bar}", z.rem_euclid(16), pct);
        }
        return;
    }

    // LINES mode: cavedump <region> lines <x0> <x1> <z0> <z1> <y0> <y1> <minlen>
    // Detect long, THIN, axis-aligned air runs (the unnatural "straight lines"). A run at fixed (x,y)
    // along Z counts only if it's thin (solid at x±1 OR y±1 along its length) — i.e. a 1-2 wide slit.
    if a.get(2).map(|s| s == "lines").unwrap_or(false) {
        let (lx0, lx1, lz0, lz1, ly0, ly1): (i32, i32, i32, i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
            a[7].parse().unwrap(),
            a[8].parse().unwrap(),
        );
        let minlen: i32 = a.get(9).and_then(|s| s.parse().ok()).unwrap_or(24);
        let mut isair = |x: i32,
                         y: i32,
                         z: i32,
                         cache: &mut HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>>|
         -> bool {
            let (cx, czz) = (x.div_euclid(16), z.div_euclid(16));
            let e = cache.entry((cx, czz)).or_insert_with(|| {
                match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize) {
                    Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                    _ => None,
                }
            });
            match e {
                Some(c) => {
                    let nm = c
                        .block(
                            x.rem_euclid(16) as usize,
                            y as isize,
                            z.rem_euclid(16) as usize,
                        )
                        .map(|b| b.name())
                        .unwrap_or("air");
                    matches!(
                        nm.strip_prefix("minecraft:").unwrap_or(nm),
                        "air" | "cave_air" | "void_air"
                    )
                }
                None => false,
            }
        };
        // Z-aligned slits: fixed (x,y), long air run in z, thin perpendicular (x-1 & x+1 solid for most of run).
        let mut zhits: Vec<(i32, i32, i32, i32, f64)> = Vec::new(); // x,y,zstart,len,thin%
        for x in lx0..=lx1 {
            for y in ly0..=ly1 {
                let mut z = lz0;
                while z <= lz1 {
                    if isair(x, y, z, &mut cache) {
                        let zs = z;
                        while z <= lz1 && isair(x, y, z, &mut cache) {
                            z += 1;
                        }
                        let len = z - zs;
                        if len >= minlen {
                            let mut thin = 0;
                            for zz in zs..zs + len {
                                if !isair(x - 1, y, zz, &mut cache)
                                    && !isair(x + 1, y, zz, &mut cache)
                                {
                                    thin += 1;
                                }
                            }
                            let tp = thin as f64 / len as f64 * 100.0;
                            if tp >= 60.0 {
                                zhits.push((x, y, zs, len, tp));
                            }
                        }
                    } else {
                        z += 1;
                    }
                }
            }
        }
        // X-aligned slits
        let mut xhits: Vec<(i32, i32, i32, i32, f64)> = Vec::new(); // z,y,xstart,len,thin%
        for z in lz0..=lz1 {
            for y in ly0..=ly1 {
                let mut x = lx0;
                while x <= lx1 {
                    if isair(x, y, z, &mut cache) {
                        let xs = x;
                        while x <= lx1 && isair(x, y, z, &mut cache) {
                            x += 1;
                        }
                        let len = x - xs;
                        if len >= minlen {
                            let mut thin = 0;
                            for xx in xs..xs + len {
                                if !isair(xx, y, z - 1, &mut cache)
                                    && !isair(xx, y, z + 1, &mut cache)
                                {
                                    thin += 1;
                                }
                            }
                            let tp = thin as f64 / len as f64 * 100.0;
                            if tp >= 60.0 {
                                xhits.push((z, y, xs, len, tp));
                            }
                        }
                    } else {
                        x += 1;
                    }
                }
            }
        }
        zhits.sort_by(|a, b| b.3.cmp(&a.3));
        xhits.sort_by(|a, b| b.3.cmp(&a.3));
        eprintln!("THIN Z-slits (fixed x,y; long air run in Z; >=60% perpendicular-thin): {} found, minlen={minlen}", zhits.len());
        for (x, y, zs, len, tp) in zhits.iter().take(15) {
            eprintln!(
                "  x={x} y={y} z=[{zs}..{}] len={len} thin={tp:.0}%",
                zs + len - 1
            );
        }
        eprintln!(
            "THIN X-slits (fixed z,y; long air run in X): {} found",
            xhits.len()
        );
        for (z, y, xs, len, tp) in xhits.iter().take(15) {
            eprintln!(
                "  z={z} y={y} x=[{xs}..{}] len={len} thin={tp:.0}%",
                xs + len - 1
            );
        }
        return;
    }

    // SURFMAP mode: cavedump <region> surfmap <x0> <x1> <z0> <z1> <png>
    // Top-down surface heightmap (topmost non-air block per column). Z-running 1-wide stripes here =
    // the elevation data is striped, and the cave ceiling-gate prints that stripe into the caves.
    if a.get(2).map(|s| s == "surfmap").unwrap_or(false) {
        let (mx0, mx1, mz0, mz1): (i32, i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
        );
        let png = &a[7];
        let mut top = |x: i32,
                       z: i32,
                       cache: &mut HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>>|
         -> i32 {
            let (cx, czz) = (x.div_euclid(16), z.div_euclid(16));
            let e = cache.entry((cx, czz)).or_insert_with(|| {
                match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize) {
                    Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                    _ => None,
                }
            });
            let c = match e {
                Some(c) => c,
                None => return i32::MIN,
            };
            for y in (-60..=200).rev() {
                let nm = c
                    .block(
                        x.rem_euclid(16) as usize,
                        y as isize,
                        z.rem_euclid(16) as usize,
                    )
                    .map(|b| b.name())
                    .unwrap_or("air");
                let nm = nm.strip_prefix("minecraft:").unwrap_or(nm);
                if !matches!(nm, "air" | "cave_air" | "void_air") {
                    return y;
                }
            }
            i32::MIN
        };
        // first pass: collect heights + min/max for color scaling
        let w = (mx1 - mx0 + 1) as usize;
        let h = (mz1 - mz0 + 1) as usize;
        let mut hs = vec![i32::MIN; w * h];
        let (mut lo, mut hi) = (i32::MAX, i32::MIN);
        for (ri, z) in (mz0..=mz1).enumerate() {
            for (ci, x) in (mx0..=mx1).enumerate() {
                let t = top(x, z, &mut cache);
                hs[ri * w + ci] = t;
                if t != i32::MIN {
                    lo = lo.min(t);
                    hi = hi.max(t);
                }
            }
        }
        const SCALE: u32 = 5;
        let span = (hi - lo).max(1) as f64;
        let mut img = image::RgbImage::new(w as u32 * SCALE, h as u32 * SCALE);
        for ri in 0..h {
            for ci in 0..w {
                let t = hs[ri * w + ci];
                let rgb = if t == i32::MIN {
                    [255, 0, 255]
                } else {
                    let f = ((t - lo) as f64 / span).clamp(0.0, 1.0);
                    [
                        (30.0 + f * 220.0) as u8,
                        (60.0 + f * 180.0) as u8,
                        (40.0 + f * 120.0) as u8,
                    ]
                };
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        img.put_pixel(
                            ci as u32 * SCALE + dx,
                            ri as u32 * SCALE + dy,
                            image::Rgb(rgb),
                        );
                    }
                }
            }
        }
        img.save(png).expect("save png");
        // detect 1-wide-in-X height ridges/valleys running in Z (col x differs from x-1 AND x+1 by >=2, for many z)
        let mut ridge_cols = 0u32;
        for ci in 1..w - 1 {
            let mut hits = 0;
            for ri in 0..h {
                let (a, b, c) = (hs[ri * w + ci - 1], hs[ri * w + ci], hs[ri * w + ci + 1]);
                if a != i32::MIN
                    && b != i32::MIN
                    && c != i32::MIN
                    && (b - a).abs() >= 2
                    && (b - c).abs() >= 2
                    && (b - a).signum() == (b - c).signum()
                {
                    hits += 1;
                }
            }
            if hits as f64 / h as f64 > 0.4 {
                ridge_cols += 1;
            }
        }
        eprintln!("WROTE {png}  surf height range [{lo}..{hi}]  1-wide-X ridge cols running in Z = {ridge_cols}/{}", w);
        return;
    }

    // FIND mode: cavedump <region> find <x0> <x1> <z0> <z1> <y0> <y1> <substr>
    // Count + Y-histogram + sample coords of any block whose name CONTAINS <substr> (e.g. grass, snow).
    if a.get(2).map(|s| s == "find").unwrap_or(false) {
        let (fx0, fx1, fz0, fz1, fy0, fy1): (i32, i32, i32, i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
            a[7].parse().unwrap(),
            a[8].parse().unwrap(),
        );
        let needle = a.get(9).map(|s| s.as_str()).unwrap_or("grass");
        // name -> (count, ymin, ymax, per-y histogram, sample x,y,z)
        let mut counts: HashMap<
            String,
            (
                u64,
                i32,
                i32,
                std::collections::BTreeMap<i32, u64>,
                i32,
                i32,
                i32,
            ),
        > = HashMap::new();
        for x in fx0..=fx1 {
            for z in fz0..=fz1 {
                let (cx, czz) = (x.div_euclid(16), z.div_euclid(16));
                let e = cache.entry((cx, czz)).or_insert_with(|| {
                    match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize)
                    {
                        Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                        _ => None,
                    }
                });
                let c = match e {
                    Some(c) => c,
                    None => continue,
                };
                for y in fy0..=fy1 {
                    let nm = c
                        .block(
                            x.rem_euclid(16) as usize,
                            y as isize,
                            z.rem_euclid(16) as usize,
                        )
                        .map(|b| b.name())
                        .unwrap_or("air");
                    let nm = nm.strip_prefix("minecraft:").unwrap_or(nm);
                    if !nm.contains(needle) {
                        continue;
                    }
                    let e = counts.entry(nm.to_string()).or_insert((
                        0,
                        i32::MAX,
                        i32::MIN,
                        std::collections::BTreeMap::new(),
                        x,
                        y,
                        z,
                    ));
                    e.0 += 1;
                    e.1 = e.1.min(y);
                    e.2 = e.2.max(y);
                    *e.3.entry(y).or_insert(0) += 1;
                    e.4 = x;
                    e.5 = y;
                    e.6 = z;
                }
            }
        }
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        eprintln!("FIND '{needle}' in box x[{fx0}..{fx1}] z[{fz0}..{fz1}] y[{fy0}..{fy1}]:");
        for (nm, (c, ymn, ymx, hist, sx, sy, sz)) in v.into_iter() {
            eprintln!(
                "  {:>8}  {:28} y[{}..{}]  e.g.({sx},{sy},{sz})",
                c, nm, ymn, ymx
            );
            // print the deepest 6 y-levels for this block
            let mut deep: Vec<(&i32, &u64)> = hist.iter().collect();
            deep.sort_by_key(|(y, _)| **y);
            let s: String = deep
                .iter()
                .take(8)
                .map(|(y, n)| format!("y{}:{} ", y, n))
                .collect();
            eprintln!("      deepest: {s}");
        }
        return;
    }

    // BLOCKS mode: cavedump <region> blocks <x0> <x1> <z0> <z1> <y0> <y1>
    // Histogram of non-air block types in the box (find what forms floating "lines": chain/iron_bars/etc).
    if a.get(2).map(|s| s == "blocks").unwrap_or(false) {
        let (bx0, bx1, bz0, bz1, by0, by1): (i32, i32, i32, i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
            a[7].parse().unwrap(),
            a[8].parse().unwrap(),
        );
        let mut counts: HashMap<String, (u64, i32, i32)> = HashMap::new(); // name -> (count, ymin, ymax)
        for x in bx0..=bx1 {
            for z in bz0..=bz1 {
                for y in by0..=by1 {
                    let (cx, czz) = (x.div_euclid(16), z.div_euclid(16));
                    let e = cache.entry((cx, czz)).or_insert_with(|| {
                        match region
                            .read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize)
                        {
                            Ok(Some(b)) => {
                                fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok()
                            }
                            _ => None,
                        }
                    });
                    let c = match e {
                        Some(c) => c,
                        None => continue,
                    };
                    let nm = c
                        .block(
                            x.rem_euclid(16) as usize,
                            y as isize,
                            z.rem_euclid(16) as usize,
                        )
                        .map(|b| b.name())
                        .unwrap_or("air");
                    let nm = nm.strip_prefix("minecraft:").unwrap_or(nm);
                    if matches!(
                        nm,
                        "air"
                            | "cave_air"
                            | "void_air"
                            | "stone"
                            | "deepslate"
                            | "dirt"
                            | "grass_block"
                            | "water"
                            | "gravel"
                            | "bedrock"
                    ) {
                        continue;
                    }
                    let e = counts
                        .entry(nm.to_string())
                        .or_insert((0, i32::MAX, i32::MIN));
                    e.0 += 1;
                    e.1 = e.1.min(y);
                    e.2 = e.2.max(y);
                }
            }
        }
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
        eprintln!("BLOCKS in box x[{bx0}..{bx1}] z[{bz0}..{bz1}] y[{by0}..{by1}] (excl. air/stone/dirt/water):");
        for (nm, (c, ymn, ymx)) in v.into_iter().take(20) {
            eprintln!("  {:>7}  {:24} y[{}..{}]", c, nm, ymn, ymx);
        }
        return;
    }

    // AUDIT mode: cavedump <region> audit <x0> <x1> <z0> <z1> <y0> <y1>
    // One pass: air/rock %, water+lava counts, and FLOATING-rock count (solid with >=5/6 air neighbours).
    if a.get(2).map(|s| s == "audit").unwrap_or(false) {
        let (ax0, ax1, az0, az1, ay0, ay1): (i32, i32, i32, i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
            a[7].parse().unwrap(),
            a[8].parse().unwrap(),
        );
        let mut name = |x: i32,
                        y: i32,
                        z: i32,
                        cache: &mut HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>>|
         -> String {
            let (cx, czz) = (x.div_euclid(16), z.div_euclid(16));
            let e = cache.entry((cx, czz)).or_insert_with(|| {
                match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize) {
                    Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                    _ => None,
                }
            });
            match e {
                Some(c) => {
                    let n = c
                        .block(
                            x.rem_euclid(16) as usize,
                            y as isize,
                            z.rem_euclid(16) as usize,
                        )
                        .map(|b| b.name())
                        .unwrap_or("air");
                    n.strip_prefix("minecraft:").unwrap_or(n).to_string()
                }
                None => "air".to_string(),
            }
        };
        let is_air = |s: &str| matches!(s, "air" | "cave_air" | "void_air");
        let (mut air, mut water, mut lava, mut floaters, mut total) =
            (0u64, 0u64, 0u64, 0u64, 0u64);
        for x in ax0..=ax1 {
            for z in az0..=az1 {
                for y in ay0..=ay1 {
                    total += 1;
                    let s = name(x, y, z, &mut cache);
                    if is_air(&s) {
                        air += 1;
                    } else if s == "water" {
                        water += 1;
                    } else if s == "lava" {
                        lava += 1;
                    } else {
                        // solid rock: floater if >=5 of 6 neighbours are air
                        let mut an = 0;
                        for (dx, dy, dz) in [
                            (1, 0, 0),
                            (-1, 0, 0),
                            (0, 1, 0),
                            (0, -1, 0),
                            (0, 0, 1),
                            (0, 0, -1),
                        ] {
                            if is_air(&name(x + dx, y + dy, z + dz, &mut cache)) {
                                an += 1;
                            }
                        }
                        if an >= 5 {
                            floaters += 1;
                        }
                    }
                }
            }
        }
        let p = |v: u64| v as f64 / total.max(1) as f64 * 100.0;
        eprintln!("AUDIT box x[{ax0}..{ax1}] z[{az0}..{az1}] y[{ay0}..{ay1}] cells={total}");
        eprintln!(
            "  air={:.1}%  water={} lava={}  FLOATING-rock cells={} ({:.3}% of solid)",
            p(air),
            water,
            lava,
            floaters,
            floaters as f64 / (total - air).max(1) as f64 * 100.0
        );
        return;
    }

    // STATS mode:  cavedump <region> stats <x0> <x1> <z0> <z1> <y0> <y1>
    // Flood-fills AIR components in the box and prints the size histogram (isolated pockets vs caves).
    if a.get(2).map(|s| s == "stats").unwrap_or(false) {
        let (sx0, sx1, sz0, sz1, sy0, sy1): (i32, i32, i32, i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
            a[7].parse().unwrap(),
            a[8].parse().unwrap(),
        );
        let mut is_air = |x: i32,
                          y: i32,
                          z: i32,
                          cache: &mut HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>>|
         -> bool {
            let (cx, czz) = (x.div_euclid(16), z.div_euclid(16));
            let e = cache.entry((cx, czz)).or_insert_with(|| {
                match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize) {
                    Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                    _ => None,
                }
            });
            match e {
                Some(c) => {
                    let nm = c
                        .block(
                            x.rem_euclid(16) as usize,
                            y as isize,
                            z.rem_euclid(16) as usize,
                        )
                        .map(|b| b.name())
                        .unwrap_or("air");
                    matches!(
                        nm.strip_prefix("minecraft:").unwrap_or(nm),
                        "air" | "cave_air" | "void_air"
                    )
                }
                None => false,
            }
        };
        use std::collections::HashSet;
        let mut seen: HashSet<(i32, i32, i32)> = HashSet::new();
        let mut sizes: Vec<usize> = Vec::new();
        let mut air_total = 0u64;
        for x in sx0..=sx1 {
            for z in sz0..=sz1 {
                for y in sy0..=sy1 {
                    if !is_air(x, y, z, &mut cache) {
                        continue;
                    }
                    air_total += 1;
                    if seen.contains(&(x, y, z)) {
                        continue;
                    }
                    let mut stack = vec![(x, y, z)];
                    seen.insert((x, y, z));
                    let mut n = 0usize;
                    while let Some((px, py, pz)) = stack.pop() {
                        n += 1;
                        if n > 200000 {
                            break;
                        }
                        for (dx, dy, dz) in [
                            (1, 0, 0),
                            (-1, 0, 0),
                            (0, 1, 0),
                            (0, -1, 0),
                            (0, 0, 1),
                            (0, 0, -1),
                        ] {
                            let (nx, ny, nz) = (px + dx, py + dy, pz + dz);
                            if nx < sx0 || nx > sx1 || nz < sz0 || nz > sz1 || ny < sy0 || ny > sy1
                            {
                                continue;
                            }
                            if seen.contains(&(nx, ny, nz)) {
                                continue;
                            }
                            if is_air(nx, ny, nz, &mut cache) {
                                seen.insert((nx, ny, nz));
                                stack.push((nx, ny, nz));
                            }
                        }
                    }
                    sizes.push(n);
                }
            }
        }
        let mut buckets = [0u64; 6];
        let mut cells_in = [0u64; 6];
        let b = |n: usize| -> usize {
            match n {
                1 => 0,
                2..=4 => 1,
                5..=12 => 2,
                13..=40 => 3,
                41..=200 => 4,
                _ => 5,
            }
        };
        for &s in &sizes {
            buckets[b(s)] += 1;
            cells_in[b(s)] += s as u64;
        }
        let labels = ["1", "2-4", "5-12", "13-40", "41-200", "200+"];
        eprintln!("AIR-COMPONENTS in box x[{sx0}..{sx1}] z[{sz0}..{sz1}] y[{sy0}..{sy1}]  air_cells={air_total} components={}", sizes.len());
        for i in 0..6 {
            eprintln!(
                "  size {:>6}: {:>6} comps, {:>8} cells ({:.1}% of air)",
                labels[i],
                buckets[i],
                cells_in[i],
                cells_in[i] as f64 / air_total.max(1) as f64 * 100.0
            );
        }
        return;
    }
    // ZYPNG mode: cavedump <region> zypng <xFIXED> <z0> <z1> <y0> <y1> <png>  (vertical slice ⟂ to X)
    if a.get(2).map(|s| s == "zypng").unwrap_or(false) {
        let (xf, z0, z1, y0, y1): (i32, i32, i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
            a[7].parse().unwrap(),
        );
        let png = &a[8];
        const SCALE: u32 = 6;
        let cx = xf.div_euclid(16);
        let lx = xf.rem_euclid(16) as usize;
        let (ytop, ybot) = (y0.max(y1), y0.min(y1));
        let w = (z1 - z0 + 1) as u32;
        let h = (ytop - ybot + 1) as u32;
        let mut img = image::RgbImage::new(w * SCALE, h * SCALE);
        for (ri, y) in (ybot..=ytop).rev().enumerate() {
            for (ci, z) in (z0..=z1).enumerate() {
                let czz = z.div_euclid(16);
                let entry = cache.entry((cx, czz)).or_insert_with(|| {
                    match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize)
                    {
                        Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                        _ => None,
                    }
                });
                let rgb = match entry {
                    Some(c) => rgb_for(
                        c.block(lx, y as isize, z.rem_euclid(16) as usize)
                            .map(|b| b.name())
                            .unwrap_or("air"),
                    ),
                    None => [255, 0, 255],
                };
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        img.put_pixel(
                            ci as u32 * SCALE + dx,
                            ri as u32 * SCALE + dy,
                            image::Rgb(rgb),
                        );
                    }
                }
            }
        }
        img.save(png).expect("save png");
        eprintln!("WROTE {png} (Z-Y slice at x={xf})");
        return;
    }

    // PROFILE mode: cavedump <region> prof <x0> <x1> <z0> <z1> <y0> <y1>
    // Prints air% per Y level (and per X, per Z) so a periodic "stripe spacing" shows as oscillation.
    if a.get(2).map(|s| s == "prof").unwrap_or(false) {
        let (px0, px1, pz0, pz1, py0, py1): (i32, i32, i32, i32, i32, i32) = (
            a[3].parse().unwrap(),
            a[4].parse().unwrap(),
            a[5].parse().unwrap(),
            a[6].parse().unwrap(),
            a[7].parse().unwrap(),
            a[8].parse().unwrap(),
        );
        let mut isair = |x: i32,
                         y: i32,
                         z: i32,
                         cache: &mut HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>>|
         -> bool {
            let (cx, czz) = (x.div_euclid(16), z.div_euclid(16));
            let e = cache.entry((cx, czz)).or_insert_with(|| {
                match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize) {
                    Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                    _ => None,
                }
            });
            match e {
                Some(c) => {
                    let nm = c
                        .block(
                            x.rem_euclid(16) as usize,
                            y as isize,
                            z.rem_euclid(16) as usize,
                        )
                        .map(|b| b.name())
                        .unwrap_or("air");
                    matches!(
                        nm.strip_prefix("minecraft:").unwrap_or(nm),
                        "air" | "cave_air" | "void_air"
                    )
                }
                None => false,
            }
        };
        let area = ((px1 - px0 + 1) * (pz1 - pz0 + 1)) as f64;
        eprintln!("PER-Y air% (look for a repeating spike spacing = the stripe period):");
        for y in (py0..=py1).rev() {
            let mut n = 0;
            for x in px0..=px1 {
                for z in pz0..=pz1 {
                    if isair(x, y, z, &mut cache) {
                        n += 1;
                    }
                }
            }
            let pct = n as f64 / area * 100.0;
            let bar = "#".repeat((pct * 1.5) as usize);
            eprintln!("y={y:>4} {:5.1}% {bar}", pct);
        }
        return;
    }

    let (zc, x0, x1, y0, y1): (i32, i32, i32, i32, i32) = (
        a[2].parse().unwrap(),
        a[3].parse().unwrap(),
        a[4].parse().unwrap(),
        a[5].parse().unwrap(),
        a[6].parse().unwrap(),
    );
    let cz = zc.div_euclid(16);
    let lz = zc.rem_euclid(16) as usize;

    // PNG mode: a 7th arg = output png path → render the vertical cross-section as an image (block=SCALE px).
    if let Some(png) = a.get(7) {
        const SCALE: u32 = 6;
        let w = (x1 - x0 + 1) as u32;
        let (ytop, ybot) = (y0.max(y1), y0.min(y1));
        let h = (ytop - ybot + 1) as u32;
        let mut img = image::RgbImage::new(w * SCALE, h * SCALE);
        for (ri, y) in (ybot..=ytop).rev().enumerate() {
            for (ci, x) in (x0..=x1).enumerate() {
                let cx = x.div_euclid(16);
                let entry = cache.entry((cx, cz)).or_insert_with(|| {
                    match region.read_chunk(cx.rem_euclid(32) as usize, cz.rem_euclid(32) as usize)
                    {
                        Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                        _ => None,
                    }
                });
                let rgb = match entry {
                    Some(c) => rgb_for(
                        c.block(x.rem_euclid(16) as usize, y as isize, lz)
                            .map(|b| b.name())
                            .unwrap_or("air"),
                    ),
                    None => [255, 0, 255],
                };
                for dy in 0..SCALE {
                    for dx in 0..SCALE {
                        img.put_pixel(
                            ci as u32 * SCALE + dx,
                            ri as u32 * SCALE + dy,
                            image::Rgb(rgb),
                        );
                    }
                }
            }
        }
        img.save(png).expect("save png");
        eprintln!(
            "WROTE {png}  ({}x{} px)  z={zc} x=[{x0}..{x1}] y=[{ybot}..{ytop}]",
            w * SCALE,
            h * SCALE
        );
        return;
    }
    eprintln!(
        "CROSS z={zc} x=[{x0}..{x1}] y=[{y1}..{y0}] file={path}\n  ' '=air ~=water L=lava #=stone %=deepslate :=dirt o=ore m=moss A=amethyst/drip &=obsidian/magma +=other"
    );
    // PLAN-VIEW mode: y0 == y1 → dump an X(rows)×Z(cols) horizontal slice at that Y (z range = [zc, zc+span]).
    if y0 == y1 {
        let y = y0;
        let span = (x1 - x0).abs(); // reuse: z spans [zc .. zc+span]
        eprintln!(
            "PLAN y={y} x(rows)=[{x0}..{x1}] z(cols)=[{zc}..{}]",
            zc + span
        );
        for x in x0..=x1 {
            let cx = x.div_euclid(16);
            let lx = x.rem_euclid(16) as usize;
            let mut row = String::new();
            for z in zc..=(zc + span) {
                let czz = z.div_euclid(16);
                let entry = cache.entry((cx, czz)).or_insert_with(|| {
                    match region.read_chunk(cx.rem_euclid(32) as usize, czz.rem_euclid(32) as usize)
                    {
                        Ok(Some(b)) => fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&b).ok(),
                        _ => None,
                    }
                });
                let c = match entry {
                    Some(c) => c,
                    None => {
                        row.push('?');
                        continue;
                    }
                };
                let name = c
                    .block(lx, y as isize, z.rem_euclid(16) as usize)
                    .map(|b| b.name())
                    .unwrap_or("air");
                row.push(ch_for(name));
            }
            eprintln!("x={x:>5} {row}");
        }
        return;
    }
    for y in (y0..=y1).rev() {
        let mut row = String::with_capacity((x1 - x0 + 1) as usize);
        for x in x0..=x1 {
            let cx = x.div_euclid(16);
            let entry = cache.entry((cx, cz)).or_insert_with(|| {
                let rlx = cx.rem_euclid(32) as usize;
                let rlz = cz.rem_euclid(32) as usize;
                match region.read_chunk(rlx, rlz) {
                    Ok(Some(bytes)) => {
                        fastnbt::from_bytes::<fastanvil::CurrentJavaChunk>(&bytes).ok()
                    }
                    _ => None,
                }
            });
            let c = match entry {
                Some(c) => c,
                None => {
                    row.push('?');
                    continue;
                }
            };
            let lx = x.rem_euclid(16) as usize;
            let name = c
                .block(lx, y as isize, lz)
                .map(|b| b.name())
                .unwrap_or("air");
            row.push(ch_for(name));
        }
        eprintln!("y={y:>4} {row}");
    }
}
