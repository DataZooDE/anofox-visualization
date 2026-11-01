//! Conversations (plan-ai-assistant.md §12.5, security review L1): held on
//! the server only, in memory, keyed by a random 128-bit id **and** bound to
//! the viewer's session and `Claims::key()`. The client sends a question,
//! never a history, so it can't forge one in which the model "already read"
//! data.
//!
//! Caps: 3 conversations per viewer, 2 000 per server, 256 KB and 10 turns
//! each (the oldest turns drop first), 30 minutes after the last question.
//! A conversation is dropped on a snapshot refresh (the next question starts
//! fresh, "the data was refreshed"), on a claims change (a different key no
//! longer finds it), on logout and on a policy reload ([`Store::drop_owner`],
//! [`Store::clear`]). Per replica.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::cite::Read;
use super::mask::Masker;

pub const MAX_PER_VIEWER: usize = 3;
pub const MAX_PER_SERVER: usize = 2_000;
pub const MAX_BYTES: usize = 256 << 10;
pub const MAX_TURNS: usize = 10;
pub const IDLE: Duration = Duration::from_secs(30 * 60);

/// Who a conversation belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Owner {
    /// The page or login session.
    pub session: String,
    /// `Claims::key()`: a claims or group change is another owner.
    pub claims: String,
}

/// One finished question: the messages it added (the question, the model's
/// turns, the tool results), as the API takes them back.
#[derive(Debug, Clone)]
pub struct Turn {
    pub messages: Vec<Value>,
    pub bytes: usize,
}

#[derive(Debug, Clone)]
pub struct Conversation {
    pub id: String,
    pub owner: Owner,
    pub dashboard: String,
    /// The snapshot it was held on.
    pub epoch: u64,
    pub turns: Vec<Turn>,
    pub reads: Vec<Read>,
    pub masker: Masker,
    pub last: Instant,
}

impl Conversation {
    pub fn bytes(&self) -> usize {
        self.turns.iter().map(|t| t.bytes).sum::<usize>()
            + self
                .reads
                .iter()
                .map(|r| serde_json::to_string(&r.rows).map_or(0, |s| s.len()))
                .sum::<usize>()
    }

    /// The history to send with the next question.
    pub fn history(&self) -> Vec<Value> {
        self.turns.iter().flat_map(|t| t.messages.clone()).collect()
    }

    /// Keep to the turn and size caps, the oldest turn first.
    fn trim(&mut self) {
        while self.turns.len() > MAX_TURNS || (self.bytes() > MAX_BYTES && !self.turns.is_empty()) {
            self.turns.remove(0);
        }
        while self.bytes() > MAX_BYTES && !self.reads.is_empty() {
            self.reads.remove(0);
        }
    }
}

/// A new conversation id: 128 random bits, hex.
pub fn new_id() -> String {
    let mut b = [0u8; 16];
    aws_lc_rs::rand::fill(&mut b).expect("system randomness");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Default)]
pub struct Store {
    map: Mutex<HashMap<String, Conversation>>,
}

/// What [`Store::take`] found.
pub struct Taken {
    pub conv: Conversation,
    /// The id named a conversation of this owner on an older snapshot.
    pub refreshed: bool,
}

impl Store {
    pub fn new() -> Store {
        Store::default()
    }

    /// The conversation `id` for `owner` on `dashboard` at `epoch`, taken out
    /// of the store for the question (one question per viewer at a time); a
    /// new one when there is none, it expired, it belongs to someone else or
    /// to another dashboard, or the snapshot changed.
    pub fn take(&self, id: Option<&str>, owner: &Owner, dashboard: &str, epoch: u64) -> Taken {
        let now = Instant::now();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, c| now.duration_since(c.last) < IDLE);
        let mut refreshed = false;
        if let Some(id) = id.filter(|i| valid_id(i)) {
            let mine = map
                .get(id)
                .is_some_and(|c| c.owner == *owner && c.dashboard == dashboard);
            if mine {
                let c = map.remove(id).expect("checked above");
                if c.epoch == epoch {
                    return Taken {
                        conv: c,
                        refreshed: false,
                    };
                }
                refreshed = true;
            }
        }
        Taken {
            conv: Conversation {
                id: new_id(),
                owner: owner.clone(),
                dashboard: dashboard.to_string(),
                epoch,
                turns: Vec::new(),
                reads: Vec::new(),
                masker: Masker::new(),
                last: now,
            },
            refreshed,
        }
    }

    /// Put a conversation back after its question, within the caps.
    pub fn put(&self, mut c: Conversation) {
        c.last = Instant::now();
        c.trim();
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let owner = c.owner.clone();
        map.insert(c.id.clone(), c);
        let mut oldest_of = |pred: &dyn Fn(&Conversation) -> bool, cap: usize| loop {
            let mine: Vec<(&String, Instant)> = map
                .iter()
                .filter(|(_, c)| pred(c))
                .map(|(k, c)| (k, c.last))
                .collect();
            if mine.len() <= cap {
                break;
            }
            let k = mine
                .iter()
                .min_by_key(|(_, t)| *t)
                .map(|(k, _)| (*k).clone())
                .expect("over the cap");
            map.remove(&k);
        };
        oldest_of(&|c| c.owner.claims == owner.claims, MAX_PER_VIEWER);
        oldest_of(&|_| true, MAX_PER_SERVER);
    }

    /// Drop every conversation of a session (logout, revocation).
    pub fn drop_owner(&self, session: &str) {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, c| c.owner.session != session);
    }

    /// Drop all (a policy reload).
    pub fn clear(&self) {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    pub fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn owner(s: &str, c: &str) -> Owner {
        Owner {
            session: s.into(),
            claims: c.into(),
        }
    }

    #[test]
    fn found_only_by_its_owner_on_its_dashboard_and_snapshot() {
        let st = Store::new();
        let a = owner("s1", "alice");
        let t = st.take(None, &a, "d", 1);
        let id = t.conv.id.clone();
        assert_eq!(id.len(), 32);
        st.put(t.conv);
        // someone else, another claims key, another dashboard: a new one
        for (o, d) in [
            (owner("s2", "bob"), "d"),
            (owner("s1", "alice2"), "d"),
            (a.clone(), "e"),
        ] {
            let t = st.take(Some(&id), &o, d, 1);
            assert_ne!(t.conv.id, id);
        }
        // its owner gets it back (and it leaves the store meanwhile)
        let t = st.take(Some(&id), &a, "d", 1);
        assert_eq!(t.conv.id, id);
        assert!(
            st.take(Some(&id), &a, "d", 1).conv.id != id,
            "taken while asked"
        );
        st.put(t.conv);
        // a refresh: a fresh conversation, flagged
        let t = st.take(Some(&id), &a, "d", 2);
        assert!(t.refreshed);
        assert_ne!(t.conv.id, id);
    }

    #[test]
    fn caps() {
        let st = Store::new();
        let a = owner("s1", "alice");
        for _ in 0..5 {
            let t = st.take(None, &a, "d", 1);
            st.put(t.conv);
        }
        assert_eq!(st.len(), MAX_PER_VIEWER);
        let mut t = st.take(None, &a, "d", 1);
        for i in 0..15 {
            t.conv.turns.push(Turn {
                messages: vec![json!({"role": "user", "content": format!("q{i}")})],
                bytes: 30_000,
            });
        }
        let id = t.conv.id.clone();
        st.put(t.conv);
        let c = st.take(Some(&id), &a, "d", 1).conv;
        assert!(c.turns.len() <= MAX_TURNS && c.bytes() <= MAX_BYTES);
        assert_eq!(
            c.turns.last().unwrap().messages[0]["content"],
            "q14",
            "the oldest turns dropped"
        );
        st.drop_owner("s1");
        assert!(st.is_empty());
    }
}
