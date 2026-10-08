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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

pub mod budget;
pub mod child;

/// Signage map ids each piece may use. A piece needing more fails loudly.
const MAP_IDS_PER_PIECE: i32 = 1 << 16;
/// Lines of a failed piece's output shown with its error.
const TAIL_LINES: usize = 40;
/// Further attempts at a piece that failed for a reason that may pass.
const MAX_RETRIES: u32 = 2;

/// The window's run controls (gui.rs). Pause holds the queue: no piece
/// starts, the running ones finish, and the job stays resumable. Stop ends
/// the job and kills its running pieces; the finished ones are kept, so the
/// same run again resumes. Set only from the window, which clears both when
/// a run starts.
pub static PAUSE: AtomicBool = AtomicBool::new(false);
pub static STOP: AtomicBool = AtomicBool::new(false);

/// What a job returns when Stop ended it.
pub const STOPPED: &str = "Stopped.";

/// Waits while the job is paused, saying so once. False once it is stopping.
fn hold_while_paused() -> bool {
    let mut said = false;
    while PAUSE.load(Ordering::Acquire) && !STOP.load(Ordering::Acquire) {
        if !std::mem::replace(&mut said, true) {
            crate::progress::emit_gui_progress_update(crate::progress::MESSAGE_ONLY, "Paused.");
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    !STOP.load(Ordering::Acquire)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a finished piece reported.
#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PieceResult {
    pub spawn_y: Option<i32>,
    pub peak_rss_mb: Option<u64>,
    pub wall_s: Option<f64>,
    pub chunks: u64,
}

/// One job's folder and the decisions it keeps across a resume.
struct Job {
    dir: PathBuf,
    nonce: String,
    map_base: i32,
}

impl Job {
    fn dir(world_dir: &Path, rect: &XZBBox, n: i32) -> PathBuf {
        world_dir.join(one_world::JOBS_DIR).join(format!(
            "{}_{}_{}_{}_n{n}",
            rect.min_x(),
            rect.min_z(),
            rect.max_x(),
            rect.max_z()
        ))
    }

    fn open(world_dir: &Path, rect: &XZBBox, n: i32, map_base: i32) -> Result<Self, String> {
        let dir = Self::dir(world_dir, rect, n);
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
        serde_json::from_str(&text).ok()
    }
}

fn write(path: &Path, value: &Value) -> Result<(), String> {
    crate::world_utils::replace_file_atomically(path, value.to_string().as_bytes())
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

/// Builds the selection `args` was opened on, piece by piece. `argv` is the
/// command line that asks for `args`, without the executable: each piece
/// runs it with its own bbox and lease.
pub fn run(
    args: &Args,
    world_dir: &Path,
    selection: &LLBBox,
    argv: &[OsString],
) -> Result<(), String> {
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

    // The map folder of the world's layout, which Minecraft 26.1+ moves.
    let maps_dir = crate::world_utils::WorldLayout::of(world_dir).maps_dir(world_dir);
    let extras = match (fresh, args.map_item) {
        (false, _) => 0,
        (true, true) => 2,
        (true, false) => 1,
    };
    let job = Job::open(
        world_dir,
        &rect,
        n,
        crate::map_item::next_map_id(&maps_dir) + extras,
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
    // The extract is read once, here, on this process's threads; each piece then cuts
    // its own area from the bake.
    if let Some(src) = crate::osm_pbf::Source::from_args(args).filter(|_| !args.skip_objects()) {
        crate::osm_pbf::bake_for_job(&src, *selection)?;
    }
    // As `generate_world_with_options` decides it for one run of the whole selection.
    let tiled = crate::tile::create_tiles(&rect, crate::tile::DEFAULT_TILE_SIZE).len() >= 3;
    let leases: Vec<UnitLease> = units
        .iter()
        .map(|u| UnitLease {
            nonce: job.nonce.clone(),
            piece: u.index,
            area_id: run.area_id,
            rect: u.rect.to_array(),
            build: one_world::piece_build_rect(&u.rect, &rect).to_array(),
            tiled,
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
    let lease_file = |i: usize| -> Result<PathBuf, String> {
        let path = job.dir.join(format!("piece-{i}.lease.json"));
        write(
            &path,
            &serde_json::to_value(&leases[i]).map_err(|e| e.to_string())?,
        )?;
        Ok(path)
    };

    // `--prewarm-first`: one piece at a time, each with the job's whole download
    // allowance, so N workers then read the caches instead of all fetching at once.
    if args.process.prewarm || args.process.prewarm_first {
        for unit in units.iter().filter(|u| job.finished(u.index).is_none()) {
            if !hold_while_paused() {
                return Err(STOPPED.to_string());
            }
            let i = unit.index;
            let mut argv = child_args(argv.iter().cloned(), unit, &lease_file(i)?);
            let downloads = args.process.max_downloads.unwrap_or(16).to_string();
            argv.extend([
                "--prewarm".into(),
                "--max-downloads".into(),
                downloads.into(),
            ]);
            println!("  piece {}/{}: warming the caches", i + 1, units.len());
            run_piece_until(&argv, &[], Some(&STOP), |_| {}).map_err(|f| {
                if STOP.load(Ordering::Acquire) {
                    STOPPED.to_string()
                } else {
                    format!("warming piece {} failed: {}", i + 1, f.message)
                }
            })?;
        }
        // `--prewarm` alone stops here, before anything is built.
        if args.process.prewarm {
            close_job(&job, world_dir);
            return Ok(());
        }
    }

    let largest = units
        .iter()
        .map(|u| u.chunks().div_ceil(1024))
        .max()
        .unwrap_or(1);
    let sizing = budget::sizing(args, units.len(), largest);
    if sizing.workers > 1 {
        println!(
            "  {} pieces at a time, {} threads each",
            sizing.workers, sizing.threads
        );
    }
    // Pieces running, and how many may: `auto` lowers the cap once a
    // finished piece shows pieces need more memory than estimated.
    let running = Mutex::new(0usize);
    let cap = AtomicUsize::new(sizing.workers);
    let of = units.len();
    // Each piece's area, in the world's blocks and on the map, on its records.
    let bounds: Vec<Option<[f64; 4]>> = units
        .iter()
        .map(|u| {
            crate::projection::llbbox_for_rect(&proj, &u.rect)
                .ok()
                .map(|b| [b.min().lat(), b.min().lng(), b.max().lat(), b.max().lng()])
        })
        .collect();
    let piece_record = |i: usize, state: &str, extra: Value| {
        let mut body = json!({"piece": i, "of": of, "state": state,
                              "rect": units[i].rect.to_array(), "bounds": bounds[i]});
        if let (Value::Object(body), Value::Object(extra)) = (&mut body, extra) {
            body.extend(extra);
        }
        progress_json::record("piece", body.clone());
        crate::progress::emit_gui_piece(&body);
    };
    let results: Mutex<Vec<Option<PieceResult>>> = Mutex::new(vec![None; of]);
    let mut queue = VecDeque::new();
    for unit in &units {
        match job.finished(unit.index) {
            Some(r) => {
                println!("  piece {}/{of}: already built", unit.index + 1);
                piece_record(unit.index, "skipped", json!({}));
                lock(&results)[unit.index] = Some(r);
            }
            None => queue.push_back(unit.index),
        }
    }
    let queue = Mutex::new(queue);
    // Each piece's fraction done, and the highest job percentage sent.
    let done: Mutex<(Vec<f64>, f64)> = Mutex::new((
        lock(&results)
            .iter()
            .map(|r| if r.is_some() { 1.0 } else { 0.0 })
            .collect(),
        0.0,
    ));
    let finished = AtomicUsize::new(of - lock(&queue).len());
    // The window's status line: the count, and whether the queue is held.
    let status = || {
        if !crate::progress::is_running_with_gui() {
            // `--progress json` has its piece records, so its stream stays as it was.
            return String::new();
        }
        let n = finished.load(Ordering::Relaxed);
        let mut line = format!("Building pieces... {n}/{of} done");
        if PAUSE.load(Ordering::Acquire) {
            match *lock(&running) {
                0 => line.push_str(". Paused."),
                r => line.push_str(&format!(". Pausing: {r} running to finish.")),
            }
        }
        line
    };
    // The fraction each running piece last showed on the map.
    let shown = Mutex::new(vec![0.0; of]);
    // Piece `i` is `f` done: moves the job's bar and, in the window, the count.
    let report = |i: usize, f: f64| {
        let mut d = lock(&done);
        d.0[i] = f;
        // A retried piece starts over; the bar waits for it instead.
        d.1 = d.1.max(job_percent(&d.0));
        let percent = d.1;
        drop(d);
        crate::progress::emit_gui_progress_update(percent, &status());
        // The map fills a running piece's cell in steps; window only.
        let step = {
            let mut s = lock(&shown);
            let step = f < 1.0 && f - s[i] >= 0.05;
            if step {
                s[i] = f;
            }
            step
        };
        if step {
            crate::progress::emit_gui_piece(
                &json!({"piece": i, "of": of, "state": "progress", "fraction": f}),
            );
        }
    };
    let paused_said = AtomicBool::new(false);
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let aborting = AtomicBool::new(false);

    let work = || -> Result<(), String> {
        while !aborting.load(Ordering::Relaxed) && !STOP.load(Ordering::Acquire) {
            // Paused: the queue waits while the running pieces finish.
            if PAUSE.load(Ordering::Acquire) {
                if !paused_said.swap(true, Ordering::Relaxed) {
                    crate::progress::emit_gui_progress_update(
                        crate::progress::MESSAGE_ONLY,
                        &status(),
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
            if paused_said.swap(false, Ordering::Relaxed) {
                crate::progress::emit_gui_progress_update(crate::progress::MESSAGE_ONLY, &status());
            }
            {
                let mut r = lock(&running);
                if *r >= cap.load(Ordering::Relaxed) {
                    drop(r);
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    continue;
                }
                *r += 1;
            }
            let Some(i) = lock(&queue).pop_front() else {
                *lock(&running) -= 1;
                break;
            };
            let unit = &units[i];
            let argv = sizing.child_args(child_args(argv.iter().cloned(), unit, &lease_file(i)?));
            let mut attempt = 0;
            let r = loop {
                println!(
                    "  piece {}/{of}: {} chunks at x {} z {}{}",
                    i + 1,
                    unit.chunks(),
                    unit.rect.min_x(),
                    unit.rect.min_z(),
                    if attempt > 0 { " (retry)" } else { "" }
                );
                let state = if attempt > 0 { "retry" } else { "start" };
                lock(&shown)[i] = 0.0;
                piece_record(i, state, json!({}));
                let result =
                    run_piece_until(&argv, &sizing.child_env(), Some(&STOP), |f| report(i, f));
                match result {
                    Ok(r) => break r,
                    // Killed by Stop: not a failure, and built again on resume.
                    Err(_) if STOP.load(Ordering::Acquire) => {
                        piece_record(i, "stopped", json!({}));
                        *lock(&running) -= 1;
                        return Ok(());
                    }
                    // Never once the job is stopping: a piece killed with it
                    // looks like a crash.
                    Err(f)
                        if f.transient
                            && attempt < MAX_RETRIES
                            && !aborting.load(Ordering::Relaxed) =>
                    {
                        attempt += 1;
                        eprintln!("Piece {} failed, retrying: {}", i + 1, f.message);
                        std::thread::sleep(std::time::Duration::from_secs(5 * attempt as u64));
                    }
                    Err(f) => {
                        piece_record(i, "failed", json!({}));
                        aborting.store(true, Ordering::Relaxed);
                        return Err(format!("piece {} of {of} failed: {}", i + 1, f.message));
                    }
                }
            };
            *lock(&running) -= 1;
            if let Some(peak) = r.peak_rss_mb {
                let allowed = sizing.cap_for(peak);
                if cap.fetch_min(allowed, Ordering::Relaxed) > allowed {
                    println!("  pieces peak at {peak} MB: {allowed} at a time from now on");
                }
            }
            write(&job.done_path(i), &json!(r))?;
            crate::keep_one_world();
            let pieces_done = finished.fetch_add(1, Ordering::Relaxed) + 1;
            piece_record(
                i,
                "done",
                json!({"peak_rss_mb": r.peak_rss_mb, "wall_s": r.wall_s,
                       "pieces_done": pieces_done}),
            );
            progress_json::CHUNKS_WRITTEN.fetch_add(r.chunks, Ordering::Relaxed);
            report(i, 1.0);
            lock(&results)[i] = Some(r);
        }
        Ok(())
    };
    std::thread::scope(|s| {
        for _ in 0..sizing.workers {
            s.spawn(|| {
                if let Err(e) = work() {
                    aborting.store(true, Ordering::Relaxed);
                    lock(&failure).get_or_insert(e);
                }
            });
        }
    });
    if let Some(e) = failure.into_inner().unwrap_or_else(|e| e.into_inner()) {
        return Err(format!(
            "{e}\nFinished pieces are kept; run the same command again to resume."
        ));
    }
    // Stopped before the last piece: kept for a resume, not finished.
    if finished.load(Ordering::Relaxed) < of {
        return Err(STOPPED.to_string());
    }
    // Folded in plan order, so the outcome does not depend on which piece
    // finished first.
    let results: Vec<PieceResult> = results
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect();

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
    close_job(&job, world_dir);
    Ok(())
}

/// Forgets the finished pieces of the job that building `selection` in
/// `world_dir` with `--unit-regions n` would resume, so it starts fresh.
/// Their chunks stay until the pieces build them again.
pub fn forget_job(world_dir: &Path, selection: &LLBBox, n: i32) -> Result<(), String> {
    let Some(manifest) = one_world::Manifest::load(world_dir)? else {
        return Ok(());
    };
    let (rect, _) = plan_units(&manifest.projection(), selection, n)?;
    let dir = Job::dir(world_dir, &rect, n);
    match std::fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("Failed to remove {}: {e}", dir.display()))
        }
        _ => Ok(()),
    }
}

fn close_job(job: &Job, world_dir: &Path) {
    let _ = std::fs::remove_dir_all(&job.dir);
    let _ = std::fs::remove_file(world_dir.join(one_world::COORDINATOR_FILE));
    // Gone unless another job is still waiting to be resumed.
    let _ = std::fs::remove_dir(world_dir.join(one_world::JOBS_DIR));
}

/// Whether a job in `world_dir` has finished a piece, which makes the world
/// worth keeping after a failure: running the job again resumes it.
pub fn has_finished_pieces(world_dir: &Path) -> bool {
    let Ok(jobs) = std::fs::read_dir(world_dir.join(one_world::JOBS_DIR)) else {
        return false;
    };
    jobs.flatten()
        .filter_map(|job| std::fs::read_dir(job.path()).ok())
        .flat_map(|files| files.flatten())
        .any(|f| f.file_name().to_string_lossy().starts_with("done-"))
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
    // Once for the job, around every area the world now holds.
    if args.world_border {
        one_world::apply_world_border(world_dir, rect);
    }
    // After every piece, so this is the database's only writer.
    if args.dh_lod {
        crate::dh_lod::run(world_dir, rect, fresh);
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

/// Switches the coordinator decides, so the user's are dropped: it hands
/// `--prewarm` to the pieces it warms, and clap refuses one given twice.
const PER_PIECE_SWITCHES: &[&str] = &["--prewarm", "--prewarm-first"];

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
        if PER_PIECE_SWITCHES.contains(&name) {
            continue;
        }
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
    ]);
    out
}

/// Why a piece failed, and whether running it again may help.
pub(crate) struct PieceFailure {
    pub(crate) message: String,
    transient: bool,
}

/// Network trouble, or a piece that died without saying why (killed, out of
/// memory), may pass; a reported error, a panic or a stop will not.
fn is_transient(code: Option<i32>, tail: &[String]) -> bool {
    const NETWORK: &[&str] = &[
        "timed out",
        "timeout",
        "connection",
        "network",
        "dns",
        "429",
        "502",
        "503",
        "504",
        "too many requests",
        "rate limit",
        "temporarily",
    ];
    // 101 is a panic; 0xC000013A a Ctrl-C on Windows; no code is a signal.
    let Some(code) = code.filter(|&c| c != 101 && c != 0xC000_013Au32 as i32) else {
        return false;
    };
    let text = tail.join("\n").to_lowercase();
    let reported = text.contains("error");
    code != 0 && (!reported || NETWORK.iter().any(|w| text.contains(w)))
}

/// Runs one piece to the end, or kills it as soon as `cancel` is set.
/// `progress` gets the piece's own fraction done. Its `transfer` records
/// (a download or bake in the child) go on to the window as they come.
pub(crate) fn run_piece_until(
    argv: &[OsString],
    env: &[(&str, String)],
    cancel: Option<&AtomicBool>,
    mut progress: impl FnMut(f64),
) -> Result<PieceResult, PieceFailure> {
    let fail = |message: String| PieceFailure {
        message,
        transient: false,
    };
    let exe =
        std::env::current_exe().map_err(|e| fail(format!("Cannot find this executable: {e}")))?;
    let mut cmd = Command::new(exe);
    // Every piece skips the update check and the cache sweep; through the
    // environment, so a user's own switch for either does not clash.
    cmd.args(argv)
        .env("ARNIS_NO_UPDATE_CHECK", "1")
        .env("ARNIS_NO_CACHE_SWEEP", "1")
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    child::prepare(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| fail(format!("Failed to start a piece: {e}")))?;
    if let Err(e) = child::adopt(&child) {
        let _ = child.kill();
        return Err(fail(format!("Could not tie a piece to this process: {e}")));
    }
    // Held open for the piece's life: on Unix it exits when this closes.
    let _stdin = child.stdin.take();
    let tail = Arc::new(Mutex::new(VecDeque::with_capacity(TAIL_LINES)));
    let push = |tail: &Mutex<VecDeque<String>>, line: String| {
        let mut t = lock(tail);
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
    let stdout = child.stdout.take();
    let child = Mutex::new(child);
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        // Killing the child closes its pipes, which ends the read below.
        if let Some(cancel) = cancel {
            scope.spawn(|| {
                while !finished.load(Ordering::Acquire) {
                    if cancel.load(Ordering::Acquire) {
                        let _ = lock(&child).kill();
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            });
        }
        let Some(out) = stdout else {
            finished.store(true, Ordering::Release);
            return;
        };
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
                Some("transfer") => {
                    if let Ok(t) =
                        <crate::transfer::Transfer as serde::Deserialize>::deserialize(&record)
                    {
                        crate::progress::emit_gui_transfer(crate::progress::MESSAGE_ONLY, "", &t);
                    }
                }
                _ => {}
            }
        }
        finished.store(true, Ordering::Release);
    });
    let status = lock(&child).wait().map_err(|e| fail(e.to_string()))?;
    if let Some(t) = stderr {
        let _ = t.join();
    }
    if status.success() {
        return Ok(result);
    }
    let tail: Vec<String> = lock(&tail).iter().cloned().collect();
    Err(PieceFailure {
        transient: is_transient(status.code(), &tail),
        message: format!("{status}; last output:\n{}", tail.join("\n")),
    })
}

/// The job's percentage: every piece is an equal share, so it reads n/N as
/// "n/N done" does, plus the running pieces' own fractions. Not chunk-weighted:
/// the large middle pieces go first, which put the bar far ahead of the count.
fn job_percent(done: &[f64]) -> f64 {
    done.iter().map(|f| f.clamp(0.0, 1.0)).sum::<f64>() / done.len().max(1) as f64 * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_job_percentage_follows_the_pieces_done() {
        // 2 of 16 done, two running at a half and a quarter.
        let mut d = vec![0.0; 16];
        d[0] = 1.0;
        d[1] = 1.0;
        d[2] = 0.5;
        d[3] = 0.25;
        assert!((job_percent(&d) - 2.75 / 16.0 * 100.0).abs() < 1e-9);
        // Only finished pieces: exactly n/N.
        d[2] = 1.0;
        d[3] = 0.0;
        assert!((job_percent(&d) - 18.75).abs() < 1e-9);
        assert_eq!(job_percent(&[1.0; 16]), 100.0);
        assert_eq!(job_percent(&[]), 0.0);
    }

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
            "--no-update-check",
            "--prewarm-first",
            "--offline",
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
                "--no-update-check",
                "--offline",
                "--scale=1",
                "--bbox",
                &units[3].bbox_arg(),
                "--one-world-unit",
                "l.json",
                "--progress",
                "json",
            ]
        );
        // What the piece parses is the bbox that snaps to its rectangle.
        let parsed = LLBBox::from_str(&got[7]).unwrap();
        let (rect, _) = crate::projection::snap_bbox_to_chunks(&proj, &parsed).unwrap();
        assert_eq!(rect.min_x(), units[3].rect.min_x());
        assert_eq!(rect.max_z(), units[3].rect.max_z());
    }

    #[test]
    fn only_failures_that_may_pass_are_retried() {
        let lines = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Killed or crashed without a word: worth another go.
        assert!(is_transient(Some(1), &lines(&["Processing data..."])));
        assert!(is_transient(
            Some(1),
            &lines(&["Error: Failed to fetch data: operation timed out"])
        ));
        assert!(!is_transient(
            Some(1),
            &lines(&["Error: This area cannot be added to the One World"])
        ));
        assert!(!is_transient(Some(101), &lines(&["thread panicked"])));
        assert!(!is_transient(Some(0xC000_013Au32 as i32), &[]));
        assert!(!is_transient(None, &[]));
    }
}
