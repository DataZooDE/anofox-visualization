//! Memory bounds of a locked server (#103).
//!
//! DuckDB's `memory_limit` bounds its buffer pool, but not everything a busy
//! server holds:
//!
//! * **Results in flight.** Each executed panel result is built as JSON
//!   outside DuckDB and held until its response is written. [`Inflight`]
//!   counts those bytes across a server's requests (`max_inflight_mb`) and a
//!   request reserves them *before* its buffer grows ([`Charge::hold`]). A
//!   request that would push the total over the cap stops and answers `503`
//!   with `Retry-After`. The first [`FREE_BYTES`] of every request are counted
//!   but never refused, so ordinary panels are not turned away by one large
//!   result; the bound is `max_inflight + threads × FREE_BYTES`.
//! * **Queries at once.** A query's working memory outside the buffer pool
//!   (per-thread sort and aggregate state, the allocator's caches) grows with
//!   the number of queries running together. [`Queries`] lets `max_queries`
//!   run at once; the others wait for a slot (cache hits and requests that
//!   join a running query don't need one). A request that waits longer than
//!   its timeout answers `503`.
//!
//! A per-response cap (`max_response_mb`, like `max_rows`) is checked by the
//! same writer and answers `413`.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// What a request refused for memory or a free query slot answers (`503`).
pub const BUSY: &str = "server busy: too many large results in flight — retry shortly";

/// `Retry-After` of a `503` (seconds).
pub const RETRY_AFTER_S: u64 = 1;

/// Bytes every request may hold without being refused (counted all the same).
pub const FREE_BYTES: usize = 256 << 10;

/// The in-flight result bytes of one server.
pub struct Inflight {
    cap: usize,
    used: AtomicUsize,
    peak: AtomicUsize,
    refused: AtomicU64,
}

impl Inflight {
    pub fn new(cap: usize) -> Inflight {
        Inflight {
            cap,
            used: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            refused: AtomicU64::new(0),
        }
    }

    /// A request's account (empty; released on drop).
    pub fn charge(&self) -> Charge<'_> {
        Charge {
            pool: self,
            held: 0,
            deadline: None,
        }
    }

    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }
}

/// One request's share of [`Inflight`].
pub struct Charge<'a> {
    pool: &'a Inflight,
    held: usize,
    /// #195 (D2, #746): a caller's own deadline (an Ask question's): the
    /// request's slot wait, query time and `-- @materialize` build end by
    /// it as well as by `timeout_ms`.
    deadline: Option<std::time::Instant>,
}

impl Charge<'_> {
    /// End this request by `deadline` too (Ask's `get_panel`).
    pub fn with_deadline(mut self, deadline: std::time::Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// The time a request may take: `base` (`timeout_ms`), or less when the
    /// caller's deadline is nearer (at least 1 ms, so it fails as a timeout).
    pub fn timeout(&self, base: std::time::Duration) -> std::time::Duration {
        match self.deadline {
            Some(d) => base
                .min(d.saturating_duration_since(std::time::Instant::now()))
                .max(std::time::Duration::from_millis(1)),
            None => base,
        }
    }

    /// Hold `bytes` in total from now on: call it *before* allocating them.
    /// `Err(BUSY)` (and nothing held beyond what was) if that would take the
    /// server over its cap.
    pub fn hold(&mut self, bytes: usize) -> Result<(), String> {
        if bytes <= self.held {
            self.pool
                .used
                .fetch_sub(self.held - bytes, Ordering::Relaxed);
            self.held = bytes;
            return Ok(());
        }
        let more = bytes - self.held;
        let now = self.pool.used.fetch_add(more, Ordering::Relaxed) + more;
        if bytes > FREE_BYTES && now > self.pool.cap {
            self.pool.used.fetch_sub(more, Ordering::Relaxed);
            self.pool.refused.fetch_add(1, Ordering::Relaxed);
            return Err(BUSY.into());
        }
        self.held = bytes;
        self.pool.peak.fetch_max(now, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for Charge<'_> {
    fn drop(&mut self) {
        self.pool.used.fetch_sub(self.held, Ordering::Relaxed);
    }
}

/// Slots for queries running at once.
pub struct Queries {
    max: usize,
    state: Mutex<QueryState>,
    cv: Condvar,
}

#[derive(Default)]
struct QueryState {
    running: usize,
    waiting: usize,
    refused: u64,
}

/// A running query's slot; freed on drop.
pub struct Slot<'a>(&'a Queries);

impl Queries {
    pub fn new(max: usize) -> Queries {
        Queries {
            max: max.max(1),
            state: Mutex::new(QueryState::default()),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueryState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Wait for a free slot until `deadline`, or until `gave_up()` (checked
    /// every 50 ms) — then `None`.
    pub fn acquire(&self, deadline: Instant, gave_up: impl Fn() -> bool) -> Option<Slot<'_>> {
        let mut g = self.lock();
        g.waiting += 1;
        let ok = loop {
            if g.running < self.max {
                g.running += 1;
                break true;
            }
            let now = Instant::now();
            if now >= deadline || gave_up() {
                break false;
            }
            let wait = (deadline - now).min(Duration::from_millis(50));
            g = self
                .cv
                .wait_timeout(g, wait)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        };
        g.waiting -= 1;
        if ok {
            Some(Slot(self))
        } else {
            g.refused += 1;
            None
        }
    }

    /// (running, waiting, refused)
    pub fn stats(&self) -> (usize, usize, u64) {
        let g = self.lock();
        (g.running, g.waiting, g.refused)
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.0.lock().running -= 1;
        self.0.cv.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn charges_are_released_and_capped() {
        let pool = Inflight::new(1 << 20);
        let mut a = pool.charge();
        a.hold(600 << 10).unwrap();
        let mut b = pool.charge();
        // Over the cap: refused, nothing more held.
        assert_eq!(b.hold(600 << 10).unwrap_err(), BUSY);
        assert_eq!(pool.used(), 600 << 10);
        assert_eq!(pool.refused(), 1);
        // The free allowance is never refused (but counted).
        b.hold(FREE_BYTES).unwrap();
        assert_eq!(pool.used(), (600 << 10) + FREE_BYTES);
        // Shrinking gives bytes back.
        a.hold(100).unwrap();
        assert_eq!(pool.used(), 100 + FREE_BYTES);
        drop(a);
        b.hold(900 << 10).unwrap();
        drop(b);
        assert_eq!(pool.used(), 0);
        assert_eq!(pool.peak(), 900 << 10);
    }

    #[test]
    fn a_zero_cap_still_lets_small_results_through() {
        let pool = Inflight::new(0);
        let mut c = pool.charge();
        assert!(c.hold(1000).is_ok());
        assert!(c.hold(FREE_BYTES + 1).is_err());
    }

    #[test]
    fn query_slots_queue_and_time_out() {
        let q = Arc::new(Queries::new(1));
        let far = Instant::now() + Duration::from_secs(10);
        let s = q.acquire(far, || false).unwrap();
        // Full: a short deadline expires, a caller who gives up leaves.
        assert!(q
            .acquire(Instant::now() + Duration::from_millis(30), || false)
            .is_none());
        assert!(q.acquire(far, || true).is_none());
        assert_eq!(q.stats(), (1, 0, 2));
        // A waiter gets the slot as soon as it is freed.
        let q2 = q.clone();
        let t = std::thread::spawn(move || q2.acquire(far, || false).is_some());
        while q.stats().1 == 0 {
            std::thread::yield_now();
        }
        drop(s);
        assert!(t.join().unwrap());
        assert_eq!(q.stats(), (0, 0, 2));
    }
}
