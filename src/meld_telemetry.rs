//! Meld stdout protocol v1 — machine-readable phase markers for the Meld worker governor.
//!
//! Entirely opt-in: nothing is printed unless the environment variable
//! `ARNIS_PHASE_MARKERS` is set to exactly `1`. With it unset every entry point
//! returns before any formatting or clock read happens, so a default run is
//! byte-identical to one built without this module.
//!
//! Wire format (one line per marker, stdout, flushed):
//!
//! ```text
//! [meld] v=1 phase=<name> t=<ms_since_process_start>
//! [meld] v=1 phase=done wall_s=<f.3> cpu_s=<f.3> peak_mb=<f.1> gpu_ms=<u64>
//! ```
//!
//! Phase markers are emitted at the START of each phase, so a consumer derives a
//! phase's duration as `t(next) - t(this)` and the last phase's duration as
//! `wall_s*1000 - t(last)`.
//!
//! `cpu_s` / `peak_mb` are read straight from the Win32 process counters
//! (`GetProcessTimes` / `K32GetProcessMemoryInfo`, both exported by kernel32, so
//! no new crate dependency is introduced). On non-Windows targets they are
//! reported as `-1` and Meld's psutil-side sampling covers them instead.

use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

/// Process-start reference clock. Seeded by [`init`] from `main`.
static START: OnceLock<Instant> = OnceLock::new();
/// Cached `ARNIS_PHASE_MARKERS` lookup — the env var is read exactly once.
static ENABLED: OnceLock<bool> = OnceLock::new();

/// Seed the process-start clock (and the env lookup). Call as early as possible
/// in `main`; every other entry point falls back to lazy init if it is skipped.
pub fn init() {
    let _ = START.set(Instant::now());
    let _ = enabled();
}

#[inline]
fn start() -> Instant {
    *START.get_or_init(Instant::now)
}

/// True when `ARNIS_PHASE_MARKERS=1`. Read once, then a plain atomic load.
#[inline]
pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var_os("ARNIS_PHASE_MARKERS")
            .map(|v| v == *"1")
            .unwrap_or(false)
    })
}

/// Emit `[meld] v=1 phase=<name> t=<ms>`. No-op unless the protocol is enabled.
///
/// `name` must be one of the protocol's phase names: `fetch`, `parse`,
/// `overture`, `elevation`, `ground`, `place`, `post`, `merge`, `save`.
#[inline]
pub fn phase(name: &str) {
    if !enabled() {
        return;
    }
    emit_phase(name);
}

// Kept out of `phase` so the disabled path stays a load + branch with nothing to inline.
#[cold]
fn emit_phase(name: &str) {
    let t: u128 = start().elapsed().as_millis();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(out, "[meld] v=1 phase={name} t={t}");
    let _ = out.flush();
}

/// Emit the terminating `phase=done` line. No-op unless the protocol is enabled.
///
/// `gpu_ms` is the run's total GPU busy time (the same counter the `[gpu]
/// busy_ms=` line reports).
#[inline]
pub fn done(gpu_ms: u64) {
    if !enabled() {
        return;
    }
    emit_done(gpu_ms);
}

#[cold]
fn emit_done(gpu_ms: u64) {
    let wall_s: f64 = start().elapsed().as_secs_f64();
    let (cpu_s, peak_mb) = process_counters();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let _ = writeln!(
        out,
        "[meld] v=1 phase=done wall_s={wall_s:.3} cpu_s={cpu_s:.3} peak_mb={peak_mb:.1} gpu_ms={gpu_ms}"
    );
    let _ = out.flush();
}

/// `(cpu_seconds, peak_working_set_mb)` for this process, or `(-1.0, -1.0)`
/// where the platform counters are unavailable.
#[cfg(windows)]
fn process_counters() -> (f64, f64) {
    // SAFETY: both calls take a pseudo-handle that never needs closing and write
    // into stack buffers we own and size correctly.
    unsafe {
        let handle = win::GetCurrentProcess();

        let mut creation = win::Filetime::default();
        let mut exit = win::Filetime::default();
        let mut kernel = win::Filetime::default();
        let mut user = win::Filetime::default();
        let cpu_s = if win::GetProcessTimes(
            handle,
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        ) != 0
        {
            // FILETIME ticks are 100 ns.
            (kernel.as_ticks() + user.as_ticks()) as f64 / 1e7
        } else {
            -1.0
        };

        let mut counters = win::ProcessMemoryCounters {
            cb: std::mem::size_of::<win::ProcessMemoryCounters>() as u32,
            ..Default::default()
        };
        let peak_mb = if win::K32GetProcessMemoryInfo(handle, &mut counters, counters.cb) != 0 {
            counters.peak_working_set_size as f64 / (1024.0 * 1024.0)
        } else {
            -1.0
        };

        (cpu_s, peak_mb)
    }
}

#[cfg(not(windows))]
fn process_counters() -> (f64, f64) {
    // No new dependency is worth pulling in for the non-Windows path: Meld's
    // psutil-side sampling already covers CPU time and peak RSS there.
    (-1.0, -1.0)
}

/// Minimal hand-rolled bindings for the two process counters we need.
///
/// The tree already pins the `windows` crate, but only with the
/// `Win32_System_Console` feature; widening it would mean editing Cargo.toml for
/// two functions. Both live in kernel32 (`K32GetProcessMemoryInfo` is the
/// kernel32-exported alias of psapi's `GetProcessMemoryInfo`, present since
/// Windows 7), which the MSVC and GNU toolchains both link by default.
#[cfg(windows)]
mod win {
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    pub struct Filetime {
        pub low_date_time: u32,
        pub high_date_time: u32,
    }

    impl Filetime {
        #[inline]
        pub fn as_ticks(self) -> u64 {
            ((self.high_date_time as u64) << 32) | self.low_date_time as u64
        }
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    pub struct ProcessMemoryCounters {
        pub cb: u32,
        pub page_fault_count: u32,
        pub peak_working_set_size: usize,
        pub working_set_size: usize,
        pub quota_peak_paged_pool_usage: usize,
        pub quota_paged_pool_usage: usize,
        pub quota_peak_non_paged_pool_usage: usize,
        pub quota_non_paged_pool_usage: usize,
        pub pagefile_usage: usize,
        pub peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetCurrentProcess() -> isize;
        pub fn GetProcessTimes(
            process: isize,
            creation_time: *mut Filetime,
            exit_time: *mut Filetime,
            kernel_time: *mut Filetime,
            user_time: *mut Filetime,
        ) -> i32;
        pub fn K32GetProcessMemoryInfo(
            process: isize,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_are_sane_or_sentinel() {
        let (cpu_s, peak_mb) = process_counters();
        if cfg!(windows) {
            assert!(cpu_s >= 0.0, "cpu_s should be readable on Windows");
            assert!(peak_mb > 0.0, "peak_mb should be readable on Windows");
        } else {
            assert_eq!(cpu_s, -1.0);
            assert_eq!(peak_mb, -1.0);
        }
    }

    #[test]
    fn disabled_by_default_in_test_process() {
        // The test harness never sets ARNIS_PHASE_MARKERS, so these must be
        // silent no-ops rather than panicking or printing.
        phase("fetch");
        done(0);
    }
}
