//! Ask's limits (plan-ai-assistant.md §12.4, security review H2 / M2): per
//! viewer, questions and tokens over a sliding hour and day and one question
//! at a time; per server, tokens over a sliding hour and questions at once.
//! Tokens are **reserved before** a model call and reconciled from its
//! `usage` after, so concurrent questions can't overshoot a budget.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::config::AskConfig;

const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(24 * 3600);

/// Over a limit: the viewer's message (HTTP 429).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Over(pub String);

#[derive(Default)]
struct Use {
    questions: VecDeque<Instant>,
    /// Tokens (reservations and corrections, signed), with when.
    tokens: VecDeque<(Instant, i64)>,
    busy: bool,
}

fn since(q: &VecDeque<(Instant, i64)>, now: Instant, window: Duration) -> i64 {
    q.iter()
        .filter(|(t, _)| now.duration_since(*t) < window)
        .map(|(_, n)| *n)
        .sum()
}

#[derive(Default)]
struct State {
    viewers: HashMap<String, Use>,
    server: VecDeque<(Instant, i64)>,
    active: usize,
}

pub struct Limits {
    cfg: AskConfig,
    state: Arc<Mutex<State>>,
}

/// One question in progress: dropping it frees the viewer and the server
/// slot.
pub struct Ticket {
    state: Arc<Mutex<State>>,
    viewer: String,
}

impl std::fmt::Debug for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ticket({})", self.viewer)
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.active = s.active.saturating_sub(1);
        if let Some(u) = s.viewers.get_mut(&self.viewer) {
            u.busy = false;
        }
    }
}

impl Limits {
    pub fn new(cfg: &AskConfig) -> Limits {
        Limits {
            cfg: cfg.clone(),
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// Start a question for `viewer` (counted now, also if it fails later).
    pub fn begin(&self, viewer: &str) -> Result<Ticket, Over> {
        let now = Instant::now();
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.active >= self.cfg.concurrency {
            return Err(Over(
                "The assistant is busy right now — try again in a minute.".into(),
            ));
        }
        let pv = &self.cfg.per_viewer;
        let u = s.viewers.entry(viewer.to_string()).or_default();
        while u
            .questions
            .front()
            .is_some_and(|t| now.duration_since(*t) >= DAY)
        {
            u.questions.pop_front();
        }
        while u
            .tokens
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) >= DAY)
        {
            u.tokens.pop_front();
        }
        if u.busy {
            return Err(Over(
                "You already have a question running — wait for its answer or cancel it.".into(),
            ));
        }
        let last_hour = u
            .questions
            .iter()
            .filter(|t| now.duration_since(**t) < HOUR)
            .count();
        if u.questions.len() >= pv.per_day as usize {
            return Err(Over(format!(
                "You've reached today's limit of {} questions.",
                pv.per_day
            )));
        }
        if last_hour >= pv.per_hour as usize {
            return Err(Over(format!(
                "You've reached the limit of {} questions an hour.",
                pv.per_hour
            )));
        }
        if since(&u.tokens, now, DAY) >= pv.tokens_per_day as i64
            || since(&u.tokens, now, HOUR) >= pv.tokens_per_hour as i64
        {
            return Err(Over("You've used up your assistant budget for now.".into()));
        }
        u.questions.push_back(now);
        u.busy = true;
        s.active += 1;
        Ok(Ticket {
            state: self.state.clone(),
            viewer: viewer.to_string(),
        })
    }

    /// Reserve `n` tokens for a model call of `viewer`.
    pub fn reserve(&self, viewer: &str, n: u64) -> Result<(), Over> {
        let now = Instant::now();
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while s
            .server
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) >= HOUR)
        {
            s.server.pop_front();
        }
        let n = n as i64;
        if since(&s.server, now, HOUR) + n > self.cfg.tokens_per_hour as i64 {
            return Err(Over(
                "The assistant has used its budget for this hour — try again later.".into(),
            ));
        }
        let pv = self.cfg.per_viewer.clone();
        let u = s.viewers.entry(viewer.to_string()).or_default();
        if since(&u.tokens, now, HOUR) + n > pv.tokens_per_hour as i64
            || since(&u.tokens, now, DAY) + n > pv.tokens_per_day as i64
        {
            return Err(Over("You've used up your assistant budget for now.".into()));
        }
        u.tokens.push_back((now, n));
        s.server.push_back((now, n));
        Ok(())
    }

    /// A call reserved `reserved` and used `used`: book the difference.
    pub fn reconcile(&self, viewer: &str, reserved: u64, used: u64) {
        let d = used as i64 - reserved as i64;
        if d == 0 {
            return;
        }
        let now = Instant::now();
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.server.push_back((now, d));
        s.viewers
            .entry(viewer.to_string())
            .or_default()
            .tokens
            .push_back((now, d));
    }

    /// Tokens booked for the server over the last hour.
    pub fn server_hour(&self) -> i64 {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        since(&s.server, Instant::now(), HOUR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> AskConfig {
        let mut c = AskConfig::default();
        c.per_viewer.per_hour = 2;
        c.per_viewer.tokens_per_hour = 1_000;
        c.tokens_per_hour = 1_500;
        c.concurrency = 2;
        c
    }

    #[test]
    fn one_question_at_a_time_and_per_hour() {
        let l = Limits::new(&cfg());
        let t = l.begin("a").unwrap();
        assert!(l
            .begin("a")
            .unwrap_err()
            .0
            .contains("already have a question"));
        drop(t);
        drop(l.begin("a").unwrap());
        assert!(l.begin("a").unwrap_err().0.contains("2 questions an hour"));
        // concurrency is server-wide
        let _b = l.begin("b").unwrap();
        let _c = l.begin("c").unwrap();
        assert!(l.begin("d").unwrap_err().0.contains("busy"));
    }

    #[test]
    fn tokens_are_reserved_then_reconciled() {
        let l = Limits::new(&cfg());
        l.reserve("a", 800).unwrap();
        assert!(l.reserve("a", 300).is_err(), "over the viewer's hour");
        l.reconcile("a", 800, 100);
        l.reserve("a", 300).unwrap();
        l.reserve("b", 900).unwrap();
        assert!(l.reserve("c", 300).is_err(), "over the server's hour");
        assert_eq!(l.server_hour(), 1_300);
    }
}
