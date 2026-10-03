//! Process-wide ceiling on in-flight HTTP requests.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};

/// Sits above every per-provider download pool (4-8 threads), so ordinary
/// fetching never waits on it. It only bounds unrelated fan-outs stacking on
/// top of each other, which is what exhausts the Windows I/O resource limits
/// the tokio driver panics on.
pub(crate) const MAX_CONCURRENT_REQUESTS: usize = 16;

/// The ceiling in force: `MAX_CONCURRENT_REQUESTS` unless `--max-downloads`
/// set another.
static MAX_REQUESTS: AtomicUsize = AtomicUsize::new(MAX_CONCURRENT_REQUESTS);

/// Changes the ceiling. Call before any download starts.
pub fn set_max_requests(n: usize) {
    MAX_REQUESTS.store(n.max(1), Ordering::Relaxed);
}

pub fn max_requests() -> usize {
    MAX_REQUESTS.load(Ordering::Relaxed)
}

static IN_FLIGHT: Mutex<usize> = Mutex::new(0);
static SLOT_FREED: Condvar = Condvar::new();

/// Releases its slot on drop, including while unwinding from a panic.
pub struct RequestPermit {
    _private: (),
}

impl Drop for RequestPermit {
    fn drop(&mut self) {
        {
            let mut in_flight = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
            *in_flight = in_flight.saturating_sub(1);
        }
        SLOT_FREED.notify_one();
    }
}

/// The in-flight counter is process wide, so a test that asserts an exact
/// value of it cannot run beside a test that takes a permit of its own. Every
/// test in the crate that does either holds this first; the tests below and the
/// Mapillary fetch tests are the current users.
#[cfg(test)]
pub static PERMIT_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Blocks until a slot is free. Hold it across a single request/response only:
/// acquiring a second permit while holding one would deadlock.
#[must_use]
pub fn request_permit() -> RequestPermit {
    let mut in_flight = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    while *in_flight >= max_requests() {
        in_flight = SLOT_FREED
            .wait(in_flight)
            .unwrap_or_else(|e| e.into_inner());
    }
    *in_flight += 1;
    RequestPermit { _private: () }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The counter is process-global, so these cases cannot observe each other
    // or anything else in the crate that takes a permit.
    use super::PERMIT_TEST_LOCK as SERIALIZE;

    fn in_flight() -> usize {
        *IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn permit_releases_on_drop() {
        let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
        {
            let _permit = request_permit();
            assert_eq!(in_flight(), 1);
        }
        assert_eq!(in_flight(), 0);
    }

    #[test]
    fn permit_releases_while_unwinding() {
        let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
        let result = std::panic::catch_unwind(|| {
            let _permit = request_permit();
            panic!("boom");
        });
        assert!(result.is_err());
        assert_eq!(in_flight(), 0);
    }

    #[test]
    fn concurrent_holders_never_exceed_the_cap() {
        let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
        let threads: Vec<_> = (0..MAX_CONCURRENT_REQUESTS * 2)
            .map(|_| {
                std::thread::spawn(|| {
                    let _permit = request_permit();
                    assert!(in_flight() <= MAX_CONCURRENT_REQUESTS);
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(in_flight(), 0);
    }

    #[test]
    fn a_lowered_ceiling_is_the_one_enforced() {
        let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
        set_max_requests(2);
        let peak = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    let _permit = request_permit();
                    peak.fetch_max(in_flight(), Ordering::Relaxed);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                });
            }
        });
        set_max_requests(MAX_CONCURRENT_REQUESTS);
        assert!(peak.load(Ordering::Relaxed) <= 2);
        assert_eq!(in_flight(), 0);
    }
}
