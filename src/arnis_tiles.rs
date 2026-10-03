//! Local Archive: runs louis-e/arnis-tiles to bake Geofabrik country extracts
//! into a local tile archive folder that `--osm-tiles-url <folder>` reads.
//!
//! arnis-tiles is a separate program: the release bundles it as a Tauri
//! sidecar next to the Arnis executable, and a dev build finds it on PATH or
//! at the path set in the window.

use std::ffi::OsStr;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const HOME: &str = "https://github.com/louis-e/arnis-tiles";

const EXE: &str = if cfg!(windows) {
    "arnis-tiles.exe"
} else {
    "arnis-tiles"
};

/// The arnis-tiles executable: next to this one (where the bundle puts the
/// sidecar), then on PATH, then `setting` (a file, or a folder holding it).
pub fn locate(setting: &str) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    locate_in(
        exe_dir.as_deref(),
        std::env::var_os("PATH").as_deref(),
        setting,
    )
}

fn locate_in(exe_dir: Option<&Path>, path: Option<&OsStr>, setting: &str) -> Option<PathBuf> {
    let setting = PathBuf::from(setting.trim());
    let set = (!setting.as_os_str().is_empty()).then(|| {
        if setting.is_dir() {
            setting.join(EXE)
        } else {
            setting
        }
    });
    exe_dir
        .map(|d| d.join(EXE))
        .into_iter()
        .chain(
            path.into_iter()
                .flat_map(std::env::split_paths)
                .map(|d| d.join(EXE)),
        )
        .chain(set)
        .find(|p| p.is_file())
}

/// The default archive folder under the cache root.
pub fn default_folder(root: &Path) -> PathBuf {
    root.join("arnis").join("local-archive")
}

/// One Geofabrik extract `prepare` picked for the selection.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
pub struct Extract {
    pub id: String,
    pub name: String,
    /// Download size of the .osm.pbf.
    pub bytes: u64,
    pub url: String,
}

/// The JSON line `arnis-tiles prepare` prints before it downloads anything.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
pub struct Prepare {
    pub extracts: Vec<Extract>,
    pub total_bytes: u64,
    /// Sample points of the selection no extract covers (open sea).
    pub uncovered_points: u64,
}

/// The plan out of `prepare`'s output: its last line that parses as one.
pub fn parse_prepare(stdout: &str) -> Option<Prepare> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .filter(|l| l.starts_with('{'))
        .find_map(|l| serde_json::from_str(l).ok())
}

/// `--bbox` as arnis-tiles takes it: min_lat,min_lon,max_lat,max_lon.
fn bbox_arg(bbox: &crate::coordinate_system::geographic::LLBBox) -> String {
    let (lo, hi) = (bbox.min(), bbox.max());
    format!("{},{},{},{}", lo.lat(), lo.lng(), hi.lat(), hi.lng())
}

/// `exe` with its index cache and scratch kept under `state`, never in the
/// working directory.
fn command(exe: &Path, state: &Path, out: &Path) -> Result<Command, String> {
    std::fs::create_dir_all(state).map_err(|e| format!("{}: {e}", state.display()))?;
    let mut cmd = Command::new(exe);
    cmd.current_dir(state)
        .arg("--cache")
        .arg(state.join("cache"))
        .arg("--work")
        .arg(state.join("work"))
        .arg("--out")
        .arg(out);
    crate::scale::child::prepare(&mut cmd);
    Ok(cmd)
}

/// `prepare --dry-run`: the extracts that cover `bbox` and their sizes. The
/// first call downloads the Geofabrik index into `state`; later ones read it.
pub fn dry_run(
    exe: &Path,
    state: &Path,
    bbox: &crate::coordinate_system::geographic::LLBBox,
) -> Result<Prepare, String> {
    let mut cmd = command(exe, state, &state.join("out"))?;
    cmd.args(["prepare", "--dry-run", "--bbox", &bbox_arg(bbox)]);
    let out = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    // With no extract covering the area it prints the empty plan and fails.
    parse_prepare(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| {
        let err = String::from_utf8_lossy(&out.stderr);
        err.lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("arnis-tiles printed no plan")
            .trim()
            .to_string()
    })
}

/// Where a bake is, read off arnis-tiles' output lines.
#[derive(Debug, Default, PartialEq)]
pub struct BakeProgress {
    /// Extract being worked on, 0-based, of `total`.
    index: usize,
    total: usize,
    id: String,
    stage: Stage,
}

#[derive(Debug, Default, PartialEq)]
enum Stage {
    #[default]
    Planning,
    /// Download percentage.
    Downloading(f64),
    /// osmpbf passes; arnis-tiles reports nothing until they end.
    Baking,
    Writing,
}

impl BakeProgress {
    /// Takes one output line (stdout or stderr).
    pub fn line(&mut self, line: &str) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('[') {
            // `[2/3] romania (330 MB)` or `[2/3] romania already baked, skipping`
            let Some((count, rest)) = rest.split_once(']') else {
                return;
            };
            let Some((i, n)) = count.split_once('/') else {
                return;
            };
            let (Ok(i), Ok(n)) = (i.parse::<usize>(), n.parse::<usize>()) else {
                return;
            };
            self.index = i.saturating_sub(1);
            self.total = n;
            self.id = rest.split_whitespace().next().unwrap_or("").to_string();
            self.stage = if rest.contains("already baked") {
                Stage::Writing
            } else {
                Stage::Downloading(0.0)
            };
        } else if let Some(rest) = line.strip_prefix("downloaded ") {
            // `downloaded 120 MB (36%)` while it runs, without a share at the end.
            self.stage = match rest
                .split_once('(')
                .and_then(|(_, p)| p.split_once('%'))
                .and_then(|(p, _)| p.trim().parse::<f64>().ok())
            {
                Some(p) => Stage::Downloading(p.clamp(0.0, 100.0)),
                None => Stage::Baking,
            };
        } else if line.contains(" ways, ") && line.contains(" tiles, ") {
            self.stage = Stage::Writing;
        }
    }

    /// 0-100 across all extracts: per extract, the download is the first 40 %,
    /// the bake runs to 85 % and writing the archive takes the rest.
    pub fn percent(&self) -> f64 {
        let within = match self.stage {
            Stage::Planning => return 0.0,
            Stage::Downloading(p) => 0.4 * p,
            Stage::Baking => 40.0,
            Stage::Writing => 85.0,
        };
        (self.index as f64 * 100.0 + within) / self.total.max(1) as f64
    }

    /// One status line, without the elapsed time.
    pub fn message(&self) -> String {
        let of = format!("{}/{}", self.index + 1, self.total.max(1));
        match self.stage {
            Stage::Planning => "Choosing the extracts to bake...".to_string(),
            Stage::Downloading(p) => format!("Downloading {} ({of}): {p:.0}%", self.id),
            Stage::Baking => format!("Baking {} ({of})...", self.id),
            Stage::Writing => format!("Writing the {} archive ({of})...", self.id),
        }
    }
}

fn clock(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

/// `prepare` for `bbox` into `folder` on `threads` threads, reporting
/// `(percent, message)` to `report` about once a second. Stops the child when
/// `cancel` is set. `Ok(false)` when cancelled; what was baked is kept, and
/// arnis-tiles resumes from it.
pub fn bake(
    exe: &Path,
    state: &Path,
    folder: &Path,
    bbox: &crate::coordinate_system::geographic::LLBBox,
    threads: usize,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(f64, &str),
) -> Result<bool, String> {
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let mut cmd = command(exe, state, folder)?;
    cmd.args(["prepare", "--bbox", &bbox_arg(bbox), "--threads"])
        .arg(threads.max(1).to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", exe.display()))?;
    // Killed with this process, however it ends.
    if let Err(e) = crate::scale::child::adopt(&child) {
        let _ = child.kill();
        return Err(e);
    }
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let out = child.stdout.take().expect("piped stdout");
    let err = child.stderr.take().expect("piped stderr");
    let tx2 = tx.clone();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    // The download line is redrawn with \r, so stderr splits on both.
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        for b in std::io::BufReader::new(err).bytes().map_while(Result::ok) {
            if b == b'\r' || b == b'\n' {
                if !buf.is_empty()
                    && tx2
                        .send(String::from_utf8_lossy(&buf).into_owned())
                        .is_err()
                {
                    break;
                }
                buf.clear();
            } else {
                buf.push(b);
            }
        }
    });

    let start = Instant::now();
    let mut progress = BakeProgress::default();
    let mut tail: Vec<String> = Vec::new();
    let mut last_report = start.checked_sub(Duration::from_secs(1)).unwrap_or(start);
    loop {
        if cancel.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(false);
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(line) => {
                progress.line(&line);
                if !line.trim().is_empty() {
                    tail.push(line);
                    if tail.len() > 8 {
                        tail.remove(0);
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            // Both pipes closed: the child is ending.
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let status = child.wait().map_err(|e| e.to_string())?;
                if status.success() {
                    return Ok(true);
                }
                let last = tail
                    .iter()
                    .rev()
                    .find(|l| l.starts_with("error:"))
                    .or(tail.last())
                    .map_or("no output", |l| l.trim());
                return Err(format!("arnis-tiles failed ({status}): {last}"));
            }
        }
        if last_report.elapsed() >= Duration::from_secs(1) {
            last_report = Instant::now();
            let msg = format!("{} {}", progress.message(), clock(start.elapsed()));
            report(progress.percent(), &msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dry_run_json_line_is_found_among_the_others() {
        let out = "  romania                         329.5 MB  Romania\n\
                   1 extract(s), 329.5 MB to download\n\
                   {\"bbox\":[44.445,26.095,44.448,26.103],\"extracts\":[{\"bytes\":329538806,\"id\":\"romania\",\"name\":\"Romania\",\"url\":\"https://download.geofabrik.de/europe/romania-latest.osm.pbf\"}],\"total_bytes\":329538806,\"uncovered_points\":0}\n";
        let p = parse_prepare(out).unwrap();
        assert_eq!(p.total_bytes, 329_538_806);
        assert_eq!(p.uncovered_points, 0);
        assert_eq!(
            p.extracts,
            vec![Extract {
                id: "romania".into(),
                name: "Romania".into(),
                bytes: 329_538_806,
                url: "https://download.geofabrik.de/europe/romania-latest.osm.pbf".into(),
            }]
        );
        // No extract covers open sea: the plan is empty, not missing.
        let sea = parse_prepare(
            "0 extract(s), 0.0 MB to download\n{\"bbox\":[0,0,1,1],\"extracts\":[],\"total_bytes\":0,\"uncovered_points\":1089}",
        )
        .unwrap();
        assert!(sea.extracts.is_empty());
        assert!(
            parse_prepare("fetching https://download.geofabrik.de/index-v1.json\n{oops").is_none()
        );
    }

    #[test]
    fn lookup_goes_next_to_the_exe_then_path_then_the_setting() {
        let root = tempfile::tempdir().unwrap();
        let dir = |n: &str| {
            let d = root.path().join(n);
            std::fs::create_dir_all(&d).unwrap();
            d
        };
        let (exe_dir, on_path, set) = (dir("exe"), dir("path"), dir("set"));
        let path = std::env::join_paths([dir("empty"), on_path.clone()]).unwrap();
        let find = |s: &str| locate_in(Some(&exe_dir), Some(&path), s);

        assert_eq!(find(""), None);
        std::fs::write(set.join(EXE), b"").unwrap();
        // The setting takes a file or the folder holding it.
        assert_eq!(find(set.join(EXE).to_str().unwrap()), Some(set.join(EXE)));
        assert_eq!(find(set.to_str().unwrap()), Some(set.join(EXE)));
        std::fs::write(on_path.join(EXE), b"").unwrap();
        assert_eq!(find(set.to_str().unwrap()), Some(on_path.join(EXE)));
        std::fs::write(exe_dir.join(EXE), b"").unwrap();
        assert_eq!(find(set.to_str().unwrap()), Some(exe_dir.join(EXE)));
    }

    #[test]
    fn progress_follows_the_output_lines() {
        let mut p = BakeProgress::default();
        assert_eq!(p.percent(), 0.0);
        p.line("[1/2] liechtenstein (3 MB)");
        assert_eq!(p.message(), "Downloading liechtenstein (1/2): 0%");
        p.line("    downloaded 2 MB (50%)    ");
        assert_eq!(p.percent(), 10.0);
        p.line("    downloaded 3 MB          ");
        assert_eq!(
            (p.percent(), p.message().as_str()),
            (20.0, "Baking liechtenstein (1/2)...")
        );
        p.line("    120 ways, 30 pois, 4 relations -> 90 tiles, 1 MB in 2s");
        assert_eq!(p.percent(), 42.5);
        p.line("[2/2] austria already baked, skipping");
        assert_eq!(p.percent(), 92.5);
        assert_eq!(p.message(), "Writing the austria archive (2/2)...");
        p.line("archive ready: arnis --osm-tiles-url out ...");
        assert_eq!(p.percent(), 92.5, "an unknown line changes nothing");
    }
}
