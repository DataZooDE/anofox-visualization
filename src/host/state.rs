//! Per-viewer append-only state (`options.state`, #517 slice **S1**): config
//! parsing/validation and the JSONL store + a per-user rate limiter. No
//! server wiring yet — `LockedOptions::parse` (`duckext/src/serve.rs`)
//! doesn't accept `options.state` until slice S2, and there is no
//! `/api/submit` endpoint (S2) or read-back (S3). This module is unit-tested
//! in isolation so S2 only has to wire it in.
//!
//! See `~/projects/duckdb/plans/plan-517.md` §§1–2 for the contract this
//! follows (storage choice, JSONL line format, path rules, start-time
//! refusals) and §11 for the threat model each check below defends against
//! (row numbers noted in the doc comments).
//!
//! * [`StateOptions::parse`] — structural validation of the `options.state`
//!   JSON object alone (unknown keys, types, ranges).
//! * [`StateOptions::validate_start`] — the start-time refusals that need
//!   the rest of the server's config ([`StartContext`]): `auth`, an `embed`
//!   login without `frame_ancestors`, the known dashboard ids, and names
//!   already taken in the snapshot's default database.
//! * [`check_path_rules`] — §1's placement rules for `options.state.path`.
//! * [`Store`] — the append-only JSONL file: single writer (`flock`),
//!   `fdatasync` before every acknowledged append, torn-tail recovery and
//!   fail-closed validation at open.
//! * [`RateLimiter`] — a per-user token bucket, bounded (oldest evicted).

use crate::host::serving::too_broad;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

// ─────────────────────────────────────────────────────────────── options ──

/// `options.state.trainers`: dashboards whose viewers in `groups`/`users` may
/// read every viewer's rows (threat model #19: anyone else on the same
/// dashboard still gets only their own).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Trainers {
    pub dashboards: Vec<String>,
    pub groups: Vec<String>,
    pub users: Vec<String>,
}

/// Parsed, structurally valid `options.state` (plan §2).
#[derive(Debug, Clone, PartialEq)]
pub struct StateOptions {
    pub path: PathBuf,
    /// May submit, and read their own rows.
    pub dashboards: Vec<String>,
    /// Read their own rows only ("my progress" overviews).
    pub read: Vec<String>,
    pub trainers: Option<Trainers>,
    pub text_max_chars: usize,
    pub rate_per_minute: u32,
    pub rate_burst: u32,
    pub max_per_viewer: u64,
    pub max_mb: u64,
    pub trainer_max_rows: u64,
}

/// `MAX_VALUE_LEN` from plan §4.4 — the hard ceiling `text_max_chars` is
/// clamped to, however large an operator asks for.
pub const MAX_TEXT_MAX_CHARS: usize = 4096;
pub const DEFAULT_TEXT_MAX_CHARS: usize = 1000;
pub const DEFAULT_RATE_PER_MINUTE: u32 = 6;
pub const DEFAULT_RATE_BURST: u32 = 3;
pub const DEFAULT_MAX_PER_VIEWER: u64 = 1000;
pub const DEFAULT_MAX_MB: u64 = 256;
pub const DEFAULT_TRAINER_MAX_ROWS: u64 = 200_000;
/// Bound for the rate limiter's and the (future, S2) nonce-dedup map's user
/// count — plan §4.4 "bounded (LRU, 100k users)".
pub const DEFAULT_BOUNDED_USERS: usize = 100_000;

impl StateOptions {
    /// Parse `options.state` (already-decoded JSON) on its own — no start
    /// refusal needs `LockedOptions` here. See [`validate_start`] for those.
    pub fn parse(v: &serde_json::Value) -> Result<StateOptions, String> {
        let m = v.as_object().ok_or("options.state must be an object")?;

        let str_list = |val: &serde_json::Value, key: &str| -> Result<Vec<String>, String> {
            let arr = val
                .as_array()
                .ok_or_else(|| format!("options.state.{key} must be an array of strings"))?;
            arr.iter()
                .map(|x| {
                    x.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| format!("options.state.{key}: every entry must be a string"))
                })
                .collect()
        };

        let mut path: Option<PathBuf> = None;
        let mut dashboards = Vec::new();
        let mut read = Vec::new();
        let mut trainers: Option<Trainers> = None;
        let mut text_max_chars = DEFAULT_TEXT_MAX_CHARS;
        let mut rate_per_minute = DEFAULT_RATE_PER_MINUTE;
        let mut rate_burst = DEFAULT_RATE_BURST;
        let mut max_per_viewer = DEFAULT_MAX_PER_VIEWER;
        let mut max_mb = DEFAULT_MAX_MB;
        let mut trainer_max_rows = DEFAULT_TRAINER_MAX_ROWS;

        for (k, val) in m {
            match k.as_str() {
                "path" => {
                    let p = val
                        .as_str()
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .ok_or("options.state.path must be a file path")?;
                    path = Some(PathBuf::from(p));
                }
                "dashboards" => dashboards = str_list(val, "dashboards")?,
                "read" => read = str_list(val, "read")?,
                "trainers" => {
                    let tm = val.as_object().ok_or(
                        "options.state.trainers must be an object like \
                         {\"dashboards\": […], \"groups\": […], \"users\": […]}",
                    )?;
                    if let Some(other) = tm
                        .keys()
                        .find(|k| !matches!(k.as_str(), "dashboards" | "groups" | "users"))
                    {
                        return Err(format!(
                            "options.state.trainers: unknown key '{other}' (dashboards, groups, users)"
                        ));
                    }
                    let t_dashboards = match tm.get("dashboards") {
                        Some(v) => str_list(v, "trainers.dashboards")?,
                        None => Vec::new(),
                    };
                    let groups = match tm.get("groups") {
                        Some(v) => str_list(v, "trainers.groups")?,
                        None => Vec::new(),
                    };
                    let users = match tm.get("users") {
                        Some(v) => str_list(v, "trainers.users")?,
                        None => Vec::new(),
                    };
                    if groups.is_empty() && users.is_empty() {
                        return Err("options.state.trainers needs groups or users \
                                    (otherwise no one could ever read as a trainer)"
                            .into());
                    }
                    trainers = Some(Trainers {
                        dashboards: t_dashboards,
                        groups,
                        users,
                    });
                }
                "text_max_chars" => {
                    let n = val
                        .as_u64()
                        .filter(|n| *n > 0)
                        .ok_or("options.state.text_max_chars must be a positive integer")?;
                    text_max_chars = (n as usize).min(MAX_TEXT_MAX_CHARS);
                }
                "rate" => {
                    let rm = val.as_object().ok_or(
                        "options.state.rate must be an object like \
                         {\"per_minute\": 6, \"burst\": 3}",
                    )?;
                    if let Some(other) = rm
                        .keys()
                        .find(|k| !matches!(k.as_str(), "per_minute" | "burst"))
                    {
                        return Err(format!(
                            "options.state.rate: unknown key '{other}' (per_minute, burst)"
                        ));
                    }
                    if let Some(v) = rm.get("per_minute") {
                        rate_per_minute = v
                            .as_u64()
                            .filter(|n| *n > 0)
                            .ok_or("options.state.rate.per_minute must be a positive integer")?
                            .min(10_000) as u32;
                    }
                    if let Some(v) = rm.get("burst") {
                        rate_burst = v
                            .as_u64()
                            .filter(|n| *n > 0)
                            .ok_or("options.state.rate.burst must be a positive integer")?
                            .min(10_000) as u32;
                    }
                }
                "max_per_viewer" => {
                    max_per_viewer = val
                        .as_u64()
                        .filter(|n| *n > 0)
                        .ok_or("options.state.max_per_viewer must be a positive integer")?;
                }
                "max_mb" => {
                    max_mb = val
                        .as_u64()
                        .filter(|n| *n > 0)
                        .ok_or("options.state.max_mb must be a positive integer")?
                        .min(1 << 20);
                }
                "trainer_max_rows" => {
                    trainer_max_rows = val
                        .as_u64()
                        .filter(|n| *n > 0)
                        .ok_or("options.state.trainer_max_rows must be a positive integer")?;
                }
                other => {
                    return Err(format!(
                        "options.state: unknown key '{other}' (path, dashboards, read, \
                         trainers, text_max_chars, rate, max_per_viewer, max_mb, trainer_max_rows)"
                    ));
                }
            }
        }

        let path = path.ok_or("options.state.path is required")?;
        if dashboards.is_empty()
            && read.is_empty()
            && trainers.as_ref().is_none_or(|t| t.dashboards.is_empty())
        {
            return Err(
                "options.state needs at least one of dashboards, read or trainers.dashboards"
                    .into(),
            );
        }
        if let Some(t) = &trainers {
            if let Some(dup) = t.dashboards.iter().find(|d| dashboards.contains(d)) {
                return Err(format!(
                    "options.state: dashboard '{dup}' is in both dashboards and \
                     trainers.dashboards — keep the learner and trainer pages separate"
                ));
            }
        }

        Ok(StateOptions {
            path,
            dashboards,
            read,
            trainers,
            text_max_chars,
            rate_per_minute,
            rate_burst,
            max_per_viewer,
            max_mb,
            trainer_max_rows,
        })
    }

    /// Every dashboard id named anywhere in this config (deduplicated),
    /// for a caller checking them against the server's known dashboards.
    pub fn named_dashboards(&self) -> Vec<&str> {
        let mut all: Vec<&str> = self
            .dashboards
            .iter()
            .map(String::as_str)
            .chain(self.read.iter().map(String::as_str))
            .chain(
                self.trainers
                    .iter()
                    .flat_map(|t| t.dashboards.iter().map(String::as_str)),
            )
            .collect();
        all.sort_unstable();
        all.dedup();
        all
    }

    /// What [`validate_start`] needs from the rest of the server's config —
    /// kept separate so this module doesn't have to know about
    /// `LockedOptions`, `Auth` or the dashboard loader.
    pub fn validate_start(&self, ctx: &StartContext) -> Result<(), String> {
        // Threat #14: no anonymous submissions.
        if !ctx.has_auth {
            return Err(
                "options.state needs options.auth: a submission always carries a \
                         signed-in viewer's identity, never an anonymous one"
                    .into(),
            );
        }
        // Threat #8: clickjacking the submit button through an embed frame.
        if ctx.embed_without_frame_ancestors {
            return Err(
                "options.state with an embed login needs an explicit options.frame_ancestors \
                 — without it any site may frame the page the submit button is on"
                    .into(),
            );
        }
        // §2: `viewer_state`/`viewer_state_latest` are server-provided; a
        // same-named table or view already in the snapshot would collide
        // with the temp this feature creates (S3) once it's wired in.
        for n in ["viewer_state", "viewer_state_latest"] {
            if ctx.reserved_names.iter().any(|r| r.eq_ignore_ascii_case(n)) {
                return Err(format!(
                    "options.state: the default database already has a table or view \
                     named '{n}' — options.state reserves that name for the viewer's own rows"
                ));
            }
        }
        for d in self.named_dashboards() {
            if !ctx.known_dashboards.iter().any(|k| k == d) {
                return Err(format!(
                    "options.state names dashboard '{d}', which this server doesn't serve"
                ));
            }
        }
        check_path_rules(&self.path, ctx.data_dir, ctx.dashboard_dir)?;
        Ok(())
    }
}

/// What [`StateOptions::validate_start`] needs from the rest of the locked
/// server's configuration.
pub struct StartContext<'a> {
    pub has_auth: bool,
    /// An `embed` login is configured and `frame_ancestors` was **not**
    /// given explicitly.
    pub embed_without_frame_ancestors: bool,
    /// The canonical `options.data` directory, if any.
    pub data_dir: Option<&'a Path>,
    /// The folder the server's dashboards live in.
    pub dashboard_dir: &'a Path,
    /// Every dashboard id this server serves.
    pub known_dashboards: &'a [String],
    /// Table/view names already defined in the snapshot's default database.
    pub reserved_names: &'a [String],
}

/// Plan §1's placement rules for `options.state.path`. Fails closed: any
/// I/O error resolving a directory (doesn't exist, no permission, …) is
/// itself a refusal, not a "skip the check".
///
/// Checked against the path's **parent** directory, which must already
/// exist (the store file itself is created by [`Store::open`] if missing).
pub fn check_path_rules(
    path: &Path,
    data_dir: Option<&Path>,
    dashboard_dir: &Path,
) -> Result<(), String> {
    if path.file_name().is_none() {
        return Err("options.state.path must name a file".into());
    }
    let parent_raw = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let parent = std::fs::canonicalize(parent_raw)
        .map_err(|e| format!("options.state.path: '{}': {e}", parent_raw.display()))?;
    if !parent.is_dir() {
        return Err(format!(
            "options.state.path: '{}' is not a directory",
            parent.display()
        ));
    }
    if let Some(why) = too_broad(&parent) {
        return Err(format!(
            "options.state.path: refusing a store under '{}': {why} — point it at a folder \
             that holds the store only",
            parent.display()
        ));
    }
    if let Some(data) = data_dir {
        if parent.starts_with(data) || data.starts_with(&parent) {
            return Err(
                "options.state.path must not be inside options.data (it would become \
                 readable through allowed_directories), and options.data must not be inside \
                 the state file's directory"
                    .into(),
            );
        }
    }
    if let Ok(dash) = std::fs::canonicalize(dashboard_dir) {
        if parent.starts_with(&dash) {
            return Err("options.state.path must not be inside the dashboard folder".into());
        }
    }
    // The snapshot's own scratch files live under the system temp dir — a
    // store there could be swept by anything that cleans it up, and the
    // path rule means to keep the store off every path the server already
    // treats specially.
    if let Ok(tmp) = std::fs::canonicalize(std::env::temp_dir()) {
        if parent.starts_with(&tmp) {
            return Err(
                "options.state.path must not be inside the system temp directory (the \
                 snapshot's own scratch files live there)"
                    .into(),
            );
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&parent)
            .map_err(|e| format!("options.state.path: '{}': {e}", parent.display()))?;
        if meta.permissions().mode() & 0o022 != 0 {
            return Err(format!(
                "options.state.path: '{}' is group- or world-writable — point it at a \
                 directory only the server can write",
                parent.display()
            ));
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────── records ──

/// Current line-format version (plan §1's "versioned" contract).
pub const CURRENT_VERSION: u32 = 1;

/// One append-only line: `{"v":1,"seq":42,"ts":"…","user":"…","auth":"oidc",
/// "dashboard":"tutorial","submit":"quiz_1","id":"<32 hex>","vars":{…}}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub v: u32,
    pub seq: u64,
    pub ts: String,
    pub user: String,
    pub auth: String,
    pub dashboard: String,
    pub submit: String,
    pub id: String,
    pub vars: serde_json::Map<String, serde_json::Value>,
}

/// The fields the caller supplies for a new submission — `v`, `seq` and `ts`
/// are always the store's own (plan §4.1: "there is no `user`, `ts` or `seq`
/// field" in the request).
#[derive(Debug, Clone)]
pub struct NewRecord {
    pub user: String,
    pub auth: String,
    pub dashboard: String,
    pub submit: String,
    pub id: String,
    pub vars: serde_json::Map<String, serde_json::Value>,
}

impl Record {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "v": self.v,
            "seq": self.seq,
            "ts": self.ts,
            "user": self.user,
            "auth": self.auth,
            "dashboard": self.dashboard,
            "submit": self.submit,
            "id": self.id,
            "vars": self.vars,
        })
    }

    /// `serde_json`'s own single-line encoding (control characters,
    /// newlines and quotes inside a value are escaped, threat #6) — one
    /// record is always one line.
    fn to_line(&self) -> String {
        self.to_json().to_string()
    }

    fn from_json(v: &serde_json::Value) -> Result<Record, String> {
        let o = v.as_object().ok_or("not a JSON object")?;
        let get_str = |k: &str| -> Result<String, String> {
            o.get(k)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(|| format!("missing or non-string '{k}'"))
        };
        let v_ver = o
            .get("v")
            .and_then(|v| v.as_u64())
            .ok_or("missing or non-integer 'v'")?;
        let seq = o
            .get("seq")
            .and_then(|v| v.as_u64())
            .ok_or("missing or non-integer 'seq'")?;
        let vars = o
            .get("vars")
            .and_then(|v| v.as_object())
            .cloned()
            .ok_or("missing or non-object 'vars'")?;
        Ok(Record {
            v: v_ver as u32,
            seq,
            ts: get_str("ts")?,
            user: get_str("user")?,
            auth: get_str("auth")?,
            dashboard: get_str("dashboard")?,
            submit: get_str("submit")?,
            id: get_str("id")?,
            vars,
        })
    }
}

// ────────────────────────────────────────────────────────────────── store ──

struct StoreState {
    file: std::fs::File,
    records: Vec<Record>,
    by_user: HashMap<String, Vec<usize>>,
    next_seq: u64,
    size_bytes: u64,
}

/// The append-only JSONL store (plan §1): one writer per file (`flock`),
/// `fdatasync` before every acknowledged append, fail-closed validation and
/// torn-tail recovery at open. Pure except for the file handle — the
/// `#[cfg(unix)]` guards mirror [`crate::host::access::open_log`].
pub struct Store {
    inner: Mutex<StoreState>,
    max_bytes: u64,
    /// Set once an append's write/sync fails: every later append refuses
    /// (plan §1 "never retry silently, never acknowledge an unsynced row").
    failed: AtomicBool,
    /// Test-only fault injection (`io_error_while_appending_flips_store_to_
    /// failed`): forces the next append's write to fail. Per-instance (not
    /// a global static) so it can't make an unrelated `Store` in another
    /// test fail under `cargo test`'s default thread-per-test parallelism.
    /// Avoids the double-close-a-live-fd trick, which recent `std` treats
    /// as a hard IO-safety violation and aborts the process over.
    #[cfg(test)]
    force_fail: AtomicBool,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("max_bytes", &self.max_bytes)
            .field("failed", &self.failed.load(Ordering::SeqCst))
            .field("len", &self.len())
            .finish()
    }
}

/// Bytes accounted per line beyond its own length (the `\n`).
const LINE_OVERHEAD: u64 = 1;

impl Store {
    /// Open (creating if absent) and validate the whole file, holding an
    /// exclusive `flock` for as long as the returned `Store` lives.
    /// `max_mb` is the cap from `options.state.max_mb` (plan §4.4).
    pub fn open(path: &Path, max_mb: u64) -> Result<Store, String> {
        let max_bytes = max_mb.saturating_mul(1 << 20);
        #[cfg(unix)]
        {
            use std::io::{Read, Seek, SeekFrom, Write};
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            use std::os::unix::io::AsRawFd;

            let mut o = std::fs::OpenOptions::new();
            o.create(true)
                .read(true)
                .append(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW);
            let mut file = o
                .open(path)
                .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
            let meta = file
                .metadata()
                .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
            if !meta.is_file() {
                return Err(format!(
                    "options.state.path: '{}' is not a regular file",
                    path.display()
                ));
            }
            if meta.permissions().mode() & 0o077 != 0 {
                file.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
            }
            // Single writer (plan §1 D6): a second server on this path must
            // fail to start, not silently interleave writes.
            let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if locked != 0 {
                return Err(format!(
                    "state file '{}' is in use by another server",
                    path.display()
                ));
            }

            file.seek(SeekFrom::Start(0))
                .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
            let mut buf = String::new();
            file.read_to_string(&mut buf).map_err(|e| {
                format!(
                    "options.state.path: '{}' is not valid UTF-8 or can't be read: {e}",
                    path.display()
                )
            })?;

            let (clean, torn_bytes) = recover_torn_tail(&buf);
            if torn_bytes > 0 {
                let new_len = (buf.len() - torn_bytes) as u64;
                file.set_len(new_len)
                    .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
                eprintln!(
                    "[anofox-state] {}: truncated a torn last line ({torn_bytes} bytes) — a \
                     crash or `kill -9` mid-write, recovered at start",
                    path.display()
                );
            }

            let mut state = validate_and_index(clean, max_bytes, file)
                .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
            if clean.is_empty() {
                let header = format!(
                    "{{\"anofox_state\":{CURRENT_VERSION},\"created\":\"{}\"}}\n",
                    crate::host::access::rfc3339(std::time::SystemTime::now())
                );
                state
                    .file
                    .write_all(header.as_bytes())
                    .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
                state
                    .file
                    .sync_data()
                    .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
                state.size_bytes = header.len() as u64;
            }
            Ok(Store {
                inner: Mutex::new(state),
                max_bytes,
                failed: AtomicBool::new(false),
                #[cfg(test)]
                force_fail: AtomicBool::new(false),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (path, max_bytes);
            Err("the state store needs a Unix host (flock)".into())
        }
    }

    /// Append one submission, assigning its `seq`/`ts`. `fdatasync`s before
    /// returning `Ok` (plan §1: "an acknowledged submission is durable").
    #[cfg(unix)]
    pub fn append(&self, new: NewRecord) -> Result<Record, String> {
        use std::io::Write;
        if self.failed.load(Ordering::SeqCst) {
            return Err("state store: unavailable (a previous write failed)".into());
        }
        let mut st = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let seq = st.next_seq;
        let rec = Record {
            v: CURRENT_VERSION,
            seq,
            ts: crate::host::access::rfc3339(std::time::SystemTime::now()),
            user: new.user,
            auth: new.auth,
            dashboard: new.dashboard,
            submit: new.submit,
            id: new.id,
            vars: new.vars,
        };
        let line = rec.to_line();
        let added = line.len() as u64 + LINE_OVERHEAD;
        if st.size_bytes + added > self.max_bytes {
            return Err("state store is full".into());
        }
        let wrote = (|| -> std::io::Result<()> {
            #[cfg(test)]
            if self.force_fail.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("forced test failure"));
            }
            st.file.write_all(line.as_bytes())?;
            st.file.write_all(b"\n")?;
            st.file.sync_data()?;
            Ok(())
        })();
        if let Err(e) = wrote {
            self.failed.store(true, Ordering::SeqCst);
            return Err(format!("state store write failed: {e}"));
        }
        st.size_bytes += added;
        st.next_seq = seq + 1;
        let idx = st.records.len();
        st.by_user.entry(rec.user.clone()).or_default().push(idx);
        st.records.push(rec.clone());
        Ok(rec)
    }

    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    /// The last assigned `seq` (0 before the first append).
    pub fn global_seq(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .next_seq
            .saturating_sub(1)
    }

    /// A viewer's row count (their `gen`, plan §5).
    pub fn user_count(&self, user: &str) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .by_user
            .get(user)
            .map(|v| v.len() as u64)
            .unwrap_or(0)
    }

    /// One viewer's own rows, oldest first.
    pub fn rows_for_user(&self, user: &str) -> Vec<Record> {
        let st = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        st.by_user
            .get(user)
            .map(|idxs| idxs.iter().map(|&i| st.records[i].clone()).collect())
            .unwrap_or_default()
    }

    /// Every row, newest `limit` kept (plan §5 `trainer_max_rows`).
    pub fn all_rows(&self, limit: usize) -> Vec<Record> {
        let st = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let n = st.records.len();
        st.records[n.saturating_sub(limit)..].to_vec()
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .records
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[cfg(test)]
    fn set_force_fail(&self, v: bool) {
        self.force_fail.store(v, Ordering::SeqCst);
    }
}

/// If `buf` doesn't end in `\n`, the last (possibly partial) line is a torn
/// write (plan §1: "a crash or `kill -9` mid-load test"). Returns `(buf
/// trimmed to the last complete line, bytes dropped)`; `(buf, 0)` when `buf`
/// is empty or already ends cleanly.
fn recover_torn_tail(buf: &str) -> (&str, usize) {
    if buf.is_empty() || buf.ends_with('\n') {
        return (buf, 0);
    }
    match buf.rfind('\n') {
        Some(i) => (&buf[..=i], buf.len() - (i + 1)),
        None => ("", buf.len()),
    }
}

#[cfg(unix)]
fn validate_and_index(
    clean: &str,
    max_bytes: u64,
    file: std::fs::File,
) -> Result<StoreState, String> {
    let mut lines = clean.lines();
    let mut size_bytes = 0u64;
    let mut records = Vec::new();
    let mut by_user: HashMap<String, Vec<usize>> = HashMap::new();
    let mut next_seq = 1u64;

    if let Some(header_line) = lines.next() {
        size_bytes += header_line.len() as u64 + LINE_OVERHEAD;
        let header: serde_json::Value = serde_json::from_str(header_line)
            .map_err(|e| format!("state file: bad header line: {e}"))?;
        let ver = header
            .get("anofox_state")
            .and_then(|v| v.as_u64())
            .ok_or("state file: header line is missing 'anofox_state'")?;
        if ver != CURRENT_VERSION as u64 {
            return Err(format!(
                "state file: unsupported version {ver} (this build writes {CURRENT_VERSION})"
            ));
        }
        if header.get("created").and_then(|v| v.as_str()).is_none() {
            return Err("state file: header line is missing 'created'".into());
        }

        for (i, line) in lines.enumerate() {
            size_bytes += line.len() as u64 + LINE_OVERHEAD;
            if size_bytes > max_bytes {
                return Err(format!(
                    "state file exceeds max_mb (line {}) — the operator must inspect / \
                     grow max_mb",
                    i + 2
                ));
            }
            let v: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| format!("state file: malformed line {}: {e}", i + 2))?;
            let rec = Record::from_json(&v)
                .map_err(|e| format!("state file: malformed line {}: {e}", i + 2))?;
            if rec.v != CURRENT_VERSION {
                return Err(format!(
                    "state file: line {} has unsupported version {}",
                    i + 2,
                    rec.v
                ));
            }
            if rec.seq < next_seq {
                return Err(format!(
                    "state file: line {} has seq {} which is not strictly increasing \
                     (expected > {})",
                    i + 2,
                    rec.seq,
                    next_seq - 1
                ));
            }
            next_seq = rec.seq + 1;
            by_user
                .entry(rec.user.clone())
                .or_default()
                .push(records.len());
            records.push(rec);
        }
    }

    Ok(StoreState {
        file,
        records,
        by_user,
        next_seq,
        size_bytes,
    })
}

// ──────────────────────────────────────────────────────────── rate limit ──

/// A per-user token bucket (plan §4.4: `rate.per_minute`/`rate.burst`).
struct TokenBucket {
    tokens: f64,
    capacity: f64,
    refill_per_sec: f64,
    last: std::time::Instant,
}

impl TokenBucket {
    fn new(capacity: u32, refill_per_sec: f64, now: std::time::Instant) -> Self {
        TokenBucket {
            tokens: capacity as f64,
            capacity: capacity as f64,
            refill_per_sec,
            last: now,
        }
    }

    /// `true` and takes one token, or `false` (no change) when empty.
    fn try_take(&mut self, now: std::time::Instant) -> bool {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + dt * self.refill_per_sec).min(self.capacity);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Seconds until one token is available (`Retry-After`, plan §4.4).
    fn retry_after_secs(&self) -> u64 {
        if self.refill_per_sec <= 0.0 {
            return u64::MAX;
        }
        let need = (1.0 - self.tokens).max(0.0);
        (need / self.refill_per_sec).ceil().max(1.0) as u64
    }
}

struct RateLimiterState {
    buckets: HashMap<String, TokenBucket>,
    /// Insertion order, for "evict oldest" once `capacity_users` is hit —
    /// plan §4.4's bound is not a strict LRU (access doesn't reorder it).
    order: std::collections::VecDeque<String>,
}

/// A per-user token bucket, bounded to `capacity_users` distinct users
/// (oldest evicted first) so an unbounded set of distinct identities can't
/// grow this map forever (threat #10).
pub struct RateLimiter {
    per_minute: u32,
    burst: u32,
    capacity_users: usize,
    inner: Mutex<RateLimiterState>,
}

impl RateLimiter {
    pub fn new(per_minute: u32, burst: u32, capacity_users: usize) -> Self {
        RateLimiter {
            per_minute: per_minute.max(1),
            burst: burst.max(1),
            capacity_users: capacity_users.max(1),
            inner: Mutex::new(RateLimiterState {
                buckets: HashMap::new(),
                order: std::collections::VecDeque::new(),
            }),
        }
    }

    /// `Ok(())`: the request may proceed (a token was taken). `Err(n)`: rate
    /// limited, retry in `n` seconds.
    pub fn check(&self, user: &str, now: std::time::Instant) -> Result<(), u64> {
        let mut st = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if !st.buckets.contains_key(user) {
            if st.buckets.len() >= self.capacity_users {
                if let Some(oldest) = st.order.pop_front() {
                    st.buckets.remove(&oldest);
                }
            }
            let refill_per_sec = self.per_minute as f64 / 60.0;
            st.buckets.insert(
                user.to_string(),
                TokenBucket::new(self.burst, refill_per_sec, now),
            );
            st.order.push_back(user.to_string());
        }
        let b = st.buckets.get_mut(user).expect("just inserted");
        if b.try_take(now) {
            Ok(())
        } else {
            Err(b.retry_after_secs())
        }
    }

    pub fn tracked_users(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .buckets
            .len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── StateOptions::parse ─────────────────────────────────────────────

    fn full_example() -> serde_json::Value {
        json!({
            "path": "/var/lib/anofox/state.jsonl",
            "dashboards": ["tutorial", "onboarding-quiz"],
            "read": ["my-progress"],
            "trainers": {"dashboards": ["progress"], "groups": ["trainers"], "users": []},
            "text_max_chars": 1000,
            "rate": {"per_minute": 6, "burst": 3},
            "max_per_viewer": 1000,
            "max_mb": 256,
            "trainer_max_rows": 200000
        })
    }

    #[test]
    fn parses_the_full_example_from_the_plan() {
        let o = StateOptions::parse(&full_example()).unwrap();
        assert_eq!(o.path, PathBuf::from("/var/lib/anofox/state.jsonl"));
        assert_eq!(o.dashboards, vec!["tutorial", "onboarding-quiz"]);
        assert_eq!(o.read, vec!["my-progress"]);
        let t = o.trainers.unwrap();
        assert_eq!(t.dashboards, vec!["progress"]);
        assert_eq!(t.groups, vec!["trainers"]);
        assert!(t.users.is_empty());
        assert_eq!(o.text_max_chars, 1000);
        assert_eq!(o.rate_per_minute, 6);
        assert_eq!(o.rate_burst, 3);
        assert_eq!(o.max_per_viewer, 1000);
        assert_eq!(o.max_mb, 256);
        assert_eq!(o.trainer_max_rows, 200_000);
    }

    #[test]
    fn minimal_example_gets_defaults() {
        let o = StateOptions::parse(&json!({"path": "a.jsonl", "dashboards": ["d"]})).unwrap();
        assert_eq!(o.text_max_chars, DEFAULT_TEXT_MAX_CHARS);
        assert_eq!(o.rate_per_minute, DEFAULT_RATE_PER_MINUTE);
        assert_eq!(o.rate_burst, DEFAULT_RATE_BURST);
        assert_eq!(o.max_per_viewer, DEFAULT_MAX_PER_VIEWER);
        assert_eq!(o.max_mb, DEFAULT_MAX_MB);
        assert_eq!(o.trainer_max_rows, DEFAULT_TRAINER_MAX_ROWS);
        assert!(o.trainers.is_none());
    }

    #[test]
    fn path_is_required() {
        let e = StateOptions::parse(&json!({"dashboards": ["d"]})).unwrap_err();
        assert!(e.contains("path is required"), "{e}");
    }

    #[test]
    fn not_an_object_is_refused() {
        assert!(StateOptions::parse(&json!("nope")).is_err());
        assert!(StateOptions::parse(&json!([1, 2])).is_err());
    }

    #[test]
    fn unknown_top_level_key_is_refused() {
        let e = StateOptions::parse(&json!({"path": "a.jsonl", "dashboards": ["d"], "nope": 1}))
            .unwrap_err();
        assert!(e.contains("unknown key 'nope'"), "{e}");
    }

    #[test]
    fn unknown_trainers_key_is_refused() {
        let e = StateOptions::parse(&json!({
            "path": "a.jsonl",
            "trainers": {"groups": ["t"], "nope": 1}
        }))
        .unwrap_err();
        assert!(e.contains("trainers: unknown key 'nope'"), "{e}");
    }

    #[test]
    fn unknown_rate_key_is_refused() {
        let e = StateOptions::parse(&json!({
            "path": "a.jsonl", "dashboards": ["d"], "rate": {"per_minute": 1, "nope": 1}
        }))
        .unwrap_err();
        assert!(e.contains("rate: unknown key 'nope'"), "{e}");
    }

    #[test]
    fn needs_at_least_one_dashboard_list() {
        let e = StateOptions::parse(&json!({"path": "a.jsonl"})).unwrap_err();
        assert!(
            e.contains("at least one of dashboards, read or trainers.dashboards"),
            "{e}"
        );
    }

    #[test]
    fn read_alone_is_enough() {
        assert!(StateOptions::parse(&json!({"path": "a.jsonl", "read": ["p"]})).is_ok());
    }

    #[test]
    fn trainers_without_groups_or_users_is_refused() {
        let e = StateOptions::parse(&json!({
            "path": "a.jsonl", "trainers": {"dashboards": ["progress"]}
        }))
        .unwrap_err();
        assert!(e.contains("needs groups or users"), "{e}");
    }

    #[test]
    fn dashboard_in_both_lists_is_refused() {
        let e = StateOptions::parse(&json!({
            "path": "a.jsonl",
            "dashboards": ["progress"],
            "trainers": {"dashboards": ["progress"], "groups": ["t"]}
        }))
        .unwrap_err();
        assert!(
            e.contains("in both dashboards and trainers.dashboards"),
            "{e}"
        );
    }

    #[test]
    fn text_max_chars_is_clamped_not_rejected() {
        let o = StateOptions::parse(&json!({
            "path": "a.jsonl", "dashboards": ["d"], "text_max_chars": 1_000_000
        }))
        .unwrap();
        assert_eq!(o.text_max_chars, MAX_TEXT_MAX_CHARS);
    }

    #[test]
    fn zero_or_negative_numbers_are_refused() {
        assert!(StateOptions::parse(&json!({
            "path": "a.jsonl", "dashboards": ["d"], "max_mb": 0
        }))
        .is_err());
        assert!(StateOptions::parse(&json!({
            "path": "a.jsonl", "dashboards": ["d"], "max_per_viewer": 0
        }))
        .is_err());
        assert!(StateOptions::parse(&json!({
            "path": "a.jsonl", "dashboards": ["d"], "text_max_chars": -1
        }))
        .is_err());
    }

    #[test]
    fn dashboards_must_be_an_array_of_strings() {
        assert!(StateOptions::parse(&json!({"path": "a.jsonl", "dashboards": "d"})).is_err());
        assert!(StateOptions::parse(&json!({"path": "a.jsonl", "dashboards": [1, 2]})).is_err());
    }

    // ── check_path_rules / validate_start ───────────────────────────────

    /// A per-test throwaway directory. Deliberately **not** under
    /// `std::env::temp_dir()` — that's exactly what
    /// `path_inside_system_temp_dir_is_refused` checks `check_path_rules`
    /// refuses, so fixtures live under the crate's own `target/` instead.
    fn tmp(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("state-test-fixtures")
            .join(format!("{name}-{}-{}", std::process::id(), now_nanos()))
    }

    fn now_nanos() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    fn mkdir(p: &Path) {
        std::fs::create_dir_all(p).unwrap();
    }

    #[test]
    fn valid_path_passes() {
        let root = tmp("valid");
        mkdir(&root);
        let dash = root.join("dashboards");
        mkdir(&dash);
        let store_dir = root.join("store");
        mkdir(&store_dir);
        assert!(check_path_rules(&store_dir.join("state.jsonl"), None, &dash).is_ok());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn path_inside_data_is_refused() {
        let root = tmp("in-data");
        mkdir(&root);
        let data = root.join("data");
        mkdir(&data);
        let dash = root.join("dashboards");
        mkdir(&dash);
        let e =
            check_path_rules(&data.join("state.jsonl"), Some(data.as_path()), &dash).unwrap_err();
        assert!(e.contains("options.data"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn data_inside_states_dir_is_also_refused() {
        let root = tmp("data-in-state");
        mkdir(&root);
        let state_dir = root.join("store");
        mkdir(&state_dir);
        let data = state_dir.join("data");
        mkdir(&data);
        let dash = root.join("dashboards");
        mkdir(&dash);
        let e = check_path_rules(&state_dir.join("state.jsonl"), Some(data.as_path()), &dash)
            .unwrap_err();
        assert!(e.contains("options.data"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn path_inside_dashboard_dir_is_refused() {
        let root = tmp("in-dash");
        mkdir(&root);
        let dash = root.join("dashboards");
        mkdir(&dash);
        let e = check_path_rules(&dash.join("state.jsonl"), None, &dash).unwrap_err();
        assert!(e.contains("dashboard folder"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn path_inside_system_temp_dir_is_refused() {
        let dash = tmp("temp-dash");
        mkdir(&dash);
        // A *subdirectory* of the temp dir, not the bare temp dir itself —
        // `too_broad` already refuses the bare temp dir as "a system
        // directory" (still a refusal, just a different message); this
        // exercises the dedicated temp-dir containment rule instead.
        let sub = std::env::temp_dir().join(format!("anofox-state-subdir-{}", now_nanos()));
        mkdir(&sub);
        let state_path = sub.join("state.jsonl");
        let e = check_path_rules(&state_path, None, &dash).unwrap_err();
        assert!(e.contains("temp directory"), "{e}");
        std::fs::remove_dir_all(&sub).ok();
        std::fs::remove_dir_all(&dash).ok();
    }

    #[test]
    fn too_broad_root_is_refused() {
        let dash = tmp("broad-dash");
        mkdir(&dash);
        // The filesystem root is always too broad.
        let e = check_path_rules(Path::new("/state.jsonl"), None, &dash).unwrap_err();
        assert!(e.contains("refusing"), "{e}");
        std::fs::remove_dir_all(&dash).ok();
    }

    #[cfg(unix)]
    #[test]
    fn group_writable_dir_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let root = tmp("group-writable");
        mkdir(&root);
        let dash = root.join("dashboards");
        mkdir(&dash);
        let store_dir = root.join("store");
        mkdir(&store_dir);
        std::fs::set_permissions(&store_dir, std::fs::Permissions::from_mode(0o775)).unwrap();
        let e = check_path_rules(&store_dir.join("state.jsonl"), None, &dash).unwrap_err();
        assert!(e.contains("writable"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_parent_dir_is_refused() {
        let root = tmp("missing-parent");
        // Not created.
        let dash = tmp("missing-parent-dash");
        mkdir(&dash);
        assert!(check_path_rules(&root.join("nope").join("state.jsonl"), None, &dash).is_err());
        std::fs::remove_dir_all(&dash).ok();
    }

    fn base_ctx<'a>(
        dashboard_dir: &'a Path,
        known: &'a [String],
        reserved: &'a [String],
    ) -> StartContext<'a> {
        StartContext {
            has_auth: true,
            embed_without_frame_ancestors: false,
            data_dir: None,
            dashboard_dir,
            known_dashboards: known,
            reserved_names: reserved,
        }
    }

    #[test]
    fn validate_start_refuses_without_auth() {
        let root = tmp("no-auth");
        mkdir(&root);
        let dash = root.join("dashboards");
        mkdir(&dash);
        let o = StateOptions::parse(&json!({
            "path": root.join("store").join("s.jsonl").to_str().unwrap(),
            "dashboards": ["tutorial"]
        }))
        .unwrap();
        mkdir(&root.join("store"));
        let known = vec!["tutorial".to_string()];
        let mut ctx = base_ctx(&dash, &known, &[]);
        ctx.has_auth = false;
        let e = o.validate_start(&ctx).unwrap_err();
        assert!(e.contains("options.auth"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn validate_start_refuses_embed_without_frame_ancestors() {
        let root = tmp("embed");
        mkdir(&root);
        let dash = root.join("dashboards");
        mkdir(&dash);
        mkdir(&root.join("store"));
        let o = StateOptions::parse(&json!({
            "path": root.join("store").join("s.jsonl").to_str().unwrap(),
            "dashboards": ["tutorial"]
        }))
        .unwrap();
        let known = vec!["tutorial".to_string()];
        let mut ctx = base_ctx(&dash, &known, &[]);
        ctx.embed_without_frame_ancestors = true;
        let e = o.validate_start(&ctx).unwrap_err();
        assert!(e.contains("frame_ancestors"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn validate_start_refuses_unknown_dashboard() {
        let root = tmp("unknown-dash");
        mkdir(&root);
        let dash = root.join("dashboards");
        mkdir(&dash);
        mkdir(&root.join("store"));
        let o = StateOptions::parse(&json!({
            "path": root.join("store").join("s.jsonl").to_str().unwrap(),
            "dashboards": ["tutorial"]
        }))
        .unwrap();
        let known: Vec<String> = vec![]; // "tutorial" not served
        let ctx = base_ctx(&dash, &known, &[]);
        let e = o.validate_start(&ctx).unwrap_err();
        assert!(e.contains("tutorial"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn validate_start_refuses_reserved_table_name() {
        let root = tmp("reserved-name");
        mkdir(&root);
        let dash = root.join("dashboards");
        mkdir(&dash);
        mkdir(&root.join("store"));
        let o = StateOptions::parse(&json!({
            "path": root.join("store").join("s.jsonl").to_str().unwrap(),
            "dashboards": ["tutorial"]
        }))
        .unwrap();
        let known = vec!["tutorial".to_string()];
        let reserved = vec!["viewer_state".to_string()];
        let ctx = base_ctx(&dash, &known, &reserved);
        let e = o.validate_start(&ctx).unwrap_err();
        assert!(e.contains("viewer_state"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn validate_start_passes_the_good_case() {
        let root = tmp("good");
        mkdir(&root);
        let dash = root.join("dashboards");
        mkdir(&dash);
        mkdir(&root.join("store"));
        let o = StateOptions::parse(&json!({
            "path": root.join("store").join("s.jsonl").to_str().unwrap(),
            "dashboards": ["tutorial"]
        }))
        .unwrap();
        let known = vec!["tutorial".to_string()];
        let ctx = base_ctx(&dash, &known, &[]);
        assert!(o.validate_start(&ctx).is_ok());
        std::fs::remove_dir_all(&root).ok();
    }

    // ── Store ────────────────────────────────────────────────────────────

    #[cfg(unix)]
    fn store_path(name: &str) -> PathBuf {
        let root = tmp(name);
        mkdir(&root);
        root.join("state.jsonl")
    }

    #[cfg(unix)]
    fn new_rec(user: &str, vars_json: serde_json::Value) -> NewRecord {
        NewRecord {
            user: user.to_string(),
            auth: "oidc".to_string(),
            dashboard: "tutorial".to_string(),
            submit: "quiz_1".to_string(),
            id: format!("id-{}", now_nanos()),
            vars: vars_json.as_object().cloned().unwrap_or_default(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn append_and_reload_round_trips() {
        let p = store_path("roundtrip");
        {
            let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
            let r1 = s.append(new_rec("alice", json!({"q1": "B"}))).unwrap();
            assert_eq!(r1.seq, 1);
            let r2 = s.append(new_rec("bob", json!({"q1": "A"}))).unwrap();
            assert_eq!(r2.seq, 2);
            assert_eq!(s.user_count("alice"), 1);
            assert_eq!(s.global_seq(), 2);
        }
        // Reopen: the lock was released when `s` dropped.
        let s2 = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        assert_eq!(s2.len(), 2);
        assert_eq!(s2.global_seq(), 2);
        let alice_rows = s2.rows_for_user("alice");
        assert_eq!(alice_rows.len(), 1);
        assert_eq!(alice_rows[0].vars.get("q1").unwrap(), "B");
        // A third append after reload continues the sequence.
        let r3 = s2.append(new_rec("alice", json!({"q1": "C"}))).unwrap();
        assert_eq!(r3.seq, 3);
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn injected_control_characters_stay_one_line() {
        let p = store_path("injection");
        let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        let nasty = "line1\nline2\r\t\"quote\"\0null";
        s.append(new_rec("alice", json!({"note": nasty}))).unwrap();
        drop(s);
        let text = std::fs::read_to_string(&p).unwrap();
        // Header + exactly one data line: the injected newline did not
        // split the record into two lines.
        assert_eq!(text.lines().count(), 2, "{text:?}");
        let s2 = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        let rows = s2.rows_for_user("alice");
        assert_eq!(rows[0].vars.get("note").unwrap().as_str().unwrap(), nasty);
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn torn_last_line_is_truncated_and_recovered() {
        let p = store_path("torn");
        {
            let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
            s.append(new_rec("alice", json!({"q1": "B"}))).unwrap();
        }
        // Append a partial line with no trailing newline, bypassing Store
        // (simulating a `kill -9` mid-write).
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            f.write_all(br#"{"v":1,"seq":2,"ts":"2026-01-01T00:00:00.000Z","user":"bob","auth":"oidc","dashboard":"t","submit":"q","id":"x","vars":{"#).unwrap();
        }
        let before = std::fs::metadata(&p).unwrap().len();
        let s2 = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        let after = std::fs::metadata(&p).unwrap().len();
        assert!(after < before, "torn tail should have been truncated");
        assert_eq!(s2.len(), 1, "only the complete record survives");
        assert_eq!(s2.global_seq(), 1);
        // The store still works after recovery.
        let r = s2.append(new_rec("carol", json!({"q1": "A"}))).unwrap();
        assert_eq!(r.seq, 2);
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn malformed_middle_line_refuses_to_open() {
        let p = store_path("malformed");
        {
            let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
            s.append(new_rec("alice", json!({"q1": "B"}))).unwrap();
        }
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            writeln!(f, "not even json").unwrap();
        }
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(e.contains("malformed line"), "{e}");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn non_increasing_seq_refuses_to_open() {
        let p = store_path("seq-order");
        {
            let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
            s.append(new_rec("alice", json!({"q1": "B"}))).unwrap();
        }
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            // Same seq as the first record (1), not strictly increasing.
            writeln!(f, r#"{{"v":1,"seq":1,"ts":"2026-01-01T00:00:00.000Z","user":"bob","auth":"oidc","dashboard":"t","submit":"q","id":"x","vars":{{}}}}"#).unwrap();
        }
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(e.contains("not strictly increasing"), "{e}");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn unknown_version_refuses_to_open() {
        let p = store_path("unknown-v");
        {
            let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
            s.append(new_rec("alice", json!({"q1": "B"}))).unwrap();
        }
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            writeln!(f, r#"{{"v":99,"seq":2,"ts":"2026-01-01T00:00:00.000Z","user":"bob","auth":"oidc","dashboard":"t","submit":"q","id":"x","vars":{{}}}}"#).unwrap();
        }
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(e.contains("unsupported version"), "{e}");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn unknown_header_version_refuses_to_open() {
        let p = store_path("bad-header");
        mkdir(p.parent().unwrap());
        std::fs::write(&p, b"{\"anofox_state\":2,\"created\":\"x\"}\n").unwrap();
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(e.contains("unsupported version"), "{e}");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn oversize_file_refuses_to_open() {
        let p = store_path("oversize");
        {
            let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
            for i in 0..50 {
                s.append(new_rec(&format!("user{i}"), json!({"q1": "B"})))
                    .unwrap();
            }
        }
        // Reopen with a max_mb of 0 (clamped to at least some small bound by
        // passing an explicit tiny byte budget via max_mb=0 is refused by
        // StateOptions::parse, so exercise the store directly with a byte
        // cap too small for the file that already exists).
        let e = Store::open(&p, 0).unwrap_err();
        assert!(e.contains("max_mb") || e.contains("exceeds"), "{e}");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn second_open_on_same_path_fails() {
        let p = store_path("flock");
        let s1 = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(e.contains("in use by another server"), "{e}");
        drop(s1);
        // Lock released on drop: a third open now succeeds.
        assert!(Store::open(&p, DEFAULT_MAX_MB).is_ok());
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn io_error_while_appending_flips_store_to_failed() {
        let p = store_path("io-error");
        let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        s.append(new_rec("alice", json!({"q1": "B"}))).unwrap();
        s.set_force_fail(true);
        let e = s.append(new_rec("bob", json!({"q1": "A"}))).unwrap_err();
        assert!(e.contains("write failed"), "{e}");
        assert!(s.is_failed());
        // Every later append refuses too — never retried silently.
        let e2 = s.append(new_rec("carol", json!({"q1": "A"}))).unwrap_err();
        assert!(e2.contains("unavailable"), "{e2}");
        drop(s);
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn store_full_refuses_further_appends() {
        let p = store_path("full");
        let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        // max_bytes derives from max_mb (MiB); open a second handle's worth
        // of accounting isn't needed — call append in a loop against a
        // store opened with a tiny budget instead.
        drop(s);
        std::fs::remove_file(&p).ok();
        let s = Store::open(&p, 0).unwrap_or_else(|_| {
            // max_mb=0 means max_bytes=0: even the header write at open
            // time would be "too large", so Store::open itself may refuse.
            // Fall back to a 1 MiB budget and fill it instead.
            Store::open(&p, 1).unwrap()
        });
        let big = "x".repeat(2_000_000);
        let mut filled = false;
        for i in 0..1000 {
            if s.append(new_rec(&format!("u{i}"), json!({"note": big})))
                .is_err()
            {
                filled = true;
                break;
            }
        }
        assert!(
            filled,
            "expected the store to refuse once max_mb was reached"
        );
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    // ── RateLimiter ──────────────────────────────────────────────────────

    #[test]
    fn burst_then_refill() {
        let rl = RateLimiter::new(60, 3, 100); // 1/sec refill, burst 3
        let t0 = std::time::Instant::now();
        assert!(rl.check("alice", t0).is_ok());
        assert!(rl.check("alice", t0).is_ok());
        assert!(rl.check("alice", t0).is_ok());
        let err = rl.check("alice", t0).unwrap_err();
        assert!(err >= 1);
        // One second later, one token has refilled.
        let t1 = t0 + std::time::Duration::from_secs(1);
        assert!(rl.check("alice", t1).is_ok());
        assert!(rl.check("alice", t1).is_err());
    }

    #[test]
    fn users_are_independent() {
        let rl = RateLimiter::new(60, 1, 100);
        let t0 = std::time::Instant::now();
        assert!(rl.check("alice", t0).is_ok());
        assert!(rl.check("alice", t0).is_err());
        assert!(rl.check("bob", t0).is_ok());
    }

    #[test]
    fn bounded_map_evicts_oldest() {
        let rl = RateLimiter::new(60, 1, 2);
        let t0 = std::time::Instant::now();
        rl.check("alice", t0).ok();
        rl.check("bob", t0).ok();
        assert_eq!(rl.tracked_users(), 2);
        rl.check("carol", t0).ok();
        // Bounded to 2 users: alice (the oldest) was evicted, so alice gets
        // a fresh bucket (full burst) instead of inheriting her exhausted one.
        assert_eq!(rl.tracked_users(), 2);
        assert!(rl.check("alice", t0).is_ok());
    }

    #[test]
    fn retry_after_is_at_least_one_second() {
        let rl = RateLimiter::new(6, 1, 10); // 0.1/sec refill
        let t0 = std::time::Instant::now();
        rl.check("alice", t0).ok();
        let err = rl.check("alice", t0).unwrap_err();
        assert!(err >= 1);
    }
}
