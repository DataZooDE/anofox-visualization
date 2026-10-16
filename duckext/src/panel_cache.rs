//! Result cache, single-flight and request supersession for the locked server.
//!
//! * **Cache.** A locked server queries an immutable snapshot, so a panel's
//!   result is a pure function of the request: the key is the parsed request
//!   (dashboard, panel, variables, page), and the cache is per server, i.e. per
//!   snapshot. Bounded by bytes (`cache_mb`), least-recently-used eviction;
//!   only successful results are kept.
//! * **Single-flight.** Concurrent identical requests share one execution: the
//!   first runs the query, the others wait for its outcome (errors included).
//! * **Supersession.** A request may carry a ticket key (viewer id / panel /
//!   kind). A newer request with the same key cancels the older one, which then
//!   answers `409` right away. Its query is interrupted once no request waits
//!   for it any more — a shared execution keeps running for the others, and a
//!   newer *identical* request (an impatient double click) takes it over.
//!
//! Everything is guarded by one mutex; waiters sleep on one condvar.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Condvar, Mutex, MutexGuard};

/// The outcome of a panel request: the JSON body, or `(status, message)`.
pub type Outcome = Result<std::sync::Arc<String>, (u16, String)>;

/// What a superseded request answers.
pub const SUPERSEDED: (u16, &str) = (409, "superseded by a newer request");

/// Fixed per-entry overhead counted against the byte cap (maps, LRU index).
const ENTRY_OVERHEAD: usize = 128;

/// What an entry costs: the serialized body as allocated (its capacity), and
/// the key twice (the map and the LRU index each own a copy).
fn entry_size(key: &str, body: &String) -> usize {
    2 * key.len() + body.capacity() + ENTRY_OVERHEAD
}

pub struct PanelCache {
    cap: usize,
    inner: Mutex<Inner>,
    cv: Condvar,
}

#[derive(Default)]
struct Inner {
    entries: HashMap<String, Entry>,
    lru: BTreeMap<u64, String>,
    bytes: usize,
    tick: u64,
    /// key -> the flight new identical requests join.
    joinable: HashMap<String, u64>,
    flights: HashMap<u64, Flight>,
    /// ticket key -> the live ticket holding it.
    keys: HashMap<String, u64>,
    tickets: HashMap<u64, Ticket>,
    next_id: u64,
    stats: Stats,
}

struct Entry {
    body: std::sync::Arc<String>,
    tick: u64,
}

struct Flight {
    key: String,
    /// Requests still interested in the outcome (the executor's included).
    waiters: usize,
    outcome: Option<Outcome>,
    /// Interrupts the running query; set by the executor while it runs.
    interrupt: Option<Box<dyn Fn() + Send>>,
    abandoned: bool,
}

struct Ticket {
    key: String,
    flight: Option<u64>,
    cancelled: bool,
}

/// Counters (for tests and the log).
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stats {
    pub hits: u64,
    pub executed: u64,
    pub shared: u64,
    pub cancelled: u64,
    pub evicted: u64,
}

/// How a request was answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Served {
    Hit,
    Executed,
    Shared,
}

/// Handed to the executor: lets it register an interrupter and check whether
/// every request has given up on its outcome.
pub struct Abort<'a> {
    cache: &'a PanelCache,
    flight: u64,
}

/// Unregisters the interrupter on drop. Keep it declared *after* the
/// connection it interrupts, so it is dropped first: after that, no cancel can
/// touch the connection any more.
pub struct Registered<'a>(&'a Abort<'a>);

/// A request's supersession ticket; released on drop.
struct TicketGuard<'a> {
    cache: &'a PanelCache,
    id: u64,
    /// The ticket this one supersedes (cancelled once this request has joined
    /// or started its flight).
    old: Option<u64>,
}

impl PanelCache {
    /// A cache holding at most `cap_bytes` of results (`0` = no caching; the
    /// single-flight and supersession still apply).
    pub fn new(cap_bytes: usize) -> PanelCache {
        PanelCache {
            cap: cap_bytes,
            inner: Mutex::new(Inner::default()),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn stats(&self) -> Stats {
        self.lock().stats
    }

    /// Take the ticket `key` (the request holding it is cancelled later, by
    /// `run`).
    fn ticket(&self, key: String) -> TicketGuard<'_> {
        let mut g = self.lock();
        g.next_id += 1;
        let id = g.next_id;
        let old = g.keys.insert(key.clone(), id);
        g.tickets.insert(
            id,
            Ticket {
                key,
                flight: None,
                cancelled: false,
            },
        );
        TicketGuard {
            cache: self,
            id,
            old,
        }
    }

    /// Answer `key` from the cache, by joining an identical running request, or
    /// by running `exec` (on this thread). `supersede` (a ticket key) cancels
    /// the running request with the same ticket key, and makes this one
    /// cancellable by the next.
    pub fn run(
        &self,
        key: &str,
        supersede: Option<String>,
        exec: impl FnOnce(&Abort) -> Result<String, (u16, String)>,
    ) -> (Outcome, Served) {
        let ticket = supersede.map(|k| self.ticket(k));
        self.run_ticket(key, ticket.as_ref(), exec)
    }

    fn run_ticket(
        &self,
        key: &str,
        ticket: Option<&TicketGuard>,
        exec: impl FnOnce(&Abort) -> Result<String, (u16, String)>,
    ) -> (Outcome, Served) {
        let tid = ticket.map(|t| t.id);
        let old = ticket.and_then(|t| t.old);
        let mut g = self.lock();
        if tid.is_some_and(|t| g.is_cancelled(t)) {
            return (Err(superseded()), Served::Shared);
        }
        if let Some(body) = g.lookup(key) {
            g.stats.hits += 1;
            self.supersede(&mut g, old);
            return (Ok(body), Served::Hit);
        }
        if let Some(&fid) = g.joinable.get(key) {
            // Join before cancelling the superseded request: if that one waits
            // for this very flight, the query keeps running.
            g.flights.get_mut(&fid).expect("joinable flight").waiters += 1;
            g.attach(tid, fid);
            g.stats.shared += 1;
            self.supersede(&mut g, old);
            let out = self.wait(g, fid, tid);
            return (out, Served::Shared);
        }
        g.next_id += 1;
        let fid = g.next_id;
        g.flights.insert(
            fid,
            Flight {
                key: key.to_string(),
                waiters: 1,
                outcome: None,
                interrupt: None,
                abandoned: false,
            },
        );
        g.joinable.insert(key.to_string(), fid);
        g.attach(tid, fid);
        g.stats.executed += 1;
        self.supersede(&mut g, old);
        drop(g);

        let abort = Abort {
            cache: self,
            flight: fid,
        };
        // A panic must still complete the flight, or its waiters would hang.
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exec(&abort)))
            .unwrap_or_else(|_| Err((500, "internal error".to_string())));

        let mut g = self.lock();
        let out: Outcome = out.map(std::sync::Arc::new);
        if let Ok(body) = &out {
            g.store(self.cap, key, body.clone());
        }
        if g.joinable.get(key) == Some(&fid) {
            g.joinable.remove(key);
        }
        let f = g.flights.get_mut(&fid).expect("own flight");
        f.interrupt = None;
        f.outcome = Some(out);
        if f.waiters == 0 {
            // Everyone (this request too) was superseded.
            g.flights.remove(&fid);
        }
        self.cv.notify_all();
        (self.wait(g, fid, tid), Served::Executed)
    }

    fn supersede(&self, g: &mut Inner, old: Option<u64>) {
        if let Some(old) = old {
            g.cancel(old);
            self.cv.notify_all();
        }
    }

    /// Wait for flight `fid` to finish (or for this request to be superseded),
    /// then leave it.
    fn wait(&self, mut g: MutexGuard<'_, Inner>, fid: u64, tid: Option<u64>) -> Outcome {
        loop {
            if tid.is_some_and(|t| g.is_cancelled(t)) {
                // `cancel` already took this request off the flight.
                return Err(superseded());
            }
            let f = g.flights.get_mut(&fid).expect("waited-on flight");
            if let Some(out) = f.outcome.clone() {
                f.waiters -= 1;
                if f.waiters == 0 {
                    g.flights.remove(&fid);
                }
                if let Some(t) = tid.and_then(|t| g.tickets.get_mut(&t)) {
                    t.flight = None;
                }
                return out;
            }
            g = self.cv.wait(g).unwrap_or_else(|p| p.into_inner());
        }
    }
}

impl Inner {
    fn lookup(&mut self, key: &str) -> Option<std::sync::Arc<String>> {
        self.tick += 1;
        let tick = self.tick;
        let e = self.entries.get_mut(key)?;
        self.lru.remove(&e.tick);
        e.tick = tick;
        self.lru.insert(tick, key.to_string());
        Some(e.body.clone())
    }

    fn store(&mut self, cap: usize, key: &str, body: std::sync::Arc<String>) {
        let size = entry_size(key, &body);
        // One result may take at most a quarter of the cache.
        if size > cap / 4 || self.entries.contains_key(key) {
            return;
        }
        while self.bytes + size > cap {
            let Some((_, old)) = self.lru.pop_first() else {
                break;
            };
            if let Some(e) = self.entries.remove(&old) {
                self.bytes -= entry_size(&old, &e.body);
                self.stats.evicted += 1;
            }
        }
        self.tick += 1;
        self.lru.insert(self.tick, key.to_string());
        self.entries.insert(
            key.to_string(),
            Entry {
                body,
                tick: self.tick,
            },
        );
        self.bytes += size;
    }

    fn attach(&mut self, tid: Option<u64>, fid: u64) {
        if let Some(t) = tid.and_then(|t| self.tickets.get_mut(&t)) {
            t.flight = Some(fid);
        }
    }

    fn is_cancelled(&self, tid: u64) -> bool {
        self.tickets.get(&tid).is_some_and(|t| t.cancelled)
    }

    /// Cancel ticket `tid`: its request stops waiting, and a flight nobody
    /// waits for any more is interrupted (and no longer joinable).
    fn cancel(&mut self, tid: u64) {
        let Some(t) = self.tickets.get_mut(&tid) else {
            return;
        };
        if t.cancelled {
            return;
        }
        t.cancelled = true;
        self.stats.cancelled += 1;
        let Some(fid) = t.flight.take() else {
            return;
        };
        let Some(f) = self.flights.get_mut(&fid) else {
            return;
        };
        if f.outcome.is_some() {
            // Finished already: the waiter just leaves.
            f.waiters -= 1;
            if f.waiters == 0 {
                self.flights.remove(&fid);
            }
            return;
        }
        f.waiters -= 1;
        if f.waiters == 0 {
            f.abandoned = true;
            // Under the lock: the executor unregisters under the same lock
            // before its connection goes away.
            if let Some(intr) = &f.interrupt {
                intr();
            }
            let key = f.key.clone();
            if self.joinable.get(&key) == Some(&fid) {
                self.joinable.remove(&key);
            }
        }
    }
}

impl Abort<'_> {
    /// Register `interrupt` for the running query; `None` if every request has
    /// already given up (don't start).
    pub fn register(&self, interrupt: impl Fn() + Send + 'static) -> Option<Registered<'_>> {
        let mut g = self.cache.lock();
        let f = g.flights.get_mut(&self.flight)?;
        if f.abandoned {
            return None;
        }
        f.interrupt = Some(Box::new(interrupt));
        Some(Registered(self))
    }

    /// Has every request given up on this outcome?
    pub fn abandoned(&self) -> bool {
        self.cache
            .lock()
            .flights
            .get(&self.flight)
            .is_none_or(|f| f.abandoned)
    }
}

impl Drop for Registered<'_> {
    fn drop(&mut self) {
        if let Some(f) = self.0.cache.lock().flights.get_mut(&self.0.flight) {
            f.interrupt = None;
        }
    }
}

impl Drop for TicketGuard<'_> {
    fn drop(&mut self) {
        let mut g = self.cache.lock();
        if let Some(t) = g.tickets.remove(&self.id) {
            if g.keys.get(&t.key) == Some(&self.id) {
                g.keys.remove(&t.key);
            }
        }
    }
}

fn superseded() -> (u16, String) {
    (SUPERSEDED.0, SUPERSEDED.1.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    fn ok(s: &str) -> Result<String, (u16, String)> {
        Ok(s.to_string())
    }

    #[test]
    fn hit_and_miss() {
        let c = PanelCache::new(1 << 20);
        let (a, how) = c.run("k1", None, |_| ok("one"));
        assert_eq!((a.unwrap().as_str(), how), ("one", Served::Executed));
        let (a, how) = c.run("k1", None, |_| panic!("must be cached"));
        assert_eq!((a.unwrap().as_str(), how), ("one", Served::Hit));
        let (a, how) = c.run("k2", None, |_| ok("two"));
        assert_eq!((a.unwrap().as_str(), how), ("two", Served::Executed));
        // Errors are not cached.
        let (a, _) = c.run("bad", None, |_| Err((400, "no".into())));
        assert_eq!(a.unwrap_err().0, 400);
        let (a, how) = c.run("bad", None, |_| ok("fixed"));
        assert_eq!((a.unwrap().as_str(), how), ("fixed", Served::Executed));
        assert_eq!(c.stats().hits, 1);
        assert_eq!(c.stats().executed, 4);
    }

    #[test]
    fn disabled_cache_still_answers() {
        let c = PanelCache::new(0);
        assert_eq!(c.run("k", None, |_| ok("x")).1, Served::Executed);
        assert_eq!(c.run("k", None, |_| ok("x")).1, Served::Executed);
        assert_eq!(c.lock().bytes, 0);
    }

    #[test]
    fn lru_eviction_respects_the_cap() {
        let body = "x".repeat(100);
        let size = 2 * 2 + body.len() + ENTRY_OVERHEAD; // keys are 2 bytes
        let c = PanelCache::new(size * 8); // 8 entries
        for i in 10..18 {
            let _ = c.run(&i.to_string(), None, |_| ok(&body));
        }
        // Touch "10": "11" becomes the least recently used.
        assert_eq!(c.run("10", None, |_| ok("")).1, Served::Hit);
        let _ = c.run("18", None, |_| ok(&body));
        assert_eq!(c.stats().evicted, 1);
        assert_eq!(c.run("10", None, |_| ok(&body)).1, Served::Hit);
        assert_eq!(c.run("11", None, |_| ok(&body)).1, Served::Executed);
        assert!(c.lock().bytes <= size * 8);
        // A result bigger than a quarter of the cache is not kept.
        let big = "y".repeat(size * 3);
        let _ = c.run("bg", None, |_| ok(&big));
        assert_eq!(c.run("bg", None, |_| ok(&big)).1, Served::Executed);
    }

    #[test]
    fn the_cache_counts_what_a_result_allocates() {
        let c = PanelCache::new(1 << 20);
        let mut body = String::with_capacity(10_000);
        body.push_str("[1]");
        let _ = c.run("k", None, move |_| Ok(body));
        assert_eq!(c.lock().bytes, 2 + 10_000 + ENTRY_OVERHEAD);
    }

    #[test]
    fn single_flight_runs_identical_requests_once() {
        let c = Arc::new(PanelCache::new(1 << 20));
        let runs = Arc::new(AtomicUsize::new(0));
        let n = 50;
        let start = Arc::new(Barrier::new(n));
        let handles: Vec<_> = (0..n)
            .map(|_| {
                let (c, runs, start) = (c.clone(), runs.clone(), start.clone());
                std::thread::spawn(move || {
                    start.wait();
                    let (out, _) = c.run("same", None, |_| {
                        runs.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(200));
                        ok("result")
                    });
                    out.unwrap()
                })
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap().as_str(), "result");
        }
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        let s = c.stats();
        assert_eq!(s.executed, 1);
        assert_eq!(s.shared + s.hits, n as u64 - 1);
        assert!(c.lock().flights.is_empty());
    }

    #[test]
    fn concurrent_waiters_share_an_error() {
        let c = Arc::new(PanelCache::new(1 << 20));
        let c2 = c.clone();
        let t = std::thread::spawn(move || {
            c2.run("e", None, |_| {
                std::thread::sleep(Duration::from_millis(200));
                Err((400, "boom".into()))
            })
        });
        std::thread::sleep(Duration::from_millis(50));
        let (out, how) = c.run("e", None, |_| ok("not run"));
        assert_eq!(how, Served::Shared);
        assert_eq!(out.unwrap_err().1, "boom");
        assert_eq!(t.join().unwrap().0.unwrap_err().1, "boom");
    }

    /// An executor that "runs" until interrupted (or 5 s).
    fn interruptible(
        stop: Arc<AtomicBool>,
    ) -> impl FnOnce(&Abort) -> Result<String, (u16, String)> {
        move |abort: &Abort| {
            let s = stop.clone();
            let Some(_reg) = abort.register(move || s.store(true, Ordering::SeqCst)) else {
                return Err((499, "not started".into()));
            };
            let t0 = Instant::now();
            while !stop.load(Ordering::SeqCst) && t0.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(5));
            }
            if stop.load(Ordering::SeqCst) {
                Err((400, "INTERRUPT".into()))
            } else {
                ok("finished")
            }
        }
    }

    #[test]
    fn a_newer_request_cancels_the_older_one() {
        let c = Arc::new(PanelCache::new(1 << 20));
        let stop = Arc::new(AtomicBool::new(false));
        let (c2, s2) = (c.clone(), stop.clone());
        let t0 = Instant::now();
        let old = std::thread::spawn(move || {
            c2.run("page 1", Some("viewer/3/load".into()), interruptible(s2))
                .0
        });
        std::thread::sleep(Duration::from_millis(50));
        let (out, _) = c.run("page 2", Some("viewer/3/load".into()), |_| ok("page 2"));
        assert_eq!(out.unwrap().as_str(), "page 2");
        assert_eq!(old.join().unwrap().unwrap_err().0, 409);
        assert!(stop.load(Ordering::SeqCst), "the old query was interrupted");
        assert!(t0.elapsed() < Duration::from_secs(2));
        let g = c.lock();
        assert!(g.tickets.is_empty() && g.keys.is_empty() && g.flights.is_empty());
        assert!(!g.entries.contains_key("page 1"));
    }

    #[test]
    fn an_identical_newer_request_takes_the_query_over() {
        let c = Arc::new(PanelCache::new(1 << 20));
        let stop = Arc::new(AtomicBool::new(false));
        let (c2, s2) = (c.clone(), stop.clone());
        let old = std::thread::spawn(move || {
            c2.run("page 2", Some("viewer/3/load".into()), move |_: &Abort| {
                std::thread::sleep(Duration::from_millis(200));
                if s2.load(Ordering::SeqCst) {
                    Err((400, "INTERRUPT".into()))
                } else {
                    ok("page 2")
                }
            })
            .0
        });
        std::thread::sleep(Duration::from_millis(50));
        let (out, how) = c.run("page 2", Some("viewer/3/load".into()), |_| ok("restarted"));
        assert_eq!((out.unwrap().as_str(), how), ("page 2", Served::Shared));
        assert_eq!(old.join().unwrap().unwrap_err().0, 409);
        assert_eq!(c.stats().executed, 1);
    }

    #[test]
    fn a_shared_query_survives_one_cancel() {
        let c = Arc::new(PanelCache::new(1 << 20));
        let gate = Arc::new(AtomicBool::new(false));
        // Viewer A executes; viewer B joins the same query; A moves on.
        let (ca, ga) = (c.clone(), gate.clone());
        let a = std::thread::spawn(move || {
            ca.run("same", Some("A/1/load".into()), move |abort: &Abort| {
                let interrupted = Arc::new(AtomicBool::new(false));
                let i2 = interrupted.clone();
                let _reg = abort.register(move || i2.store(true, Ordering::SeqCst));
                while !ga.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                if interrupted.load(Ordering::SeqCst) {
                    Err((400, "INTERRUPT".into()))
                } else {
                    ok("shared")
                }
            })
            .0
        });
        std::thread::sleep(Duration::from_millis(50));
        let cb = c.clone();
        let b = std::thread::spawn(move || {
            cb.run("same", Some("B/1/load".into()), |_| ok("not run")).0
        });
        std::thread::sleep(Duration::from_millis(50));
        let (next, _) = c.run("other", Some("A/1/load".into()), |_| ok("next"));
        assert_eq!(next.unwrap().as_str(), "next");
        gate.store(true, Ordering::SeqCst);
        assert_eq!(a.join().unwrap().unwrap_err().0, 409);
        assert_eq!(b.join().unwrap().unwrap().as_str(), "shared");
        assert_eq!(c.stats().executed, 2);
    }

    #[test]
    fn tickets_of_other_viewers_are_independent() {
        let c = Arc::new(PanelCache::new(1 << 20));
        let stop = Arc::new(AtomicBool::new(false));
        let (c2, s2) = (c.clone(), stop.clone());
        let a =
            std::thread::spawn(move || c2.run("x", Some("A/1/load".into()), interruptible(s2)).0);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            c.run("y", Some("B/1/load".into()), |_| ok("y"))
                .0
                .unwrap()
                .as_str(),
            "y"
        );
        assert!(!stop.load(Ordering::SeqCst));
        stop.store(true, Ordering::SeqCst); // end A's "query"
        assert_eq!(a.join().unwrap().unwrap_err().0, 400);
        let g = c.lock();
        assert!(g.keys.is_empty() && g.tickets.is_empty());
    }

    #[test]
    fn a_panicking_executor_releases_its_waiters() {
        let c = Arc::new(PanelCache::new(1 << 20));
        let c2 = c.clone();
        let t = std::thread::spawn(move || {
            c2.run("p", None, |_| {
                std::thread::sleep(Duration::from_millis(100));
                panic!("executor bug")
            })
            .0
        });
        std::thread::sleep(Duration::from_millis(30));
        let (out, how) = c.run("p", None, |_| ok("not run"));
        assert_eq!(how, Served::Shared);
        assert_eq!(out.unwrap_err().0, 500);
        assert_eq!(t.join().unwrap().unwrap_err().0, 500);
    }
}
