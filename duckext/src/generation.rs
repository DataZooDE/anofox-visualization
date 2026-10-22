//! Atomic snapshot swaps for the locked server (`refresh_s`,
//! `anofox_serve_refresh`).
//!
//! A [`Generation`] is one snapshot plus everything whose validity depends on
//! it: its own result cache (so single-flight, supersession and cached results
//! never cross snapshots), its epoch number (in ETags and `/healthz`) and its
//! build time. [`Current`] holds the generation new requests use. A request
//! takes an `Arc` to it once, at its start, and runs entirely on that one; a
//! swap only changes what the *next* request gets. The old generation (and its
//! database) is dropped when the last request using it ends.
//!
//! [`Current::consecutive_failures`] (#352) counts failed rebuild attempts
//! in a row (0 after any success; a *skipped* refresh, one already running,
//! is never an attempt) — `serve.rs` surfaces it in `/healthz` alongside
//! `source_ok` when `options.source` is configured, since a Quack source is
//! the prototype's first way for a refresh to keep failing indefinitely
//! (the source down, the token rotated, …) while the last good snapshot
//! keeps serving.

use crate::panel_cache::PanelCache;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

pub struct Generation<S> {
    pub snap: S,
    /// Panel results of this snapshot only.
    pub cache: PanelCache,
    /// 1 for the startup snapshot, +1 per successful refresh.
    pub epoch: u64,
    pub built: Instant,
}

/// The generation new requests use, plus the refresh bookkeeping.
pub struct Current<S> {
    gen: RwLock<Arc<Generation<S>>>,
    cache_bytes: usize,
    /// Serialises rebuilds (an on-demand refresh waits for a running one; the
    /// interval refresher skips its turn instead).
    rebuild: Mutex<()>,
    /// Did the last rebuild fail? (The old snapshot stays in service.)
    failed: Mutex<bool>,
    /// Consecutive failed rebuilds (0 after any success; `/healthz`'s
    /// `consecutive_failures`, #352). A *skipped* refresh (the interval
    /// refresher yielding to one already running, `rebuild(false, ..)` ->
    /// `Ok(None)`) never touches this — it isn't an attempt.
    consecutive_failures: AtomicU32,
}

impl<S> Current<S> {
    pub fn new(snap: S, cache_bytes: usize) -> Current<S> {
        Current {
            gen: RwLock::new(Arc::new(Generation {
                snap,
                cache: PanelCache::new(cache_bytes),
                epoch: 1,
                built: Instant::now(),
            })),
            cache_bytes,
            rebuild: Mutex::new(()),
            failed: Mutex::new(false),
            consecutive_failures: AtomicU32::new(0),
        }
    }

    /// The generation a new request runs on (for its whole lifetime).
    pub fn get(&self) -> Arc<Generation<S>> {
        self.gen.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Build a new snapshot with `build` and make it current. `wait`: wait for
    /// a rebuild already in progress (and then build again) instead of
    /// skipping — `Ok(None)` means skipped. On failure the current snapshot
    /// stays in service. The replaced generation is released outside the lock;
    /// it is dropped here unless requests still use it.
    pub fn rebuild(
        &self,
        wait: bool,
        build: impl FnOnce() -> Result<S, String>,
    ) -> Result<Option<Arc<Generation<S>>>, String> {
        let _guard = if wait {
            self.rebuild.lock().unwrap_or_else(|p| p.into_inner())
        } else {
            match self.rebuild.try_lock() {
                Ok(g) => g,
                Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
            }
        };
        let built = build();
        *self.failed.lock().unwrap_or_else(|p| p.into_inner()) = built.is_err();
        if built.is_err() {
            self.consecutive_failures.fetch_add(1, Ordering::SeqCst);
        } else {
            self.consecutive_failures.store(0, Ordering::SeqCst);
        }
        let snap = built?;
        let mut w = self.gen.write().unwrap_or_else(|p| p.into_inner());
        let new = Arc::new(Generation {
            snap,
            cache: PanelCache::new(self.cache_bytes),
            epoch: w.epoch + 1,
            built: Instant::now(),
        });
        let old = std::mem::replace(&mut *w, new.clone());
        drop(w);
        drop(old);
        Ok(Some(new))
    }

    pub fn refreshing(&self) -> bool {
        matches!(
            self.rebuild.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        )
    }

    pub fn last_failed(&self) -> bool {
        *self.failed.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Consecutive failed rebuild attempts (0 right after any success).
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::panel_cache::Served;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    /// A "snapshot" that counts its drops.
    struct Snap {
        data: u64,
        dropped: Arc<AtomicUsize>,
    }
    impl Drop for Snap {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn snap(data: u64, dropped: &Arc<AtomicUsize>) -> Snap {
        Snap {
            data,
            dropped: dropped.clone(),
        }
    }

    #[test]
    fn swap_is_atomic_under_concurrent_reads() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let cur = Arc::new(Current::new(snap(1, &dropped), 1 << 20));
        let stop = Arc::new(AtomicBool::new(false));
        // Readers: a request sees exactly one generation, whose data matches
        // its epoch, from start to end — even across swaps — and epochs only
        // move forward.
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let (cur, stop) = (cur.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut last = 0;
                    let mut n = 0u64;
                    while !stop.load(Ordering::SeqCst) {
                        let g = cur.get();
                        assert!(g.epoch >= last, "epochs never go back");
                        last = g.epoch;
                        assert_eq!(g.snap.data, g.epoch);
                        std::thread::sleep(Duration::from_micros(200));
                        assert_eq!(g.snap.data, g.epoch, "a held snapshot never changes");
                        n += 1;
                    }
                    n
                })
            })
            .collect();
        for i in 2..=50 {
            let d = dropped.clone();
            cur.rebuild(true, || Ok(snap(i, &d))).unwrap().unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        stop.store(true, Ordering::SeqCst);
        for r in readers {
            assert!(r.join().unwrap() > 0);
        }
        assert_eq!(cur.get().epoch, 50);
        // Every replaced snapshot was released once its last reader was done.
        assert_eq!(dropped.load(Ordering::SeqCst), 49);
    }

    #[test]
    fn an_old_snapshot_lives_until_its_last_request_ends() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let cur = Current::new(snap(1, &dropped), 1 << 20);
        let in_flight = cur.get();
        let d = dropped.clone();
        cur.rebuild(true, || Ok(snap(2, &d))).unwrap();
        assert_eq!(cur.get().epoch, 2);
        assert_eq!(dropped.load(Ordering::SeqCst), 0, "still in use");
        assert_eq!(in_flight.snap.data, 1);
        drop(in_flight);
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failed_rebuild_keeps_the_old_snapshot() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let cur = Current::new(snap(1, &dropped), 1 << 20);
        assert!(cur.rebuild(true, || Err("disk full".into())).is_err());
        assert!(cur.last_failed());
        assert_eq!(cur.get().epoch, 1);
        assert_eq!(dropped.load(Ordering::SeqCst), 0);
        let d = dropped.clone();
        cur.rebuild(true, || Ok(snap(2, &d))).unwrap();
        assert!(!cur.last_failed());
    }

    #[test]
    fn consecutive_failures_resets_on_success_and_ignores_skips() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let cur = Arc::new(Current::new(snap(1, &dropped), 1 << 20));
        assert_eq!(cur.consecutive_failures(), 0);
        assert!(cur.rebuild(true, || Err("boom".into())).is_err());
        assert_eq!(cur.consecutive_failures(), 1);
        assert!(cur.rebuild(true, || Err("boom again".into())).is_err());
        assert_eq!(cur.consecutive_failures(), 2);
        // A skipped refresh (one already running) is not an attempt.
        let (c2, d2) = (cur.clone(), dropped.clone());
        let t = std::thread::spawn(move || {
            c2.rebuild(true, || {
                std::thread::sleep(Duration::from_millis(100));
                Ok(snap(2, &d2))
            })
        });
        std::thread::sleep(Duration::from_millis(20));
        let d = dropped.clone();
        assert!(cur.rebuild(false, || Ok(snap(99, &d))).unwrap().is_none());
        assert_eq!(
            cur.consecutive_failures(),
            2,
            "a skipped refresh is not an attempt"
        );
        t.join().unwrap().unwrap();
        assert_eq!(cur.consecutive_failures(), 0, "reset by the success above");
    }

    #[test]
    fn the_cache_is_not_shared_across_epochs() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let cur = Current::new(snap(1, &dropped), 1 << 20);
        let run = |g: &Generation<Snap>| {
            let data = g.snap.data;
            g.cache
                .run("dash/1", None, move |_| Ok(format!("rows of {data}")))
        };
        let g1 = cur.get();
        assert_eq!(run(&g1).1, Served::Executed);
        assert_eq!(run(&g1).1, Served::Hit);
        let d = dropped.clone();
        cur.rebuild(true, || Ok(snap(2, &d))).unwrap();
        let g2 = cur.get();
        let (out, how) = run(&g2);
        assert_eq!(how, Served::Executed, "the new epoch starts empty");
        assert_eq!(out.unwrap().as_str(), "rows of 2");
        // A request still on the old generation keeps its own results.
        assert_eq!(run(&g1).0.unwrap().as_str(), "rows of 1");
    }

    #[test]
    fn single_flight_does_not_join_across_a_swap() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let cur = Arc::new(Current::new(snap(1, &dropped), 1 << 20));
        let g1 = cur.get();
        // A slow query on the old snapshot…
        let slow = std::thread::spawn(move || {
            g1.cache
                .run("dash/1", None, |_| {
                    std::thread::sleep(Duration::from_millis(200));
                    Ok("old".into())
                })
                .0
        });
        std::thread::sleep(Duration::from_millis(50));
        let d = dropped.clone();
        cur.rebuild(true, || Ok(snap(2, &d))).unwrap();
        // …is not joined by the identical request arriving after the swap.
        let (out, how) = cur.get().cache.run("dash/1", None, |_| Ok("new".into()));
        assert_eq!((out.unwrap().as_str(), how), ("new", Served::Executed));
        assert_eq!(slow.join().unwrap().unwrap().as_str(), "old");
    }

    #[test]
    fn concurrent_rebuilds_are_serialised_or_skipped() {
        let dropped = Arc::new(AtomicUsize::new(0));
        let cur = Arc::new(Current::new(snap(1, &dropped), 1 << 20));
        let (c2, d2) = (cur.clone(), dropped.clone());
        let t = std::thread::spawn(move || {
            c2.rebuild(true, || {
                std::thread::sleep(Duration::from_millis(200));
                Ok(snap(2, &d2))
            })
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(cur.refreshing());
        let d = dropped.clone();
        // The interval refresher skips a turn…
        assert!(cur.rebuild(false, || Ok(snap(99, &d))).unwrap().is_none());
        // …an on-demand refresh waits and then builds on top.
        let d = dropped.clone();
        let g = cur.rebuild(true, || Ok(snap(3, &d))).unwrap().unwrap();
        assert_eq!(g.epoch, 3);
        assert_eq!(t.join().unwrap().unwrap().unwrap().epoch, 2);
        assert!(!cur.refreshing());
    }
}
