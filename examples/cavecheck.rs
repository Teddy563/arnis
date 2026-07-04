//! Check the cave-generation invariants against a generated region. Reports:
//!   - ore/variant blocks exposed to cave air (masking violations: should be ~0 for >=3 air faces)
//!   - floating fluid (water/lava with air directly below: should be ~0)
//!   - water-touching-lava face adjacencies (barrier check: should be 0)
//!   - total carved air (cave volume), connectivity, and themed-floor coverage
//! Usage: cargo run --release --example cavecheck -- <region.mca> <y0> <y1>
use fastanvil::{Chunk, Region};
use std::collections::HashMap;
use std::fs::File;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = &a[1];
    let mut region = Region::from_stream(File::open(path).unwrap()).unwrap();
    let mut cache: HashMap<(i32, i32), Option<fastanvil::CurrentJavaChunk>> = HashMap::new();
    let stem = std::path::Path::new(path)
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap();
    let parts: Vec<&str> = stem.split('.').collect();
    let rx: i32 = parts[1].parse().unwrap();
    let rz: i32 = parts[2].parse().unwrap();
    let bx0 = rx * 512;
    let bz0 = rz * 512;

    let mut get = |x: i32, y: i32, z: i32, region: &mut Region<File>| -> String {
        let cx = x.div_euclid(16);
        let cz = z.div_euclid(16);
        let e = cache.entry((cx, cz)).or_insert_with(|| {
            match region.read_chunk(cx.rem_euclid(32) as usize, cz.rem_euclid(32) as usize) {
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
                .map(|b| {
                    b.name()
                        .strip_prefix("minecraft:")
                        .unwrap_or(b.name())
                        .to_string()
                })
                .unwrap_or("air".into()),
            None => "air".into(),
        }
    };
    // COLUMN MODE: `cavecheck <region> col <x> <z> [y0] [y1]` dumps the vertical column.
    if a.get(2).map(|s| s == "col").unwrap_or(false) {
        let cx: i32 = a[3].parse().unwrap();
        let cz: i32 = a[4].parse().unwrap();
        let cy0: i32 = a.get(5).and_then(|s| s.parse().ok()).unwrap_or(-56);
        let cy1: i32 = a.get(6).and_then(|s| s.parse().ok()).unwrap_or(56);
        for y in (cy0..=cy1).rev() {
            println!("  y{:>3}: {}", y, get(cx, y, cz, &mut region));
        }
        return;
    }
    // FINDPOOL MODE: `cavecheck <region> findpool <y0> <y1>` scans for columns with >=3 vertically
    // stacked water cells (a pool, not a 1-tall river) and prints their (x,z,ymin,ymax).
    if a.get(2).map(|s| s == "findpool").unwrap_or(false) {
        let fy0: i32 = a[3].parse().unwrap();
        let fy1: i32 = a[4].parse().unwrap();
        let (mut pools_found, mut springs_found) = (0, 0);
        for x in bx0..bx0 + 512 {
            for z in bz0..bz0 + 512 {
                let mut run = 0;
                let mut run_start = fy0;
                for y in fy0..=fy1 {
                    if get(x, y, z, &mut region) == "water" {
                        if run == 0 {
                            run_start = y;
                        }
                        run += 1;
                    } else {
                        if run >= 3 && pools_found < 8 {
                            println!(
                                "  pool@ {},{}  y{}..{} (height {})",
                                x,
                                z,
                                run_start,
                                run_start + run - 1,
                                run
                            );
                            pools_found += 1;
                        } else if run == 1 && springs_found < 15 {
                            // isolated single-block water = a spring candidate; report separately.
                            println!("  spring?@ {},{}  y{}", x, z, run_start);
                            springs_found += 1;
                        }
                        run = 0;
                    }
                }
                if pools_found >= 8 && springs_found >= 15 {
                    return;
                }
            }
        }
        return;
    }

    let y0: i32 = a[2].parse().unwrap();
    let y1: i32 = a[3].parse().unwrap();
    let is_air = |s: &str| s == "air" || s == "cave_air" || s == "void_air";
    let is_fluid = |s: &str| s == "water" || s == "lava";
    let is_variant = |s: &str| {
        s.ends_with("_ore")
            || matches!(
                s,
                "andesite" | "diorite" | "granite" | "tuff" | "gravel" | "dirt"
            )
    };
    // shell material only — amethyst_cluster is EXPECTED to have ~5/6 air faces (a crystal decoration
    // sticking out of a budding block into the hollow, attached by just 1 face), not a shell fragment.
    let is_geode = |s: &str| {
        matches!(
            s,
            "smooth_basalt" | "calcite" | "amethyst_block" | "budding_amethyst"
        )
    };

    let pack = |x: i32, y: i32, z: i32| -> i64 {
        (((x as i64 + (1 << 23)) & 0xFF_FFFF) << 36)
            | (((z as i64 + (1 << 23)) & 0xFF_FFFF) << 12)
            | ((y as i64 + 64) & 0xFFF)
    };
    let mut airset: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut spaceset: std::collections::HashSet<i64> = std::collections::HashSet::new(); // air+water+lava
    let mut water_pos: Vec<(i32, i32, i32)> = Vec::new();
    let (
        mut exposed_ore3,
        mut exposed_ore5,
        mut float_fluid,
        mut float_exposed,
        mut wl_adj,
        mut air_total,
        mut ore_total,
    ) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let (mut water_total, mut lava_total) = (0u64, 0u64);
    let (mut water_y_min, mut water_y_max) = (i32::MAX, i32::MIN);
    let (mut geode_total, mut geode_floating, mut geode_isolated) = (0u64, 0u64, 0u64);
    // shape check: per-column max contiguous vertical cave-air run — a tall run indicates an
    // "egg/oval" cavern silhouette (vanilla caverns should read wide/flat, not tall).
    let (mut max_vert_run, mut tall_columns) = (0i32, 0u64);
    // biome-theme coverage: classify the FLOOR block under each cave-air cell (air with a solid
    // floor). themed/total = the "X% of the cave system is populated" number.
    let mut floor_total: u64 = 0;
    let mut theme_counts: std::collections::HashMap<&'static str, u64> =
        std::collections::HashMap::new();
    let theme_of = |s: &str| -> Option<&'static str> {
        match s {
            "moss_block" | "grass_block" | "clay" | "rooted_dirt" => Some("lush"),
            "dripstone_block" => Some("dripstone"),
            "sculk" | "sculk_catalyst" => Some("deepdark"),
            "mycelium" => Some("mushroom"),
            "ice" | "packed_ice" | "blue_ice" | "snow_block" | "powder_snow" => Some("ice"),
            "amethyst_block" | "budding_amethyst" | "calcite" => Some("amethyst"),
            "basalt" | "smooth_basalt" | "blackstone" | "magma_block" | "obsidian" => {
                Some("volcanic")
            }
            s if s.starts_with("dead_") && s.ends_with("_coral_block") => Some("coral"),
            _ => None,
        }
    };
    // decoration blocks standing ON a floor (plants, buds, spikes, water) — the floor beneath them
    // still counts for coverage; without this, densely-decorated themes undercount their own floors.
    let is_deco = |s: &str| -> bool {
        matches!(
            s,
            "short_grass"
                | "grass"
                | "tall_grass"
                | "moss_carpet"
                | "azalea"
                | "flowering_azalea"
                | "red_mushroom"
                | "brown_mushroom"
                | "sculk_sensor"
                | "sculk_shrieker"
                | "snow"
                | "pointed_dripstone"
                | "small_amethyst_bud"
                | "medium_amethyst_bud"
                | "large_amethyst_bud"
                | "amethyst_cluster"
                | "sea_pickle"
                | "seagrass"
                | "tall_seagrass"
                | "small_dripleaf"
                | "big_dripleaf"
                | "big_dripleaf_stem"
                | "spore_blossom"
                | "cave_vines"
                | "cave_vines_plant"
                | "glow_lichen"
                | "sculk_vein"
                | "cobweb"
                | "hanging_roots"
                | "dead_bush"
        ) || s.ends_with("_coral")
            || s.ends_with("_coral_fan")
    };
    for x in bx0..bx0 + 512 {
        for z in bz0..bz0 + 512 {
            let mut col_run = 0i32;
            let mut col_max = 0i32;
            for y in y0..=y1 {
                let here = get(x, y, z, &mut region);
                if is_air(&here) {
                    air_total += 1;
                    airset.insert(pack(x, y, z));
                    spaceset.insert(pack(x, y, z));
                    col_run += 1;
                    col_max = col_max.max(col_run);
                    let b = get(x, y - 1, z, &mut region);
                    if !is_air(&b) && !is_fluid(&b) && !is_deco(&b) {
                        floor_total += 1;
                        if let Some(t) = theme_of(&b) {
                            *theme_counts.entry(t).or_insert(0) += 1;
                        }
                    }
                    continue;
                }
                col_run = 0;
                // floors buried under a decoration/fluid still count for theme coverage: probe down
                // through the deco/fluid stack (max 4) to the real floor block. Only from the TOP of
                // a stack (block above is not deco/fluid) so a 3-deep pool counts its floor once.
                let above_b = get(x, y + 1, z, &mut region);
                if (is_deco(&here) || is_fluid(&here)) && !is_deco(&above_b) && !is_fluid(&above_b)
                {
                    let mut dy = 1;
                    while dy <= 4 {
                        let b = get(x, y - dy, z, &mut region);
                        if is_deco(&b) || is_fluid(&b) {
                            dy += 1;
                            continue;
                        }
                        if !is_air(&b) {
                            floor_total += 1;
                            if let Some(t) = theme_of(&b) {
                                *theme_counts.entry(t).or_insert(0) += 1;
                            }
                        }
                        break;
                    }
                }
                if is_fluid(&here) {
                    spaceset.insert(pack(x, y, z));
                    if here == "water" {
                        water_pos.push((x, y, z));
                    }
                }
                if is_variant(&here) {
                    ore_total += 1;
                    let air_n = [
                        (1, 0, 0),
                        (-1, 0, 0),
                        (0, 1, 0),
                        (0, -1, 0),
                        (0, 0, 1),
                        (0, 0, -1),
                    ]
                    .iter()
                    .filter(|&&(dx, dy, dz)| is_air(&get(x + dx, y + dy, z + dz, &mut region)))
                    .count();
                    if air_n >= 3 {
                        exposed_ore3 += 1;
                    }
                    if air_n >= 5 {
                        exposed_ore5 += 1;
                    }
                }
                if is_geode(&here) {
                    geode_total += 1;
                    let air_n = [
                        (1, 0, 0),
                        (-1, 0, 0),
                        (0, 1, 0),
                        (0, -1, 0),
                        (0, 0, 1),
                        (0, 0, -1),
                    ]
                    .iter()
                    .filter(|&&(dx, dy, dz)| is_air(&get(x + dx, y + dy, z + dz, &mut region)))
                    .count();
                    if air_n >= 5 {
                        geode_floating += 1; // isolated 1-block shell nub — the support sweep should kill these
                        if geode_floating <= 5 {
                            eprintln!(
                                "  geode-nub@ {},{},{} = {} air_faces={}/6",
                                x, y, z, here, air_n
                            );
                        }
                    }
                    if air_n == 6 {
                        geode_isolated += 1; // TRUE zero-support floater — the sweep should catch ALL of these
                    }
                }
                if here == "water" {
                    water_total += 1;
                    water_y_min = water_y_min.min(y);
                    water_y_max = water_y_max.max(y);
                }
                if here == "lava" {
                    lava_total += 1;
                }
                if is_fluid(&here) {
                    if is_air(&get(x, y - 1, z, &mut region)) {
                        float_fluid += 1;
                        // "exposed" = open air directly above (a pool surface you can fall into / see),
                        // vs buried under rock (invisible). Tells visible-vs-cosmetic.
                        if is_air(&get(x, y + 1, z, &mut region)) {
                            float_exposed += 1;
                        }
                    }
                    if here == "water" {
                        for (dx, dy, dz) in [
                            (1, 0, 0),
                            (-1, 0, 0),
                            (0, 1, 0),
                            (0, -1, 0),
                            (0, 0, 1),
                            (0, 0, -1),
                        ] {
                            if get(x + dx, y + dy, z + dz, &mut region) == "lava" {
                                wl_adj += 1;
                            }
                        }
                    }
                }
            }
            max_vert_run = max_vert_run.max(col_max);
            if col_max >= 20 {
                tall_columns += 1;
            }
        }
    }
    // connectivity: largest connected cave-air component as % of total air (BFS over the air set).
    let unpack = |p: i64| -> (i32, i32, i32) {
        (
            ((p >> 36) & 0xFF_FFFF) as i32 - (1 << 23),
            (p & 0xFFF) as i32 - 64,
            ((p >> 12) & 0xFF_FFFF) as i32 - (1 << 23),
        )
    };
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut largest: u64 = 0;
    for &start in &airset {
        if seen.contains(&start) {
            continue;
        }
        let mut sz: u64 = 0;
        let mut stack = vec![start];
        seen.insert(start);
        while let Some(p) = stack.pop() {
            sz += 1;
            let (x, y, z) = unpack(p);
            for (dx, dy, dz) in [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 1, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
            ] {
                let np = pack(x + dx, y + dy, z + dz);
                if airset.contains(&np) && seen.insert(np) {
                    stack.push(np);
                }
            }
        }
        if sz > largest {
            largest = sz;
        }
    }
    let conn_pct = if air_total > 0 {
        largest as f64 * 100.0 / air_total as f64
    } else {
        0.0
    };
    println!("region {}  y[{}..{}]", stem, y0, y1);
    println!("  carved air cells      : {}", air_total);
    println!(
        "  largest component     : {} ({:.1}% — connectivity)",
        largest, conn_pct
    );
    println!("  ore/variant blocks    : {}", ore_total);
    println!(
        "  ore exposed >=3 air   : {}  (masking bug — want ~0)",
        exposed_ore3
    );
    println!(
        "  ore exposed >=5 air   : {}  (floating — want 0)",
        exposed_ore5
    );
    // tight water: water whose surrounding open-space (air+water+lava in a 5x5x4 box) < 22 — i.e. in a
    // thin 1-2-wide tunnel. The open-cave gate should drive this to ~0.
    let mut tight_water: u64 = 0;
    for &(x, y, z) in &water_pos {
        let mut open = 0;
        for dy in -1..=2 {
            for dx in -2..=2 {
                for dz in -2..=2 {
                    if spaceset.contains(&pack(x + dx, y + dy, z + dz)) {
                        open += 1;
                    }
                }
            }
        }
        if open < 22 {
            tight_water += 1;
        }
    }
    println!("  water / lava cells    : {} / {}", water_total, lava_total);
    if water_total > 0 {
        println!(
            "  water y-range          : {}..{}",
            water_y_min, water_y_max
        );
    }
    println!("  geode material cells  : {}", geode_total);
    println!(
        "  geode floating nubs   : {}  (>=5 air faces, weakly attached)",
        geode_floating
    );
    println!(
        "  geode TRUE isolated   : {}  (6/6 air faces, zero support — want 0)",
        geode_isolated
    );
    println!(
        "  tight-tunnel water    : {}  (open-cave gate — want ~0)",
        tight_water
    );
    println!(
        "  floating fluid (air↓) : {}  (buried+exposed)",
        float_fluid
    );
    println!(
        "  ↳ EXPOSED (air above) : {}  (the VISIBLE ones — want 0)",
        float_exposed
    );
    println!("  water-touching-lava   : {}  (want 0)", wl_adj);
    println!(
        "  max vertical cave run : {}  (single-column tallest air run — egg/oval proxy)",
        max_vert_run
    );
    println!(
        "  columns with run>=20  : {}  (tall-spike columns)",
        tall_columns
    );
    let themed: u64 = theme_counts.values().sum();
    let cov = if floor_total > 0 {
        themed as f64 * 100.0 / floor_total as f64
    } else {
        0.0
    };
    let mut tc: Vec<(&str, u64)> = theme_counts.iter().map(|(&k, &v)| (k, v)).collect();
    tc.sort_by(|a, b| b.1.cmp(&a.1));
    let detail: Vec<String> = tc
        .iter()
        .map(|(k, v)| {
            format!(
                "{} {:.1}%",
                k,
                *v as f64 * 100.0 / floor_total.max(1) as f64
            )
        })
        .collect();
    println!(
        "  themed floor coverage : {:.1}% of {} floor cells  [{}]",
        cov,
        floor_total,
        detail.join(", ")
    );
}
