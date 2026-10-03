//! Large One World jobs (`--unit-regions N`): the selection is cut into
//! region-group pieces (`work_units`) and each piece is built by a run of this
//! executable in piece mode (`--one-world-unit <lease>`).
//!
//! The coordinator opens the world like any One World run and holds its lock
//! for the whole job. Pieces own whole region files and write only their own
//! chunks and signage maps; everything that belongs to the world as a whole
//! (the manifest record, level.dat, the map id counter, metadata.json and the
//! area preview) is written here once, after the last piece. The elevation
//! mapping is the world's, fixed in the manifest before the first piece runs,
//! so no two pieces disagree on Y.
//!
//! A job keeps its state in `arnis_one_world/jobs/<rect>_n<N>/`: the map id
//! base it reserved, one `done-<piece>.json` per finished piece and the piece
//! previews. Running the same selection with the same N again resumes it.

use crate::args::Args;
use crate::coordinate_system::cartesian::XZBBox;
use crate::coordinate_system::geographic::{LLBBox, LLPoint};
use crate::one_world::{self, RunContext, UnitLease};
use crate::progress_json;
use crate::work_units::{plan_units, WorkUnit};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// Signage map ids each piece may use. A piece needing more fails loudly.
const MAP_IDS_PER_PIECE: i32 = 1 << 16;
/// Lines of a failed piece's output shown with its error.
const TAIL_LINES: usize = 40;

/// What a finished piece reported.
#[derive(Default, Clone, Debug)]
pub struct PieceResult {
    pub spawn_y: Option<i32>,
    pub peak_rss_mb: Option<u64>,
    pub wall_s: Option<f64>,
    pub chunks: u64,
}

impl PieceResult {
    fn to_json(&self) -> Value {
        json!({
            "spawn_y": self.spawn_y,
            "peak_rss_mb": self.peak_rss_mb,
            "wall_s": self.wall_s,
            "chunks": self.chunks,
        })
    }

    fn from_json(v: &Value) -> Self {
        Self {
            spawn_y: v["spawn_y"].as_i64().map(|y| y as i32),
            peak_rss_mb: v["peak_rss_mb"].as_u64(),
            wall_s: v["wall_s"].as_f64(),
            chunks: v["chunks"].as_u64().unwrap_or(0),
        }
    }
}

/// One job's folder and the decisions it keeps across a resume.
struct Job {
    dir: PathBuf,
    nonce: String,
    map_base: i32,
}

impl Job {
    fn open(world_dir: &Path, rect: &XZBBox, n: i32, map_base: i32) -> Result<Self, String> {
        let dir = world_dir.join(one_world::JOBS_DIR).join(format!(
            "{}_{}_{}_{}_n{n}",
            rect.min_x(),
            rect.min_z(),
            rect.max_x(),
            rect.max_z()
        ));
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;
        // The id base is kept, so a resumed job hands out the ranges it did before.
        let state = dir.join("job.json");
        let map_base = match std::fs::read_to_string(&state)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["map_base"].as_i64())
        {
            Some(base) => base as i32,
            None => {
                write(&state, &json!({ "map_base": map_base }))?;
                map_base
            }
        };
        let nonce = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        std::fs::write(world_dir.join(one_world::COORDINATOR_FILE), &nonce)
            .map_err(|e| format!("Failed to write the job's coordinator file: {e}"))?;
        Ok(Self {
            dir,
            nonce,
            map_base,
        })
    }

    fn done_path(&self, piece: usize) -> PathBuf {
        self.dir.join(format!("done-{piece}.json"))
    }

    fn preview_path(&self, piece: usize) -> PathBuf {
        self.dir.join(format!("piece-{piece}.png"))
    }

    fn finished(&self, piece: usize) -> Option<PieceResult> {
        let text = std::fs::read_to_string(self.done_path(piece)).ok()?;
        serde_json::from_str(&text)
            .ok()
            .map(|v| PieceResult::from_json(&v))
    }
}

fn write(path: &Path, value: &Value) -> Result<(), String> {
    crate::world_utils::replace_file_atomically(path, value.to_string().as_bytes())
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

/// Builds the selection `args` was opened on, piece by piece.
pub fn run(args: &Args, world_dir: &Path, selection: &LLBBox) -> Result<(), String> {
    let run = args
        .one_world_run
        .as_ref()
        .ok_or("--unit-regions needs --one-world")?;
    if args.terrain() && run.elevation.is_none() {
        return Err(
            "This One World was started by an older Arnis and takes its elevation mapping from \
             its first area, so pieces could disagree on height. Extend it without --unit-regions."
                .to_string(),
        );
    }
    let n = args.units.unit_regions.unwrap_or(4);
    let manifest = one_world::Manifest::load(world_dir)?
        .ok_or_else(|| format!("{} is not a One World.", world_dir.display()))?;
    let proj = manifest.projection();
    let (rect, units) = plan_units(&proj, selection, n)?;
    let fresh = !run.extending;

    let data_dir = world_dir.join("data");
    let extras = match (fresh, args.map_item) {
        (false, _) => 0,
        (true, true) => 2,
        (true, false) => 1,
    };
    let job = Job::open(
        world_dir,
        &rect,
        n,
        crate::map_item::next_map_id(&data_dir) + extras,
    )?;
    if i64::from(job.map_base) + units.len() as i64 * i64::from(MAP_IDS_PER_PIECE)
        > i64::from(i32::MAX)
    {
        return Err(format!(
            "{} pieces are too many for the map id space; use a larger --unit-regions.",
            units.len()
        ));
    }

    // Where the player spawns and the branding frame hangs, as one run would put them.
    let user_spawn = user_spawn_xz(args, selection)?;
    let spawn = user_spawn
        .filter(|&(x, z)| contains(&rect, x, z))
        .or(fresh.then_some((rect.min_x() + 1, rect.min_z() + 1)));
    let branding = fresh.then(|| {
        user_spawn
            .or_else(|| crate::map_item::read_spawn_xz(world_dir))
            .unwrap_or((rect.min_x() + 1, rect.min_z() + 1))
    });
    if fresh && args.world_type == crate::args::WorldType::Void {
        crate::world_utils::remove_untouched_template_region(world_dir);
    }

    println!(
        "One World: building {} piece(s) of up to {n}x{n} regions",
        units.len()
    );
    let leases: Vec<UnitLease> = units
        .iter()
        .map(|u| UnitLease {
            nonce: job.nonce.clone(),
            piece: u.index,
            area_id: run.area_id,
            rect: [
                u.rect.min_x(),
                u.rect.min_z(),
                u.rect.max_x(),
                u.rect.max_z(),
            ],
            first_map_id: job.map_base + u.index as i32 * MAP_IDS_PER_PIECE,
            map_id_end: job.map_base + (u.index as i32 + 1) * MAP_IDS_PER_PIECE,
            spawn: spawn
                .filter(|&(x, z)| contains(&u.rect, x, z))
                .map(|(x, z)| [x, z]),
            branding: branding
                .filter(|&(x, z)| contains(&u.rect, x, z))
                .map(|(x, z)| (x, z, args.map_item)),
            preview: job.preview_path(u.index),
        })
        .collect();

    let total: u64 = units.iter().map(WorkUnit::chunks).sum();
    let mut done_chunks = 0u64;
    let mut results = Vec::with_capacity(units.len());
    for (unit, lease) in units.iter().zip(&leases) {
        let of = units.len();
        if let Some(r) = job.finished(unit.index) {
            println!("  piece {}/{of}: already built", unit.index + 1);
            progress_json::record(
                "piece",
                json!({"piece": unit.index, "of": of, "state": "skipped"}),
            );
            done_chunks += unit.chunks();
            results.push(r);
            continue;
        }
        println!(
            "  piece {}/{of}: {} chunks at x {} z {}",
            unit.index + 1,
            unit.chunks(),
            unit.rect.min_x(),
            unit.rect.min_z()
        );
        progress_json::record(
            "piece",
            json!({"piece": unit.index, "of": of, "state": "start"}),
        );
        let lease_path = job.dir.join(format!("piece-{}.lease.json", unit.index));
        write(
            &lease_path,
            &serde_json::to_value(lease).map_err(|e| e.to_string())?,
        )?;
        let argv = child_args(std::env::args_os().skip(1), unit, &lease_path);
        let base = done_chunks;
        let result = run_piece(&argv, |f| {
            let pct = (base as f64 + f * unit.chunks() as f64) / total.max(1) as f64 * 100.0;
            progress_json::progress(pct, "");
        });
        let r = match result {
            Ok(r) => r,
            Err(e) => {
                progress_json::record(
                    "piece",
                    json!({"piece": unit.index, "of": of, "state": "failed"}),
                );
                return Err(format!(
                    "piece {} of {of} failed; run the same command again to resume.\n{e}",
                    unit.index + 1
                ));
            }
        };
        write(&job.done_path(unit.index), &r.to_json())?;
        crate::keep_one_world();
        progress_json::record(
            "piece",
            json!({"piece": unit.index, "of": of, "state": "done",
                   "peak_rss_mb": r.peak_rss_mb, "wall_s": r.wall_s}),
        );
        done_chunks += unit.chunks();
        progress_json::CHUNKS_WRITTEN.fetch_add(r.chunks, std::sync::atomic::Ordering::Relaxed);
        results.push(r);
    }

    finish(
        args,
        run,
        world_dir,
        selection,
        &rect,
        &units,
        &job,
        fresh,
        spawn.zip(results.iter().find_map(|r| r.spawn_y)),
    )?;
    let _ = std::fs::remove_dir_all(&job.dir);
    let _ = std::fs::remove_file(world_dir.join(one_world::COORDINATOR_FILE));
    // Gone unless another job is still waiting to be resumed.
    let _ = std::fs::remove_dir(world_dir.join(one_world::JOBS_DIR));
    Ok(())
}

/// The world-wide writes of a finished job, in the order one run makes them.
#[allow(clippy::too_many_arguments)]
fn finish(
    args: &Args,
    run: &RunContext,
    world_dir: &Path,
    selection: &LLBBox,
    rect: &XZBBox,
    units: &[WorkUnit],
    job: &Job,
    fresh: bool,
    spawn: Option<((i32, i32), i32)>,
) -> Result<(), String> {
    let preview_png = run.preview_path();
    let preview = stitch_previews(rect, units, job, &preview_png);
    if let Err(e) = &preview {
        eprintln!("Warning: Failed to build the area preview: {e}");
    }
    if fresh {
        let written = match (&preview, args.map_item) {
            (Ok((img, step)), true) => crate::map_item::write_map_item_image(
                world_dir,
                img,
                (rect.min_x(), rect.min_z(), *step),
                rect,
            ),
            _ => crate::map_item::write_branding_map_only(world_dir),
        };
        if let Err(e) = written {
            eprintln!("Warning: Failed to create the world's maps: {e}");
        }
    }
    crate::map_item::sync_map_counter(world_dir)?;

    one_world::record_area(
        run,
        selection,
        rect,
        None,
        preview.is_ok().then_some(preview_png.as_path()),
    )
    .map_err(|e| format!("The area was written but could not be recorded: {e}"))?;
    println!("One World: area #{} recorded.", run.area_id);
    if let Err(e) = update_metadata(world_dir, args) {
        eprintln!("Warning: Failed to update metadata.json: {e}");
    }
    if let Err(e) = crate::world_utils::touch_last_played(world_dir) {
        eprintln!("Warning: Failed to update LastPlayed: {e}");
    }
    if fresh {
        if let Err(e) = crate::world_utils::apply_java_world_settings(
            world_dir,
            args.gamemode,
            args.world_time,
            args.world_type,
        ) {
            eprintln!("Warning: Failed to apply world settings: {e}");
        }
    }
    if let Some(((x, z), y)) = spawn {
        if let Err(e) = crate::world_utils::set_spawn_in_level_dat(world_dir, x, y, z) {
            eprintln!("Warning: Failed to set the spawn point: {e}");
        }
    }
    Ok(())
}

/// One image of the whole job from the piece previews, at the step a single
/// run of the job would use, written to `out`. Returns it with that step.
fn stitch_previews(
    rect: &XZBBox,
    units: &[WorkUnit],
    job: &Job,
    out: &Path,
) -> Result<(image::RgbImage, u32), String> {
    let w = (rect.max_x() - rect.min_x() + 1) as u32;
    let h = (rect.max_z() - rect.min_z() + 1) as u32;
    // As PreviewAccumulator::new_capped(rect, 2048) picks it.
    let step = w.max(h).div_ceil(2048).max(1);
    let mut img = image::RgbImage::new(w.div_ceil(step), h.div_ceil(step));
    for u in units {
        let piece = image::open(job.preview_path(u.index))
            .map_err(|e| format!("piece {}: {e}", u.index))?
            .to_rgb8();
        let pw = ((u.rect.max_x() - u.rect.min_x() + 1) as u32).div_ceil(step);
        let ph = ((u.rect.max_z() - u.rect.min_z() + 1) as u32).div_ceil(step);
        let piece = image::imageops::resize(&piece, pw, ph, image::imageops::FilterType::Triangle);
        image::imageops::replace(
            &mut img,
            &piece,
            i64::from((u.rect.min_x() - rect.min_x()) / step as i32),
            i64::from((u.rect.min_z() - rect.min_z()) / step as i32),
        );
    }
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    img.save(out).map_err(|e| e.to_string())?;
    println!("Map preview saved to: {}", out.display());
    Ok((img, step))
}

/// metadata.json describes the whole world; pieces each wrote their own
/// view of it. Rewritten as one run of the job would have written it.
fn update_metadata(world_dir: &Path, args: &Args) -> Result<(), String> {
    let manifest = one_world::Manifest::load(world_dir)?.ok_or("no manifest")?;
    let Some(ext) = manifest.extent() else {
        return Ok(());
    };
    let ll = crate::projection::llbbox_for_rect(&manifest.projection(), &ext)?;
    let meta = crate::world_editor::WorldMetadata {
        min_mc_x: ext.min_x(),
        max_mc_x: ext.max_x(),
        min_mc_z: ext.min_z(),
        max_mc_z: ext.max_z(),
        min_geo_lat: ll.min().lat(),
        max_geo_lat: ll.max().lat(),
        min_geo_lon: ll.min().lng(),
        max_geo_lon: ll.max().lng(),
        projection: args.projection.to_string(),
        scale: args.scale,
    };
    let text = serde_json::to_string(&meta).map_err(|e| e.to_string())?;
    std::fs::write(world_dir.join("metadata.json"), text).map_err(|e| e.to_string())
}

fn contains(rect: &XZBBox, x: i32, z: i32) -> bool {
    rect.contains(&crate::coordinate_system::cartesian::XZPoint::new(x, z))
}

/// `--spawn-lat/--spawn-lng` in the world's blocks, as a run converts them.
fn user_spawn_xz(args: &Args, selection: &LLBBox) -> Result<Option<(i32, i32)>, String> {
    let (Some(lat), Some(lng)) = (args.spawn_lat, args.spawn_lng) else {
        return Ok(None);
    };
    let point = LLPoint::new(lat, lng).map_err(|e| format!("Invalid spawn coordinates: {e}"))?;
    let (transformer, _) =
        crate::projection::ProjectionSpec::from_args(args).transformer(selection)?;
    let p = transformer.transform_point(point);
    Ok(Some((p.x, p.z)))
}

/// Options the coordinator decides per piece, so the user's are dropped.
const PER_PIECE: &[&str] = &[
    "--bbox",
    "--unit-regions",
    "--one-world-workers",
    "--plan-units",
    "--progress",
    "--spawn-lat",
    "--spawn-lng",
    "--threads",
    "--cpu-target",
    "--ram-budget-mb",
    "--max-downloads",
    "--one-world-unit",
];

/// A piece's command line: the job's own, minus what is decided per piece,
/// plus the piece's bbox at full precision and its lease.
fn child_args(
    argv: impl Iterator<Item = OsString>,
    unit: &WorkUnit,
    lease: &Path,
) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut skip_value = false;
    for arg in argv {
        if std::mem::take(&mut skip_value) {
            continue;
        }
        let text = arg.to_string_lossy();
        let name = text.split('=').next().unwrap_or_default();
        if PER_PIECE.contains(&name) {
            skip_value = !text.contains('=');
            continue;
        }
        out.push(arg);
    }
    out.extend([
        "--bbox".into(),
        unit.bbox_arg().into(),
        "--one-world-unit".into(),
        lease.as_os_str().to_owned(),
        "--progress".into(),
        "json".into(),
        "--no-update-check".into(),
        "--no-cache-sweep".into(),
    ]);
    out
}

/// Runs one piece to the end. `progress` gets the piece's own fraction done.
fn run_piece(argv: &[OsString], mut progress: impl FnMut(f64)) -> Result<PieceResult, String> {
    let exe = std::env::current_exe().map_err(|e| format!("Cannot find this executable: {e}"))?;
    let mut child = Command::new(exe)
        .args(argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to start a piece: {e}"))?;
    let tail = Arc::new(Mutex::new(VecDeque::with_capacity(TAIL_LINES)));
    let push = |tail: &Mutex<VecDeque<String>>, line: String| {
        let mut t = tail.lock().unwrap_or_else(|e| e.into_inner());
        if t.len() == TAIL_LINES {
            t.pop_front();
        }
        t.push_back(line);
    };
    let stderr = child.stderr.take().map(|err| {
        let tail = Arc::clone(&tail);
        std::thread::spawn(move || {
            for line in BufReader::new(err).split(b'\n').map_while(Result::ok) {
                push(&tail, String::from_utf8_lossy(&line).trim_end().to_string());
            }
        })
    });
    let mut result = PieceResult::default();
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).split(b'\n').map_while(Result::ok) {
            let line = String::from_utf8_lossy(&line).trim_end().to_string();
            let Some(record) = line
                .starts_with(r#"{"v":1"#)
                .then(|| serde_json::from_str::<Value>(&line).ok())
                .flatten()
            else {
                push(&tail, line);
                continue;
            };
            match record["type"].as_str() {
                Some("phase") | Some("progress") => {
                    if let Some(p) = record["progress"].as_f64() {
                        progress(p / 100.0);
                    }
                }
                Some("result") => result.spawn_y = record["spawn_y"].as_i64().map(|y| y as i32),
                Some("done") => {
                    result.peak_rss_mb = record["peak_rss_mb"].as_u64();
                    result.wall_s = record["wall_s"].as_f64();
                    result.chunks = record["chunks"].as_u64().unwrap_or(0);
                }
                Some("error") => push(&tail, line),
                _ => {}
            }
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if let Some(t) = stderr {
        let _ = t.join();
    }
    if status.success() {
        return Ok(result);
    }
    let tail = tail.lock().unwrap_or_else(|e| e.into_inner());
    Err(format!(
        "exit status {status}; last output:\n{}",
        tail.iter().cloned().collect::<Vec<_>>().join("\n")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_piece_gets_the_jobs_options_but_its_own_bbox_and_lease() {
        let proj = crate::projection::WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        let (_, units) = plan_units(&proj, &req, 1).unwrap();
        let argv = [
            "--one-world",
            "--bbox",
            "-1,-2,3,4",
            "--unit-regions=1",
            "--threads",
            "8",
            "--spawn-lat",
            "-44.4",
            "--output-dir",
            "out dir",
            "--progress",
            "json",
            "--scale=1",
        ]
        .map(OsString::from);
        let got = child_args(argv.into_iter(), &units[3], Path::new("l.json"));
        let got: Vec<_> = got
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            got,
            [
                "--one-world",
                "--output-dir",
                "out dir",
                "--scale=1",
                "--bbox",
                &units[3].bbox_arg(),
                "--one-world-unit",
                "l.json",
                "--progress",
                "json",
                "--no-update-check",
                "--no-cache-sweep",
            ]
        );
        // What the piece parses is the bbox that snaps to its rectangle.
        let parsed = LLBBox::from_str(&got[5]).unwrap();
        let (rect, _) = crate::projection::snap_bbox_to_chunks(&proj, &parsed).unwrap();
        assert_eq!(rect.min_x(), units[3].rect.min_x());
        assert_eq!(rect.max_z(), units[3].rect.max_z());
    }

    #[test]
    fn a_piece_result_survives_the_done_file() {
        let r = PieceResult {
            spawn_y: Some(-40),
            peak_rss_mb: Some(900),
            wall_s: Some(1.5),
            chunks: 1024,
        };
        let back = PieceResult::from_json(&r.to_json());
        assert_eq!(back.spawn_y, Some(-40));
        assert_eq!(back.peak_rss_mb, Some(900));
        assert_eq!(back.chunks, 1024);
    }
}
