//! Large One World jobs (`--unit-regions N`, `--one-world-workers`): the selection is cut
//! into region-group pieces (`work_units`) and each piece is built by a run of this
//! executable on the piece's own bbox (`--one-world-unit <lease>`), several at once.
//!
//! The coordinator opens the world like any One World run and holds its lock for the
//! whole job. A piece owns whole region files, so pieces never write one file together.
//! The world's elevation mapping is fixed in the manifest at creation, so no two pieces
//! disagree on Y. The piece that makes the world's extras (map item, settings, spawn) runs
//! first and alone; the rest follow, side by side. Each finished piece is recorded in the
//! manifest by the coordinator alone, as an area that names its job and position.
//!
//! A job keeps its state in `arnis_one_world/jobs/<x0>_<z0>_<x1>_<z1>_n<N>/` (the rectangle
//! is the union of the pieces): the ids it reserved, one `done-<piece>.json` per finished
//! piece and the piece leases. Running the same command again resumes it.

use crate::args::Args;
use crate::coordinate_system::cartesian::XZBBox;
use crate::one_world::{self, Manifest, PieceRef, RunContext, UnitLease};
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

/// Map ids each piece may use. A piece needing more would run into the next one's range.
const MAP_IDS_PER_PIECE: i32 = 1 << 16;
/// Lines of a failed piece's output shown with its error.
const TAIL_LINES: usize = 40;
/// Further attempts at a piece that failed for a reason that may pass.
const MAX_RETRIES: u32 = 2;
/// Piece size when only `--one-world-workers` is given.
const DEFAULT_UNIT_REGIONS: i32 = 4;

/// Pause holds the queue: no piece starts, the running ones finish, and the job stays
/// resumable. Stop ends the job and kills its running pieces; the finished ones are kept,
/// so the same run again resumes. Set by whatever drives this process (the window).
pub static PAUSE: AtomicBool = AtomicBool::new(false);
pub static STOP: AtomicBool = AtomicBool::new(false);

/// What a job returns when Stop ended it.
pub const STOPPED: &str = "Stopped.";

/// What a job returns when it built the pieces `--only-pieces` named and others are still
/// to build: not a failure, and the next run finishes it.
pub const PARTIAL: &str =
    "The chosen pieces are built; the others wait for the next run of this job.";

/// One writer at a time to the manifest, which pieces finish into side by side.
static MANIFEST: Mutex<()> = Mutex::new(());

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a finished piece reported.
#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PieceResult {
    pub peak_rss_mb: Option<u64>,
    pub wall_s: Option<f64>,
    pub chunks: u64,
}

/// One job's folder and the ids it reserved, kept across a resume.
struct Job {
    name: String,
    dir: PathBuf,
    area_base: u32,
    map_base: i32,
}

impl Job {
    fn open(world_dir: &Path, rect: &XZBBox, n: i32, pieces: usize) -> Result<Self, String> {
        let name = format!(
            "{}_{}_{}_{}_n{n}",
            rect.min_x(),
            rect.min_z(),
            rect.max_x(),
            rect.max_z()
        );
        let dir = world_dir.join(one_world::JOBS_DIR).join(&name);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;
        let state = dir.join("job.json");
        let kept = std::fs::read_to_string(&state)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok());
        let (area_base, map_base) = match kept
            .as_ref()
            .map(|v| (v["area_base"].as_u64(), v["map_base"].as_i64()))
        {
            Some((Some(a), Some(m))) => (a as u32, m as i32),
            _ => {
                // Reserved once, so a run in between cannot take a piece's ids.
                let _guard = lock(&MANIFEST);
                let mut manifest = Manifest::load(world_dir)?.ok_or("The One World vanished.")?;
                let area_base = manifest.next_area_id;
                manifest.next_area_id += pieces as u32;
                manifest.save(world_dir)?;
                let maps = crate::world_utils::WorldLayout::of(world_dir).maps_dir(world_dir);
                let map_base = crate::map_item::next_map_id(&maps);
                if i64::from(map_base) + pieces as i64 * i64::from(MAP_IDS_PER_PIECE)
                    > i64::from(i32::MAX)
                {
                    return Err(format!(
                        "{pieces} pieces are too many for the map id space; use a larger --unit-regions."
                    ));
                }
                write(
                    &state,
                    &json!({ "area_base": area_base, "map_base": map_base }),
                )?;
                (area_base, map_base)
            }
        };
        Ok(Self {
            name,
            dir,
            area_base,
            map_base,
        })
    }

    fn done_path(&self, piece: usize) -> PathBuf {
        self.dir.join(format!("done-{piece}.json"))
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

/// Builds the selection `args` was opened on, piece by piece. Each piece runs this
/// command line again with its own bbox and lease.
pub fn run(args: &Args, world_dir: &Path) -> Result<(), String> {
    let ctx = args
        .one_world_run
        .as_ref()
        .ok_or("--unit-regions needs --one-world")?;
    let selection = args.bbox.ok_or("A job built in pieces needs a bbox.")?;
    let n = args.units.unit_regions.unwrap_or(DEFAULT_UNIT_REGIONS);
    let manifest = Manifest::load(world_dir)?
        .ok_or_else(|| format!("{} is not a One World.", world_dir.display()))?;
    let proj = manifest.projection();
    let (rect, _) = crate::projection::snap_bbox_to_chunks(&proj, &selection)?;
    let units = plan_units(&proj, &rect, n)?;
    let of = units.len();
    // `--only-pieces`: these are built, the job's other missing pieces wait.
    let only = args.units.only_pieces.as_deref();
    if let Some(&bad) = only.into_iter().flatten().find(|&&p| p as usize > of) {
        return Err(format!("--only-pieces {bad}: this job has {of} pieces."));
    }
    let wanted = |i: usize| only.is_none_or(|p| p.contains(&(i as u32 + 1)));
    let job = Job::open(world_dir, &rect, n, of)?;

    println!("One World: building {of} piece(s) of up to {n}x{n} regions");
    // The extract is read once, here; each piece then cuts its own area from the bake.
    if let Some(src) = crate::osm_pbf::Source::from_args(args).filter(|_| !args.skip_osm()) {
        crate::osm_pbf::bake_for_job(&src, selection)?;
    }
    let results: Mutex<Vec<Option<PieceResult>>> = Mutex::new(vec![None; of]);
    let mut queue = VecDeque::new();
    let piece_record = |i: usize, state: &str, extra: Value| {
        let mut body = json!({"piece": i, "of": of, "state": state,
                              "rect": corners(&units[i].rect), "bounds": bounds(&units[i])});
        if let (Value::Object(body), Value::Object(extra)) = (&mut body, extra) {
            body.extend(extra);
        }
        progress_json::record("piece", body);
    };
    for unit in &units {
        let i = unit.index as usize;
        match job.finished(i) {
            Some(r) => {
                println!("  piece {}/{of}: already built", i + 1);
                piece_record(i, "skipped", json!({}));
                lock(&results)[i] = Some(r);
            }
            None if wanted(i) => queue.push_back(i),
            None => {}
        }
    }

    // A fresh world gets its map item, settings and spawn from the first area built, so
    // one piece goes first and alone: the one holding the requested spawn, else the first.
    let head = (!ctx.extending && !queue.is_empty())
        .then(|| {
            user_spawn_xz(args, &selection)
                .and_then(|(x, z)| queue.iter().position(|&i| contains(&units[i].rect, x, z)))
                .unwrap_or(0)
        })
        .and_then(|at| queue.remove(at));
    let spawn_piece = head.or_else(|| {
        user_spawn_xz(args, &selection)
            .and_then(|(x, z)| units.iter().position(|u| contains(&u.rect, x, z)))
    });

    let largest = units.iter().map(|u| u.regions).max().unwrap_or(1);
    let sizing = budget::sizing(args, queue.len() + usize::from(head.is_some()), largest);
    if sizing.workers > 1 {
        println!(
            "  {} pieces at a time, {} threads each",
            sizing.workers, sizing.threads
        );
    }
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    let lease_for = |i: usize| UnitLease {
        piece: i as u32,
        area_id: job.area_base + i as u32,
        extending: head != Some(i),
        first_map_id: job.map_base + i as i32 * MAP_IDS_PER_PIECE,
        job: corners(&rect),
        tiled: crate::tile::create_tiles(&rect, crate::tile::DEFAULT_TILE_SIZE).len() >= 3,
    };

    // `--prewarm-first`: one piece at a time, each with the job's whole download allowance, so N
    // workers then read the caches instead of all fetching at once. `--prewarm` stops after it.
    if args.prewarm || args.prewarm_first {
        let warm: Vec<usize> = (0..of)
            .filter(|&i| wanted(i) && job.finished(i).is_none())
            .collect();
        for (k, &i) in warm.iter().enumerate() {
            if STOP.load(Ordering::Acquire) {
                return Err(STOPPED.to_string());
            }
            let lease = job.dir.join(format!("piece-{i}.lease.json"));
            write(
                &lease,
                &serde_json::to_value(lease_for(i)).map_err(|e| e.to_string())?,
            )?;
            let mut warm_argv = child_args(argv.iter().cloned(), &units[i], &lease, false);
            let downloads = args.max_downloads.unwrap_or(16).to_string();
            warm_argv.extend([
                "--prewarm".into(),
                "--max-downloads".into(),
                downloads.into(),
            ]);
            println!("  piece {}/{of}: warming the caches", i + 1);
            // The job's bar: the pieces warmed so far plus this one's share.
            let report = |f: f64| {
                let pct = (k as f64 + f) / warm.len() as f64 * 100.0;
                crate::progress::emit_gui_progress_update(pct, "");
            };
            run_piece_until(&warm_argv, &[], &STOP, report).map_err(|f| {
                if STOP.load(Ordering::Acquire) {
                    STOPPED.to_string()
                } else {
                    format!("warming piece {} failed: {}", i + 1, f.message)
                }
            })?;
        }
        if args.prewarm {
            // Only the leases it wrote: a job already under way keeps its resume records.
            for &i in &warm {
                let _ = std::fs::remove_file(job.dir.join(format!("piece-{i}.lease.json")));
            }
            let _ = std::fs::remove_dir(&job.dir);
            let _ = std::fs::remove_dir(world_dir.join(one_world::JOBS_DIR));
            return Ok(());
        }
    }

    // Each piece's fraction done, and the highest job percentage sent.
    let done: Mutex<(Vec<f64>, f64)> = Mutex::new((
        lock(&results)
            .iter()
            .map(|r| if r.is_some() { 1.0 } else { 0.0 })
            .collect(),
        0.0,
    ));
    let finished = AtomicUsize::new(lock(&results).iter().flatten().count());
    let status = || {
        if !crate::progress::is_running_with_gui() {
            // `--progress json` has its piece records, so its stream stays as it was.
            return String::new();
        }
        let line = format!(
            "Building pieces... {}/{of} done",
            finished.load(Ordering::Relaxed)
        );
        if PAUSE.load(Ordering::Acquire) {
            return line + ". Paused.";
        }
        line
    };
    // Piece `i` is `f` done: moves the job's bar.
    let report = |i: usize, f: f64| {
        let mut d = lock(&done);
        d.0[i] = f;
        // A retried piece starts over; the bar waits for it instead.
        d.1 = d.1.max(job_percent(&d.0));
        let percent = d.1;
        drop(d);
        crate::progress::emit_gui_progress_update(percent, &status());
    };
    let running = Mutex::new(0usize);
    let cap = AtomicUsize::new(sizing.workers);
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let aborting = AtomicBool::new(false);

    // Builds piece `i`: Ok(None) when Stop ended it.
    let build = |i: usize| -> Result<Option<PieceResult>, String> {
        let unit = &units[i];
        let lease = job.dir.join(format!("piece-{i}.lease.json"));
        write(
            &lease,
            &serde_json::to_value(lease_for(i)).map_err(|e| e.to_string())?,
        )?;
        let argv = sizing.child_args(child_args(
            argv.iter().cloned(),
            unit,
            &lease,
            spawn_piece == Some(i),
        ));
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
            piece_record(i, if attempt > 0 { "retry" } else { "start" }, json!({}));
            match run_piece_until(&argv, &sizing.child_env(), &STOP, |f| report(i, f)) {
                Ok(r) => break r,
                // Killed by Stop: not a failure, and built again on resume.
                Err(_) if STOP.load(Ordering::Acquire) => {
                    piece_record(i, "stopped", json!({}));
                    return Ok(None);
                }
                // Never once the job is aborting: a piece killed with it looks like a crash.
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
        // Recorded here, not by the piece, so the manifest has one writer.
        {
            let _guard = lock(&MANIFEST);
            let area_id = job.area_base + i as u32;
            let run = RunContext {
                area_id,
                piece: Some(PieceRef {
                    job: job.name.clone(),
                    index: i as u32,
                }),
                ..ctx.clone()
            };
            let preview = run.preview_path();
            one_world::record_area(
                &run,
                &unit.bbox,
                &unit.rect,
                None,
                preview.is_file().then_some(preview.as_path()),
            )
            .map_err(|e| format!("The area was written but could not be recorded: {e}"))?;
        }
        write(&job.done_path(i), &json!(r))?;
        crate::keep_one_world();
        let pieces_done = finished.fetch_add(1, Ordering::Relaxed) + 1;
        piece_record(
            i,
            "done",
            json!({"peak_rss_mb": r.peak_rss_mb, "wall_s": r.wall_s, "pieces_done": pieces_done}),
        );
        progress_json::CHUNKS_WRITTEN.fetch_add(r.chunks, Ordering::Relaxed);
        report(i, 1.0);
        lock(&results)[i] = Some(r.clone());
        Ok(Some(r))
    };

    if let Some(i) = head {
        build(i).map_err(|e| format!("{e}\n{RESUME}"))?;
    }
    let queue = Mutex::new(queue);
    let work = || -> Result<(), String> {
        while !aborting.load(Ordering::Relaxed) && !STOP.load(Ordering::Acquire) {
            // Paused: the queue waits while the running pieces finish.
            if PAUSE.load(Ordering::Acquire) && !lock(&queue).is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
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
            let built = build(i);
            *lock(&running) -= 1;
            // `auto` lowers the cap once a finished piece shows pieces need more memory.
            if let Ok(Some(PieceResult {
                peak_rss_mb: Some(peak),
                ..
            })) = &built
            {
                let allowed = sizing.cap_for(*peak);
                if cap.fetch_min(allowed, Ordering::Relaxed) > allowed {
                    println!("  pieces peak at {peak} MB: {allowed} at a time from now on");
                }
            }
            if built?.is_none() {
                return Ok(());
            }
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
        return Err(format!("{e}\n{RESUME}"));
    }
    // Stopped before the last piece, or only some retried: kept for a resume.
    if finished.load(Ordering::Relaxed) < of {
        let partial = only.is_some() && !STOP.load(Ordering::Acquire);
        return Err(if partial { PARTIAL } else { STOPPED }.to_string());
    }

    // The pieces' map ids were their own; the counter now covers all of them.
    if let Err(e) = crate::map_item::sync_map_counter(world_dir) {
        eprintln!("Warning: Failed to update the map id counter: {e}");
    }
    if let Err(e) = crate::world_utils::touch_last_played(world_dir) {
        eprintln!("Warning: Failed to update LastPlayed: {e}");
    }
    // The job is built; its state is not needed any more.
    let _ = std::fs::remove_dir_all(&job.dir);
    let _ = std::fs::remove_dir(world_dir.join(one_world::JOBS_DIR));
    Ok(())
}

/// `[min_x, min_z, max_x, max_z]`.
fn corners(r: &XZBBox) -> [i32; 4] {
    [r.min_x(), r.min_z(), r.max_x(), r.max_z()]
}

/// The piece's area on the map: `[min_lat, min_lng, max_lat, max_lng]`.
fn bounds(u: &WorkUnit) -> [f64; 4] {
    let (lo, hi) = (u.bbox.min(), u.bbox.max());
    [lo.lat(), lo.lng(), hi.lat(), hi.lng()]
}

fn contains(r: &XZBBox, x: i32, z: i32) -> bool {
    r.contains(&crate::coordinate_system::cartesian::XZPoint::new(x, z))
}

const RESUME: &str = "Finished pieces are kept; run the same command again to resume.";

/// `--spawn-lat/--spawn-lng` in the world's blocks, as a run converts them.
fn user_spawn_xz(
    args: &Args,
    selection: &crate::coordinate_system::geographic::LLBBox,
) -> Option<(i32, i32)> {
    let point =
        crate::coordinate_system::geographic::LLPoint::new(args.spawn_lat?, args.spawn_lng?)
            .ok()?;
    let (transformer, _) = crate::projection::ProjectionSpec::from_args(args)
        .transformer(selection)
        .ok()?;
    let p = transformer.transform_point(point);
    Some((p.x, p.z))
}

/// Options the coordinator decides per piece, so the user's are dropped.
const PER_PIECE: &[&str] = &[
    "--bbox",
    "--unit-regions",
    "--one-world-workers",
    "--plan-units",
    "--progress",
    "--threads",
    "--cpu-target",
    "--ram-budget-mb",
    "--max-downloads",
    "--one-world-unit",
    "--only-pieces",
];

/// Switches the coordinator decides: it hands `--prewarm` to the pieces it warms, and clap
/// refuses one given twice. They take no value, so the next argument is kept.
const PER_PIECE_SWITCHES: &[&str] = &["--prewarm", "--prewarm-first"];

/// Only the piece the world spawns in gets the spawn point: the others do not contain it,
/// and a run refuses a spawn outside its area.
const SPAWN: &[&str] = &["--spawn-lat", "--spawn-lng"];

/// A piece's command line: the job's own, minus what is decided per piece, plus the
/// piece's bbox at full precision and its lease.
fn child_args(
    argv: impl Iterator<Item = OsString>,
    unit: &WorkUnit,
    lease: &Path,
    keep_spawn: bool,
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
        if PER_PIECE.contains(&name) || (!keep_spawn && SPAWN.contains(&name)) {
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
struct PieceFailure {
    message: String,
    transient: bool,
}

/// Network trouble, or a piece that died without saying why (killed, out of memory), may
/// pass; a reported error, a panic or a stop will not.
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

/// Runs one piece to the end, or kills it as soon as `cancel` is set. `progress` gets the
/// piece's own fraction done.
fn run_piece_until(
    argv: &[OsString],
    env: &[(&str, String)],
    cancel: &AtomicBool,
    mut progress: impl FnMut(f64),
) -> Result<PieceResult, PieceFailure> {
    let fail = |message: String| PieceFailure {
        message,
        transient: false,
    };
    let exe =
        std::env::current_exe().map_err(|e| fail(format!("Cannot find this executable: {e}")))?;
    let mut cmd = Command::new(exe);
    // Every piece skips the update check and the cache sweep; through the environment, so
    // a user's own switch for either does not clash.
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
                let mut t = lock(&tail);
                if t.len() == TAIL_LINES {
                    t.pop_front();
                }
                t.push_back(String::from_utf8_lossy(&line).trim_end().to_string());
            }
        })
    });
    let mut result = PieceResult::default();
    let stdout = child.stdout.take();
    let child = Mutex::new(child);
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        // Killing the child closes its pipes, which ends the read below.
        scope.spawn(|| {
            while !finished.load(Ordering::Acquire) {
                if cancel.load(Ordering::Acquire) {
                    let _ = lock(&child).kill();
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });
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
                Some("done") => {
                    result.peak_rss_mb = record["peak_rss_mb"].as_u64();
                    result.wall_s = record["wall_s"].as_f64();
                    result.chunks = record["chunks"].as_u64().unwrap_or(0);
                }
                Some("error") => push(&tail, line),
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

/// The job's percentage: every piece is an equal share, so it reads n/N as "n/N done"
/// does, plus the running pieces' own fractions.
fn job_percent(done: &[f64]) -> f64 {
    done.iter().map(|f| f.clamp(0.0, 1.0)).sum::<f64>() / done.len().max(1) as f64 * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coordinate_system::geographic::LLBBox;

    #[test]
    fn live_with_workers_is_refused() {
        let argv = ["arnis", "--live", "--one-world", "--one-world-workers", "4"];
        // `--live` is upstream's Live mode, which this branch does not have yet; until the
        // rebase onto it adds the flag, clap itself refuses it by name.
        match <Args as clap::Parser>::try_parse_from(argv) {
            Err(e) => assert!(e.to_string().contains("--live"), "{e}"),
            Ok(args) => {
                let err = crate::args::validate_args(&args).unwrap_err();
                assert!(err.contains("--live"), "{err}");
            }
        }
    }

    #[test]
    fn the_job_percentage_follows_the_pieces_done() {
        // 2 of 16 done, two running at a half and a quarter.
        let mut d = vec![0.0; 16];
        d[0] = 1.0;
        d[1] = 1.0;
        d[2] = 0.5;
        d[3] = 0.25;
        assert!((job_percent(&d) - 2.75 / 16.0 * 100.0).abs() < 1e-9);
        assert_eq!(job_percent(&[1.0; 16]), 100.0);
        assert_eq!(job_percent(&[]), 0.0);
    }

    #[test]
    fn a_piece_gets_the_jobs_options_but_its_own_bbox_and_lease() {
        let proj = crate::projection::WebMercatorProjection::new(44.4465, 26.099, 1.0);
        let req = LLBBox::new(44.4375, 26.0865, 44.4555, 26.1115).unwrap();
        let (world, _) = crate::projection::snap_bbox_to_chunks(&proj, &req).unwrap();
        let units = plan_units(&proj, &world, 1).unwrap();
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
            "--only-pieces",
            "2,3",
            "--scale=1",
        ]
        .map(OsString::from);
        let got = |keep| {
            child_args(argv.iter().cloned(), &units[3], Path::new("l.json"), keep)
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        let got_without_spawn = got(false);
        assert_eq!(
            got_without_spawn,
            [
                "--one-world",
                "--output-dir",
                "out dir",
                "--no-update-check",
                "--scale=1",
                "--bbox",
                &units[3].bbox_arg(),
                "--one-world-unit",
                "l.json",
                "--progress",
                "json",
            ]
        );
        assert!(got(true).contains(&"--spawn-lat".to_string()));
        // What the piece parses is the bbox that snaps to its rectangle.
        let parsed = LLBBox::from_str(&got_without_spawn[6]).unwrap();
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

    /// Builds the parity area as a job, kills one piece, resumes, and compares with an
    /// uninterrupted job. Needs network, python with nbtlib, the arnis binary
    /// (`ARNIS_BIN`) and chunkdiff.py (`CHUNKDIFF`):
    /// `cargo test --release scale::tests::resume -- --ignored`
    #[test]
    #[ignore]
    fn resume_after_kill_matches_full_run() {
        let bin = std::env::var("ARNIS_BIN").expect("set ARNIS_BIN to the arnis executable");
        let chunkdiff = std::env::var("CHUNKDIFF").expect("set CHUNKDIFF to chunkdiff.py");
        let tmp = std::env::temp_dir().join("arnis-resume-test");
        let _ = std::fs::remove_dir_all(&tmp);
        let (full, resumed) = (tmp.join("full"), tmp.join("resumed"));
        let arnis = |out: &Path, bbox: &str, extra: &[&str], env: &[(&str, &Path)]| {
            Command::new(&bin)
                .args(["--one-world", "--world-name", "W", "--output-dir"])
                .arg(out)
                .args(["--bbox", bbox])
                .args(extra)
                .envs(env.iter().copied())
                .output()
                .expect("run arnis")
        };
        let job = ["--one-world-workers", "2", "--unit-regions", "1"];
        let area = "44.4450,26.0950,44.4480,26.1030";
        for w in [&full, &resumed] {
            // The seed fixes the frame, as in the parity runs.
            assert!(arnis(w, "44.4464,26.0988,44.4466,26.0992", &[], &[])
                .status
                .success());
        }
        assert!(arnis(&full, area, &job, &[]).status.success());
        // Piece 2 dies with the panic code once, as it starts; piece 1 builds alone first.
        let fail = tmp.join("fail");
        std::fs::write(&fail, "2 0").unwrap();
        let first = arnis(&resumed, area, &job, &[("ARNIS_TEST_FAIL_PIECE", &fail)]);
        assert!(
            !first.status.success(),
            "the killed piece must fail the job"
        );
        assert!(resumed.join("W/arnis_one_world/jobs").is_dir());
        assert!(arnis(&resumed, area, &job, &[]).status.success());
        let out = Command::new("python")
            .arg(&chunkdiff)
            .arg(full.join("W"))
            .arg(resumed.join("W"))
            .output()
            .expect("run chunkdiff");
        let text = String::from_utf8_lossy(&out.stdout);
        // Every chunk equal once the volatile LastUpdate and InhabitedTime are dropped.
        let number = |after: &str| -> u64 {
            let rest = text.split(after).nth(1).unwrap_or_else(|| panic!("{text}"));
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().unwrap_or_else(|_| panic!("{text}"))
        };
        assert!(text.contains("only_A=0 only_B=0"), "{text}");
        assert_eq!(number("chunks A="), number("'LastUpdate']: "), "{text}");
    }
}
