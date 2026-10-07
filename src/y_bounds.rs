//! Explicit world floor/ceiling for extended-height Java worlds (`--min-y` / `--max-y`).
//!
//! Without them the tall datapack declares the whole engine range, -2032..2031. With them the
//! same pair drives everything that range does today: the datapack's dimension_type, the
//! editor's world bounds, the scaler's sink floor and ceiling, and the `--ground-level`
//! range. The helpers in `ground` read the flags, so this module only validates them and
//! rewrites the pack.

use crate::args::Args;
use std::fs;
use std::path::Path;

/// Engine limits for a datapack dimension_type, and the vanilla range neither flag may cut
/// into: code paths keyed off the vanilla floor (no sink, bedrock at -64) stay correct only
/// if the world is at least that tall.
const PACK_MIN_Y: i32 = -2032;
const PACK_MAX_Y: i32 = 2031;

/// `--min-y` / `--max-y` describe a legal dimension and a world format that has one.
pub fn check(args: &Args) -> Result<(), String> {
    if args.min_y.is_none() && args.max_y.is_none() {
        return Ok(());
    }
    if !args.disable_height_limit || args.bedrock || args.luanti {
        return Err(
            "--min-y/--max-y set the tall datapack's range: Java worlds with --disable-height-limit only."
                .to_string(),
        );
    }
    // A One World's manifest fixes the build height for every area ever added to it.
    if args.one_world || args.units.coordinates() || args.units.one_world_unit.is_some() {
        return Err(
            "--min-y/--max-y do not combine with --one-world, --unit-regions or --one-world-workers: the world fixes its own build height."
                .to_string(),
        );
    }
    if let Some(y) = args.min_y {
        let top = crate::world_editor::DEFAULT_MIN_Y;
        if y.rem_euclid(16) != 0 || !(PACK_MIN_Y..=top).contains(&y) {
            return Err(format!(
                "--min-y must be a multiple of 16 from {PACK_MIN_Y} to {top} (got {y})."
            ));
        }
    }
    if let Some(y) = args.max_y {
        let bottom = crate::world_editor::DEFAULT_MAX_Y;
        if y.rem_euclid(16) != 15 || !(bottom..=PACK_MAX_Y).contains(&y) {
            return Err(format!(
                "--max-y must end a section (16n - 1) from {bottom} to {PACK_MAX_Y} (got {y})."
            ));
        }
    }
    Ok(())
}

/// Rewrite the installed tall datapack to declare this run's range. A no-op without the
/// flags, so the bundled files stay byte-identical.
pub fn patch_datapack(world_path: &Path, args: &Args) -> Result<(), String> {
    if args.min_y.is_none() && args.max_y.is_none() {
        return Ok(());
    }
    let min = crate::ground::extended_min_y_for(args);
    let max = crate::ground::world_top_y_for(args);
    let root = world_path
        .join("datapacks")
        .join(crate::world_utils::TALL_DATAPACK_NAME);
    // The base tree and both schema overlays each carry their own dimension_type.
    for dir in ["", "overlay_attributes", "overlay_2601"] {
        let path = root
            .join(dir)
            .join("data/minecraft/dimension_type/overworld.json");
        let bytes = fs::read(&path).map_err(|e| format!("Failed to read {path:?}: {e}"))?;
        fs::write(&path, dimension_json(&bytes, min, max)?)
            .map_err(|e| format!("Failed to write {path:?}: {e}"))?;
    }
    Ok(())
}

/// Only `min_y`, `height` and `logical_height` change; every other key is schema detail of
/// its Minecraft era and is kept as bundled.
fn dimension_json(template: &[u8], min: i32, max: i32) -> Result<Vec<u8>, String> {
    let mut doc: serde_json::Value = serde_json::from_slice(template)
        .map_err(|e| format!("bundled dimension_type is not valid JSON: {e}"))?;
    let obj = doc
        .as_object_mut()
        .ok_or("bundled dimension_type is not a JSON object")?;
    let height = max - min + 1;
    obj.insert("min_y".into(), min.into());
    obj.insert("height".into(), height.into());
    obj.insert("logical_height".into(), height.into());
    serde_json::to_vec_pretty(&doc).map_err(|e| format!("Failed to serialise dimension_type: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(extra: &[&str]) -> Args {
        let mut cmd = vec!["arnis", "--output-dir", ".", "--bbox", "1,2,3,4"];
        cmd.extend_from_slice(extra);
        Args::try_parse_from(cmd).unwrap()
    }

    #[test]
    fn absent_flags_keep_the_full_pack_range() {
        let args = parse(&["--disable-height-limit"]);
        assert!(check(&args).is_ok());
        assert_eq!(crate::ground::extended_min_y_for(&args), -2032);
        assert_eq!(crate::ground::world_top_y_for(&args), 2031);
        assert_eq!(crate::ground::extended_max_y_for(&args), 2031);
    }

    #[test]
    fn flags_drive_every_range_helper() {
        let args = parse(&["--disable-height-limit", "--min-y", "-128", "--max-y=447"]);
        assert!(crate::args::validate_args(&args).is_ok());
        assert_eq!(crate::ground::extended_min_y_for(&args), -128);
        assert_eq!(crate::ground::world_top_y_for(&args), 447);
        assert_eq!(crate::ground::extended_max_y_for(&args), 447);
        assert_eq!(crate::ground::min_ground_level_for(&args), -126);
        // Vanilla range declared through the pack: no sink below -62.
        let vanilla = parse(&["--disable-height-limit", "--min-y=-64", "--max-y=319"]);
        assert!(check(&vanilla).is_ok());
        assert_eq!(crate::ground::min_ground_level_for(&vanilla), -62);
        // One side alone keeps the other at the pack's limit.
        let floor_only = parse(&["--disable-height-limit", "--min-y=-512"]);
        assert_eq!(crate::ground::world_top_y_for(&floor_only), 2031);
    }

    #[test]
    fn refuses_what_it_cannot_honour() {
        for bad in [
            &["--min-y=-128"][..],
            &["--disable-height-limit", "--bedrock", "--min-y=-128"],
            &["--disable-height-limit", "--luanti", "--max-y=447"],
            &["--disable-height-limit", "--one-world", "--min-y=-128"],
            &[
                "--disable-height-limit",
                "--one-world",
                "--unit-regions",
                "1",
                "--max-y=447",
            ],
            &["--disable-height-limit", "--min-y=-100"],
            &["--disable-height-limit", "--min-y=-2048"],
            &["--disable-height-limit", "--min-y=0"],
            &["--disable-height-limit", "--max-y=448"],
            &["--disable-height-limit", "--max-y=303"],
            &["--disable-height-limit", "--max-y=2047"],
        ] {
            assert!(check(&parse(bad)).is_err(), "{bad:?}");
        }
        for ok in [
            &["--disable-height-limit", "--min-y=-2032", "--max-y=2031"][..],
            &["--disable-height-limit", "--max-y=319"],
        ] {
            assert!(check(&parse(ok)).is_ok(), "{ok:?}");
        }
    }

    #[test]
    fn ground_level_range_follows_the_floor() {
        let args = parse(&[
            "--disable-height-limit",
            "--min-y=-128",
            "--ground-level=-200",
        ]);
        assert!(crate::args::validate_args(&args).is_err());
    }

    #[test]
    fn rewrites_every_bundled_dimension_type() {
        let dir = tempfile::tempdir().unwrap();
        let world = dir.path();
        fs::create_dir_all(world).unwrap();
        // Lay the pack down exactly as install_tall_datapack does, minus level.dat.
        let templates: [(&str, &[u8]); 3] = [
            (
                "",
                include_bytes!(
                    "../assets/minecraft/datapack_tall/data/minecraft/dimension_type/overworld.json"
                ),
            ),
            (
                "overlay_attributes",
                include_bytes!("../assets/minecraft/datapack_tall/overlay_attributes/data/minecraft/dimension_type/overworld.json"),
            ),
            (
                "overlay_2601",
                include_bytes!("../assets/minecraft/datapack_tall/overlay_2601/data/minecraft/dimension_type/overworld.json"),
            ),
        ];
        let root = world
            .join("datapacks")
            .join(crate::world_utils::TALL_DATAPACK_NAME);
        for (dir, bytes) in templates {
            let d = root.join(dir).join("data/minecraft/dimension_type");
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("overworld.json"), bytes).unwrap();
        }

        // Absent flags: untouched.
        patch_datapack(world, &parse(&["--disable-height-limit"])).unwrap();
        for (dir, bytes) in templates {
            let p = root
                .join(dir)
                .join("data/minecraft/dimension_type/overworld.json");
            assert_eq!(fs::read(p).unwrap(), bytes);
        }

        let args = parse(&["--disable-height-limit", "--min-y=-128", "--max-y=447"]);
        patch_datapack(world, &args).unwrap();
        for (dir, bytes) in templates {
            let p = root
                .join(dir)
                .join("data/minecraft/dimension_type/overworld.json");
            let got: serde_json::Value = serde_json::from_slice(&fs::read(p).unwrap()).unwrap();
            assert_eq!(got["min_y"], -128, "{dir}");
            assert_eq!(got["height"], 576, "{dir}");
            assert_eq!(got["logical_height"], 576, "{dir}");
            // Everything else is the bundled schema.
            let mut want: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            for k in ["min_y", "height", "logical_height"] {
                want[k] = got[k].clone();
            }
            assert_eq!(got, want, "{dir}");
        }
    }
}
