//! How many pieces of a job run at once, and what each may use.
//!
//! A fixed count (`--one-world-workers N`) splits what one run would take:
//! `--threads`/`--cpu-target` else 90% of the cores, `--ram-budget-mb` else
//! free memory, `--max-downloads` else 16.
//!
//! `auto` sizes from the machine:
//! - budget = cores x CPU target (`--cpu-target`, default 75%), or `--threads`;
//! - W = clamp(min(pieces, budget / MIN_THREADS, (free - reserve) / piece RAM), 1, MAX_WORKERS);
//! - T = budget / W.
//!
//! A few fat workers beat many thin ones (T3: 4 x 5 threads was 1.26x one
//! run on 8 x 8 km at scale 1, 16 x 1 was 0.83x), hence the thread floor and
//! the cap of 6. Piece RAM starts from `estimate_piece_mb` and, once pieces
//! finish, follows the largest peak they reported (`Sizing::cap_for`).

use crate::args::{Args, Workers};
use std::ffi::OsString;

/// Threads below which a piece is not worth its own process.
const MIN_THREADS: usize = 4;
pub const MAX_WORKERS: usize = 6;
/// CPU share `auto` plans for, leaving the machine usable.
const AUTO_CPU_TARGET: usize = 75;

/// Peak memory of one piece of `regions` regions before any has reported.
/// From T3 on upstream: a piece of 64 regions at scale 1 peaked near
/// 1.6 GB, a whole 64-region run at scale 0.05 at 0.9 GB.
pub fn estimate_piece_mb(regions: u64, scale: f64) -> u64 {
    let per_region = if scale >= 0.5 { 19 } else { 8 };
    400 + per_region * regions
}

/// Memory kept free for the system and everything else running.
pub fn reserve_mb(total_mb: u64) -> u64 {
    (total_mb / 10).max(2048)
}

/// W and T for `auto`.
pub fn auto_plan(
    budget_threads: usize,
    pieces: usize,
    usable_mb: u64,
    piece_mb: u64,
) -> (usize, usize) {
    let by_cpu = budget_threads / MIN_THREADS;
    let by_ram = (usable_mb / piece_mb.max(1)) as usize;
    let workers = pieces.min(by_cpu).min(by_ram).clamp(1, MAX_WORKERS);
    (workers, (budget_threads / workers).max(1))
}

#[derive(Debug, PartialEq)]
pub struct Sizing {
    pub workers: usize,
    pub threads: usize,
    pub ram_mb: Option<u64>,
    pub downloads: u32,
    /// Memory the running pieces may share, when `auto` watches it.
    pub usable_mb: Option<u64>,
}

impl Sizing {
    pub fn child_args(&self, mut argv: Vec<OsString>) -> Vec<OsString> {
        argv.extend([
            "--threads".into(),
            self.threads.to_string().into(),
            "--max-downloads".into(),
            self.downloads.to_string().into(),
        ]);
        if let Some(mb) = self.ram_mb {
            argv.extend(["--ram-budget-mb".into(), mb.to_string().into()]);
        }
        argv
    }

    pub fn child_env(&self) -> [(&'static str, String); 2] {
        [
            ("RAYON_NUM_THREADS", self.threads.to_string()),
            // The stock writer sizing, on this piece's share of the cores.
            (
                "ARNIS_FLUSH_THREADS",
                self.threads.div_ceil(4).clamp(1, 6).to_string(),
            ),
        ]
    }

    /// Pieces that may run together once a piece peaked at `peak_mb`.
    /// Only ever lowers the planned count.
    pub fn cap_for(&self, peak_mb: u64) -> usize {
        match self.usable_mb {
            Some(usable) => ((usable / peak_mb.max(1)) as usize).clamp(1, self.workers),
            None => self.workers,
        }
    }
}

/// Sizing for a job of `pieces` pieces, the largest `piece_regions` regions.
pub fn sizing(args: &Args, pieces: usize, piece_regions: u64) -> Sizing {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let downloads = |w: usize| (args.process.max_downloads.unwrap_or(16) / w as u32).max(2);
    match args.units.one_world_workers {
        Some(Workers::Auto) => {
            let budget = args
                .process
                .thread_count()
                .unwrap_or(cores * AUTO_CPU_TARGET / 100)
                .max(1);
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            let total = sys.total_memory() / (1024 * 1024);
            let free = args.process.ram_budget_mb.unwrap_or_else(|| {
                (sys.available_memory() / (1024 * 1024)).saturating_sub(reserve_mb(total))
            });
            let piece_mb = estimate_piece_mb(piece_regions, args.scale);
            let (workers, threads) = auto_plan(budget, pieces, free, piece_mb);
            Sizing {
                workers,
                threads,
                ram_mb: Some((free / workers as u64).max(1)),
                downloads: downloads(workers),
                usable_mb: Some(free),
            }
        }
        fixed => {
            let threads = args.process.thread_count().unwrap_or(cores * 9 / 10).max(1);
            let workers = match fixed {
                Some(Workers::Count(n)) => n as usize,
                _ => 1,
            }
            .clamp(1, pieces.max(1));
            let ram_mb = args
                .process
                .ram_budget_mb
                .or_else(|| (workers > 1).then(crate::data_processing::available_memory_mb));
            Sizing {
                workers,
                threads: (threads / workers).max(1),
                ram_mb: ram_mb.map(|mb| (mb / workers as u64).max(1)),
                downloads: downloads(workers),
                usable_mb: None,
            }
        }
    }
}

/// Threads a one-process job (a download-and-bake) runs on: what a job of
/// one piece gets, so it follows the same --threads / --cpu-target and the
/// same defaults as the generation workers.
pub fn bake_threads(args: &Args) -> usize {
    sizing(args, 1, 1).threads
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_prefers_a_few_fat_workers() {
        // 24 cores at 75%: 18 threads, at most 4 workers of 4+ threads.
        assert_eq!(auto_plan(18, 16, 30_000, 720), (4, 4));
        // Never more than six, however big the machine.
        assert_eq!(auto_plan(96, 100, 500_000, 720), (6, 16));
        // Never more than there are pieces.
        assert_eq!(auto_plan(18, 2, 30_000, 720), (2, 9));
        // Memory decides when it is short.
        assert_eq!(auto_plan(18, 16, 2_000, 720), (2, 9));
        // A small machine still runs one piece, with everything it has.
        assert_eq!(auto_plan(3, 16, 500, 720), (1, 3));
    }

    #[test]
    fn piece_memory_follows_its_size_and_scale() {
        assert_eq!(estimate_piece_mb(16, 1.0), 704);
        assert!(estimate_piece_mb(64, 1.0) > estimate_piece_mb(64, 0.05));
        assert_eq!(reserve_mb(8_000), 2048);
        assert_eq!(reserve_mb(64_000), 6400);
    }

    #[test]
    fn a_measured_peak_only_ever_lowers_the_count() {
        let s = Sizing {
            workers: 4,
            threads: 4,
            ram_mb: Some(4000),
            downloads: 4,
            usable_mb: Some(16_000),
        };
        assert_eq!(s.cap_for(3000), 4);
        assert_eq!(s.cap_for(6000), 2);
        assert_eq!(s.cap_for(40_000), 1);
        let fixed = Sizing {
            usable_mb: None,
            ..s
        };
        assert_eq!(fixed.cap_for(40_000), 4);
    }

    fn parse(extra: &[&str]) -> Args {
        let mut cmd = vec![
            "arnis",
            "--output-dir",
            ".",
            "--bbox",
            "1,2,3,4",
            "--one-world",
        ];
        cmd.extend_from_slice(extra);
        <Args as clap::Parser>::parse_from(cmd)
    }

    #[test]
    fn a_fixed_count_splits_the_jobs_budget() {
        let s = sizing(
            &parse(&[
                "--one-world-workers",
                "4",
                "--threads",
                "20",
                "--ram-budget-mb",
                "8000",
            ]),
            16,
            16,
        );
        assert_eq!(
            s,
            Sizing {
                workers: 4,
                threads: 5,
                ram_mb: Some(2000),
                downloads: 4,
                usable_mb: None,
            }
        );
        // Never more workers than pieces, never less than a thread each.
        let s = sizing(
            &parse(&["--one-world-workers", "8", "--threads", "3"]),
            2,
            16,
        );
        assert_eq!((s.workers, s.threads), (2, 1));
        // Sequential: the job's own knobs, nothing invented.
        let s = sizing(&parse(&["--unit-regions", "2", "--threads", "6"]), 9, 4);
        assert_eq!((s.workers, s.threads, s.ram_mb), (1, 6, None));
    }

    #[test]
    fn a_bake_runs_on_one_workers_share_of_the_cpu_setting() {
        let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
        assert_eq!(bake_threads(&parse(&["--threads", "6"])), 6);
        assert_eq!(
            bake_threads(&parse(&["--cpu-target", "50"])),
            (cores / 2).max(1)
        );
        // With no setting, a single run's 90 %, or the 75 % auto plans for.
        assert_eq!(bake_threads(&parse(&[])), (cores * 9 / 10).max(1));
        assert_eq!(
            bake_threads(&parse(&["--one-world-workers", "auto"])),
            (cores * 3 / 4).max(1)
        );
        // A fixed worker count does not split a one-process job.
        assert_eq!(
            bake_threads(&parse(&["--one-world-workers", "4", "--threads", "20"])),
            20
        );
    }

    #[test]
    fn auto_honours_an_explicit_thread_and_memory_budget() {
        let s = sizing(
            &parse(&[
                "--one-world-workers",
                "auto",
                "--threads",
                "16",
                "--ram-budget-mb",
                "6000",
            ]),
            16,
            16,
        );
        // 16 threads allow 4 workers, 6000 MB at ~704 MB a piece allow 8.
        assert_eq!((s.workers, s.threads, s.ram_mb), (4, 4, Some(1500)));
        assert_eq!(s.usable_mb, Some(6000));
    }
}
