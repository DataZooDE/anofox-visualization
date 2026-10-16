use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// Manages a collection of threads.
///
/// A new thread is created every time all the existing threads are full.
/// Any idle thread will automatically die after a few seconds.
pub struct TaskPool {
    sharing: Arc<Sharing>,
}

struct Sharing {
    // list of the tasks to be done by worker threads
    todo: Mutex<VecDeque<Box<dyn FnMut() + Send>>>,

    // condvar that will be notified whenever a task is added to `todo`
    condvar: Condvar,

    // number of total worker threads running
    active_tasks: AtomicUsize,

    // number of idle worker threads
    waiting_tasks: AtomicUsize,
}

/// Minimum number of active threads.
static MIN_THREADS: usize = 4;

struct Registration<'a> {
    nb: &'a AtomicUsize,
}

impl<'a> Registration<'a> {
    fn new(nb: &'a AtomicUsize) -> Registration<'a> {
        nb.fetch_add(1, Ordering::Release);
        Registration { nb }
    }
}

impl<'a> Drop for Registration<'a> {
    fn drop(&mut self) {
        self.nb.fetch_sub(1, Ordering::Release);
    }
}

impl TaskPool {
    pub fn new() -> TaskPool {
        let pool = TaskPool {
            sharing: Arc::new(Sharing {
                todo: Mutex::new(VecDeque::new()),
                condvar: Condvar::new(),
                active_tasks: AtomicUsize::new(0),
                waiting_tasks: AtomicUsize::new(0),
            }),
        };

        for _ in 0..MIN_THREADS {
            pool.add_thread(None)
        }

        pool
    }

    /// Executes a function in a thread.
    /// If no thread is available, spawns a new one.
    pub fn spawn(&self, code: Box<dyn FnMut() + Send>) {
        let mut queue = self.sharing.todo.lock().unwrap();
        self.spawn_locked(&mut queue, code);
    }

    /// `spawn` with the `todo` lock already held (`queue` is its contents).
    fn spawn_locked(
        &self,
        queue: &mut VecDeque<Box<dyn FnMut() + Send>>,
        code: Box<dyn FnMut() + Send>,
    ) {
        queue.push_back(code);

        // An idle thread woken by `notify_one` stays counted in `waiting_tasks`
        // until it re-takes the lock. Checking `waiting_tasks > 0` alone loses
        // the second wake-up when two tasks are spawned before that happens,
        // and the second task then waits for some thread to finish its current
        // task (a keep-alive connection: minutes). So make sure every queued
        // task has an idle thread of its own, and start one if it does not.
        if queue.len() > self.sharing.waiting_tasks.load(Ordering::Acquire) {
            self.add_thread(None);
        } else {
            self.sharing.condvar.notify_one();
        }
    }

    fn add_thread(&self, initial_fn: Option<Box<dyn FnMut() + Send>>) {
        let sharing = self.sharing.clone();

        thread::spawn(move || {
            let sharing = sharing;
            let _active_guard = Registration::new(&sharing.active_tasks);

            if let Some(mut f) = initial_fn {
                f();
            }

            loop {
                let mut task: Box<dyn FnMut() + Send> = {
                    let mut todo = sharing.todo.lock().unwrap();

                    let task;
                    loop {
                        if let Some(poped_task) = todo.pop_front() {
                            task = poped_task;
                            break;
                        }
                        let _waiting_guard = Registration::new(&sharing.waiting_tasks);

                        let received =
                            if sharing.active_tasks.load(Ordering::Acquire) <= MIN_THREADS {
                                todo = sharing.condvar.wait(todo).unwrap();
                                true
                            } else {
                                let (new_lock, waitres) = sharing
                                    .condvar
                                    .wait_timeout(todo, Duration::from_millis(5000))
                                    .unwrap();
                                todo = new_lock;
                                !waitres.timed_out()
                            };

                        if !received && todo.is_empty() {
                            return;
                        }
                    }

                    task
                };

                task();
            }
        });
    }
}

impl Drop for TaskPool {
    fn drop(&mut self) {
        self.sharing
            .active_tasks
            .store(999_999_999, Ordering::Release);
        self.sharing.condvar.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::{TaskPool, MIN_THREADS};
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    // Several connections accepted before the woken idle thread re-takes the
    // lock: every task must still start, while all of them stay busy (as
    // keep-alive connections do). Before the fix only MIN_THREADS of them ran.
    #[test]
    fn spawns_burst_without_losing_a_wake_up() {
        let pool = TaskPool::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while pool.sharing.waiting_tasks.load(Ordering::Acquire) < MIN_THREADS {
            assert!(Instant::now() < deadline, "pool threads never went idle");
            std::thread::sleep(Duration::from_millis(1));
        }

        let n = MIN_THREADS + 2;
        let (started_tx, started_rx) = mpsc::channel();
        let release = Arc::new(Barrier::new(n + 1));
        {
            // Holding the lock across the burst is the race window made
            // deterministic: no woken thread can re-take it in between.
            let mut queue = pool.sharing.todo.lock().unwrap();
            for _ in 0..n {
                let started_tx = started_tx.clone();
                let release = release.clone();
                pool.spawn_locked(
                    &mut queue,
                    Box::new(move || {
                        started_tx.send(()).unwrap();
                        release.wait();
                    }),
                );
            }
        }

        for i in 0..n {
            assert!(
                started_rx.recv_timeout(Duration::from_secs(10)).is_ok(),
                "only {} of {} tasks started: a wake-up was lost",
                i,
                n
            );
        }
        release.wait();
    }
}
