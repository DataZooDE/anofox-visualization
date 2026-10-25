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
/// itself a refusal, not a "skip the check" — including a `dashboard_dir`
/// that doesn't canonicalise, which used to be skipped (failing open,
/// against this function's own contract; review of #530).
///
/// Checked against the path's **parent** directory, which must already
/// exist (the store file itself is created by [`Store::open`] if missing).
/// This is the config-time advisory check (a good error message before a
/// server even tries to create the store); [`Store::open`] enforces the
/// filesystem-level safety (symlinks, ownership, the TOCTOU window) itself,
/// independently, since by the time it runs this check may be stale (S2:
/// there is a time gap between `validate_start` and `Store::open`).
pub fn check_path_rules(
    path: &Path,
    data_dir: Option<&Path>,
    dashboard_dir: &Path,
) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("options.state.path must be an absolute path".into());
    }
    if path.file_name().is_none() {
        return Err("options.state.path must name a file".into());
    }
    let parent_raw = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("/"),
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
    // `options.data` is assumed already canonical (it was canonicalised by
    // `LockedOptions::parse` itself) but re-canonicalised here defensively
    // rather than trusted — a caller (or a future one) passing a raw path
    // by mistake would otherwise silently miss this check.
    if let Some(data) = data_dir {
        let data = std::fs::canonicalize(data)
            .map_err(|e| format!("options.data: '{}': {e}", data.display()))?;
        if parent.starts_with(&data) || data.starts_with(&parent) {
            return Err(
                "options.state.path must not be inside options.data (it would become \
                 readable through allowed_directories), and options.data must not be inside \
                 the state file's directory"
                    .into(),
            );
        }
    }
    // Was `if let Ok(dash) = … { … }` — a `dashboard_dir` that doesn't
    // canonicalise used to be silently skipped, failing *open* against
    // this function's documented "fails closed".
    let dash = std::fs::canonicalize(dashboard_dir).map_err(|e| {
        format!(
            "options.state: dashboard folder '{}': {e}",
            dashboard_dir.display()
        )
    })?;
    if parent.starts_with(&dash) {
        return Err("options.state.path must not be inside the dashboard folder".into());
    }
    // The snapshot's own scratch files live under the system temp dir, and
    // `/tmp`, `/var/tmp`, `/dev/shm` are shared, world-writable-by-design
    // scratch space regardless of `$TMPDIR` — refuse all of them by name,
    // not only whatever `std::env::temp_dir()` resolves to right now (a
    // `$TMPDIR` pointing elsewhere would otherwise leave the literal `/tmp`
    // unchecked; review of #530 #4).
    let mut forbidden_roots: Vec<PathBuf> = Vec::new();
    for p in ["/tmp", "/var/tmp", "/dev/shm"] {
        if let Ok(c) = std::fs::canonicalize(p) {
            forbidden_roots.push(c);
        }
    }
    if let Ok(c) = std::fs::canonicalize(std::env::temp_dir()) {
        forbidden_roots.push(c);
    }
    if let Some(root) = forbidden_roots.iter().find(|r| parent.starts_with(r)) {
        return Err(format!(
            "options.state.path must not be inside '{}' (a shared scratch/temp directory, \
             regardless of $TMPDIR)",
            root.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(&parent)
            .map_err(|e| format!("options.state.path: '{}': {e}", parent.display()))?;
        if meta.mode() & 0o022 != 0 {
            return Err(format!(
                "options.state.path: '{}' is group- or world-writable — point it at a \
                 directory only the server can write",
                parent.display()
            ));
        }
        // Review of #530 #4: the parent's *owner* wasn't checked at all — a
        // 0755 directory created by another local user would otherwise
        // pass. Accepted owners: this process (euid) or root.
        let euid = unsafe { libc::geteuid() };
        if meta.uid() != euid && meta.uid() != 0 {
            return Err(format!(
                "options.state.path: '{}' is not owned by this server (or root) — a \
                 directory another local account controls could replace its contents",
                parent.display()
            ));
        }
        // Every ancestor up to the root: a loosely-permissioned one (no
        // sticky bit) could be renamed/replaced by another local account,
        // swapping out the directory this check just approved before
        // `Store::open` gets to it.
        let mut cur: &Path = &parent;
        loop {
            let ameta =
                std::fs::symlink_metadata(cur).map_err(|e| format!("'{}': {e}", cur.display()))?;
            let mode = ameta.mode();
            let loosely_writable = mode & 0o022 != 0 && mode & 0o1000 == 0;
            if loosely_writable {
                return Err(format!(
                    "options.state.path: '{}' is group- or world-writable without the sticky \
                     bit — a local attacker could replace it (or something under it)",
                    cur.display()
                ));
            }
            match cur.parent() {
                Some(p) if p != cur && !p.as_os_str().is_empty() => cur = p,
                _ => break,
            }
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
    /// Kept as `u64` (not `u32`, the wire type `v` is specified as) so a
    /// huge or negative-looking `v` in a hand-edited line can't silently
    /// truncate into looking like `CURRENT_VERSION` (review of #530: nits).
    pub v: u64,
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

    /// The exact field set a line may carry — anything else fails closed
    /// (review of #530: `from_json` used to silently drop unknown fields).
    const KNOWN_FIELDS: &'static [&'static str] = &[
        "v",
        "seq",
        "ts",
        "user",
        "auth",
        "dashboard",
        "submit",
        "id",
        "vars",
    ];

    fn from_json(v: &serde_json::Value) -> Result<Record, String> {
        let o = v.as_object().ok_or("not a JSON object")?;
        if let Some(unknown) = o.keys().find(|k| !Self::KNOWN_FIELDS.contains(&k.as_str())) {
            return Err(format!("unknown field '{unknown}'"));
        }
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
            v: v_ver,
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

/// `clean`'s content is at least a prefix of our own header's literal
/// start, or vice versa — the only basis on which an entirely-unterminated
/// buffer (no `\n` anywhere) is trusted as *our* torn write rather than
/// some unrelated file (review of #530, the BLOCKER).
const HEADER_MAGIC: &str = "{\"anofox_state\"";

/// Is `owner_uid` an acceptable owner for a path [`Store::open`] touches —
/// this process (`euid`) or root? Pulled out of the two call sites (the
/// parent directory and the file itself) so the decision is unit-tested
/// directly: fabricating a *real* file owned by a different uid needs
/// root, which this sandbox doesn't have (confirmed by hand with a
/// Docker bind-mount, owned by an unprivileged container uid, which
/// `Store::open` correctly refused — not wired into the automated suite,
/// since relying on Docker being available in CI for one edge case
/// seemed the wrong trade-off; this pure function is what the real check
/// actually decides on).
fn owner_is_acceptable(owner_uid: u32, euid: u32) -> bool {
    owner_uid == euid || owner_uid == 0
}

fn looks_like_our_header_prefix(buf: &str) -> bool {
    if buf.len() <= HEADER_MAGIC.len() {
        HEADER_MAGIC.starts_with(buf)
    } else {
        buf.starts_with(HEADER_MAGIC)
    }
}

impl Store {
    /// Open (creating if absent) and validate the whole file, holding an
    /// exclusive `flock` for as long as the returned `Store` lives.
    /// `max_mb` is the cap from `options.state.max_mb` (plan §4.4).
    ///
    /// Opens the **parent directory** first (`O_DIRECTORY|O_NOFOLLOW`) and
    /// `openat`s the file under that held descriptor, so there is no
    /// window between resolving `path` and creating the file in which it
    /// could be swapped for a symlink (review of #530 #3: "Check/open
    /// TOCTOU"). This repeats the ownership/permission checks
    /// [`check_path_rules`] already did at config time — independently,
    /// since by the time this runs that check may be stale (S2 adds a gap
    /// between `validate_start` and `open`).
    #[cfg(unix)]
    pub fn open(path: &Path, max_mb: u64) -> Result<Store, String> {
        use std::ffi::CString;
        use std::io::{Read, Write};
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        use std::os::unix::io::{AsRawFd, FromRawFd};

        let max_bytes = max_mb.saturating_mul(1 << 20);
        if !path.is_absolute() {
            return Err("options.state.path must be an absolute path".into());
        }
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty())
            .ok_or_else(|| format!("options.state.path: '{}' must name a file", path.display()))?;
        let parent_raw = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("/"),
        };

        struct Fd(i32);
        impl Drop for Fd {
            fn drop(&mut self) {
                unsafe {
                    libc::close(self.0);
                }
            }
        }

        let dir_cstr = CString::new(parent_raw.as_os_str().as_bytes())
            .map_err(|_| "options.state.path: the directory path has a NUL byte".to_string())?;
        let dirfd = unsafe {
            libc::open(
                dir_cstr.as_ptr(),
                libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if dirfd < 0 {
            return Err(format!(
                "options.state.path: '{}': {}",
                parent_raw.display(),
                std::io::Error::last_os_error()
            ));
        }
        let dirfd = Fd(dirfd);

        let mut dstat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(dirfd.0, &mut dstat) } != 0 {
            return Err(format!(
                "options.state.path: '{}': {}",
                parent_raw.display(),
                std::io::Error::last_os_error()
            ));
        }
        let euid = unsafe { libc::geteuid() };
        if !owner_is_acceptable(dstat.st_uid, euid) {
            return Err(format!(
                "options.state.path: '{}' is not owned by this server (or root)",
                parent_raw.display()
            ));
        }
        if dstat.st_mode as u32 & 0o022 != 0 {
            return Err(format!(
                "options.state.path: '{}' is group- or world-writable",
                parent_raw.display()
            ));
        }

        let name_cstr = CString::new(file_name)
            .map_err(|_| "options.state.path: the file name has a NUL byte".to_string())?;
        let fd = unsafe {
            libc::openat(
                dirfd.0,
                name_cstr.as_ptr(),
                libc::O_RDWR | libc::O_CREAT | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(format!(
                "options.state.path: '{}': {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };

        let fmeta = file
            .metadata()
            .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
        if !fmeta.is_file() {
            return Err(format!(
                "options.state.path: '{}' is not a regular file",
                path.display()
            ));
        }
        if fmeta.nlink() > 1 {
            return Err(format!(
                "options.state.path: '{}' has more than one hard link — refusing a file \
                 that might be shared with something else",
                path.display()
            ));
        }
        if !owner_is_acceptable(fmeta.uid(), euid) {
            return Err(format!(
                "options.state.path: '{}' is not owned by this server (or root)",
                path.display()
            ));
        }

        // Single writer (plan §1 D6): a second server on this path must
        // fail to start, not silently interleave writes. `flock` is
        // unreliable over NFS/CIFS (review of #530: worth a line in the
        // operator-facing docs once they exist, S5) — `options.state.path`
        // should be local storage.
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if locked != 0 {
            return Err(format!(
                "state file '{}' is in use by another server",
                path.display()
            ));
        }

        // Size cap checked against metadata before reading the whole file
        // into memory (review of #530 #6).
        if fmeta.len() > max_bytes {
            return Err(format!(
                "options.state.path: '{}' is {} bytes, over max_mb ({max_mb} MiB) — the \
                 operator must inspect / grow max_mb",
                path.display(),
                fmeta.len()
            ));
        }

        let mut buf = String::new();
        file.read_to_string(&mut buf).map_err(|e| {
            format!(
                "options.state.path: '{}' is not valid UTF-8 or can't be read: {e}",
                path.display()
            )
        })?;

        // BLOCKER fix (review of #530 #1): validate *before* mutating
        // anything. The old order truncated a torn tail and chmodded the
        // file ahead of checking its header — pointing `options.state.path`
        // at any existing file without a trailing newline (a secret, a
        // config file) silently destroyed it. Nothing below this point
        // touches the file until `index` is known good.
        let (clean, torn_bytes) = recover_torn_tail(&buf);
        if clean.is_empty() && !buf.is_empty() && !looks_like_our_header_prefix(&buf) {
            return Err(format!(
                "options.state.path: '{}' doesn't look like a state store (no complete, \
                 recognisable header line) — refusing to touch a file that might not be ours",
                path.display()
            ));
        }
        let index = validate_lines(clean, max_bytes)
            .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;

        // Only now, having confirmed `clean` is a valid (possibly empty)
        // store prefix, mutate the file: drop a torn tail, then tighten
        // loose permissions (never the other way around, and never when
        // validation above failed).
        if torn_bytes > 0 {
            file.set_len(clean.len() as u64)
                .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
            eprintln!(
                "[anofox-state] {}: truncated a torn last line ({torn_bytes} bytes) — a \
                 crash or `kill -9` mid-write, recovered at start",
                path.display()
            );
        }
        if fmeta.mode() & 0o077 != 0 {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
            eprintln!(
                "[anofox-state] {}: tightened loose file permissions to 0600",
                path.display()
            );
        }

        let mut state = StoreState {
            file,
            records: index.records,
            by_user: index.by_user,
            next_seq: index.next_seq,
            size_bytes: index.size_bytes,
        };
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
    pub fn open(path: &Path, max_mb: u64) -> Result<Store, String> {
        let _ = (path, max_mb);
        Err("the state store needs a Unix host (flock)".into())
    }

    /// Append one submission, assigning its `seq`/`ts`. `fdatasync`s before
    /// returning `Ok` (plan §1: "an acknowledged submission is durable").
    #[cfg(unix)]
    pub fn append(&self, new: NewRecord) -> Result<Record, String> {
        use std::io::Write;
        let mut st = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        // Checked under the lock, not before it (review of #530 #2): a
        // concurrent append failing and one passing this check used to be
        // able to race, so the second write could land right after a torn
        // fragment the first one left behind and get acknowledged anyway.
        if self.failed.load(Ordering::SeqCst) {
            return Err("state store: unavailable (a previous write failed)".into());
        }
        let seq = st.next_seq;
        let rec = Record {
            v: CURRENT_VERSION as u64,
            seq,
            ts: crate::host::access::rfc3339(std::time::SystemTime::now()),
            user: new.user,
            auth: new.auth,
            dashboard: new.dashboard,
            submit: new.submit,
            id: new.id,
            vars: new.vars,
        };
        // One `write_all` of the whole line including its newline (plan
        // §1), not two separate writes — narrows the window in which a
        // crash could tear the line (review of #530 #7).
        let mut line = rec.to_line();
        line.push('\n');
        let added = line.len() as u64;
        if st.size_bytes + added > self.max_bytes {
            return Err("state store is full".into());
        }
        let wrote = (|| -> std::io::Result<()> {
            #[cfg(test)]
            if self.force_fail.load(Ordering::SeqCst) {
                return Err(std::io::Error::other("forced test failure"));
            }
            st.file.write_all(line.as_bytes())?;
            st.file.sync_data()?;
            Ok(())
        })();
        if let Err(e) = wrote {
            self.failed.store(true, Ordering::SeqCst);
            return Err(format!("state store write failed: {e}"));
        }
        st.size_bytes += added;
        st.next_seq = seq
            .checked_add(1)
            .ok_or("state store: seq exhausted (u64::MAX reached)")?;
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

/// The in-memory index built from validating a store's content — no file
/// handle, so [`validate_lines`] is pure and can be run (and its result
/// inspected) *before* [`Store::open`] mutates anything (review of #530,
/// the BLOCKER: torn-tail truncation and the permission fix used to run
/// ahead of validation, so a misconfigured path to an unrelated existing
/// file could be destroyed before its content was ever checked).
struct Index {
    records: Vec<Record>,
    by_user: HashMap<String, Vec<usize>>,
    next_seq: u64,
    size_bytes: u64,
}

/// Validate `clean` (the store's content up to its last complete line,
/// [`recover_torn_tail`]) and build its index. `clean` is always `""` or
/// ends in `\n`. Byte-exact line iteration (`split('\n')`, not `lines()`,
/// which silently strips a trailing `\r` without counting it — review of
/// #530, nits: that under-accounted a CRLF file's size and tolerated
/// line-ending corruption no writer here ever produces); a bare `\r`
/// inside a line is refused outright rather than silently dropped.
fn validate_lines(clean: &str, max_bytes: u64) -> Result<Index, String> {
    let body = clean.strip_suffix('\n').unwrap_or(clean);
    let mut lines = if body.is_empty() {
        Vec::new().into_iter()
    } else {
        body.split('\n').collect::<Vec<_>>().into_iter()
    };
    let mut size_bytes = 0u64;
    let mut records = Vec::new();
    let mut by_user: HashMap<String, Vec<usize>> = HashMap::new();
    let mut next_seq = 1u64;

    let Some(header_line) = lines.next() else {
        return Ok(Index {
            records,
            by_user,
            next_seq,
            size_bytes,
        });
    };
    if header_line.contains('\r') {
        return Err("state file: header line contains a carriage return (CRLF?)".into());
    }
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
        if line.contains('\r') {
            return Err(format!(
                "state file: line {} contains a carriage return (CRLF?)",
                i + 2
            ));
        }
        let v: serde_json::Value = serde_json::from_str(line)
            .map_err(|e| format!("state file: malformed line {}: {e}", i + 2))?;
        let rec = Record::from_json(&v)
            .map_err(|e| format!("state file: malformed line {}: {e}", i + 2))?;
        if rec.v != CURRENT_VERSION as u64 {
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
        // A `seq` at `u64::MAX` would otherwise wrap `next_seq` back to 0,
        // which the "not strictly increasing" check above would then wave
        // through for every later line (review of #530, nits).
        next_seq = rec
            .seq
            .checked_add(1)
            .ok_or_else(|| format!("state file: line {} has an out-of-range seq", i + 2))?;
        by_user
            .entry(rec.user.clone())
            .or_default()
            .push(records.len());
        records.push(rec);
    }

    Ok(Index {
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

    /// What [`try_take`](Self::try_take) would see right now, without
    /// mutating anything — used only to pick an eviction candidate.
    fn current_tokens(&self, now: std::time::Instant) -> f64 {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        (self.tokens + dt * self.refill_per_sec).min(self.capacity)
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
                // Prefer a bucket that has refilled all the way back to
                // capacity — it carries no information (no pending
                // penalty), so evicting it is lossless, unlike the plain-
                // oldest one, which could be a long-lived *active* user
                // whose drained bucket gets reset for free (review of
                // #530, nits). Falls back to oldest-inserted when every
                // bucket is still partially drained.
                let evict = st
                    .buckets
                    .iter()
                    .find(|(_, b)| b.current_tokens(now) >= b.capacity)
                    .map(|(u, _)| u.clone())
                    .or_else(|| st.order.front().cloned());
                if let Some(u) = evict {
                    st.buckets.remove(&u);
                    st.order.retain(|x| x != &u);
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

    /// Reopen a path a previous `Store` on this path already dropped. Test
    /// artefact, not a store bug (review of #530): `flock` belongs to the
    /// *open file description*; a sibling test that forks a child (the
    /// `duckdb_cli`/`auth::session` tests do) keeps that description open
    /// across the `fork` until the child's `execve` closes its
    /// `O_CLOEXEC` fds, so a reopen racing that window sees a transient,
    /// spurious "in use" from a process that isn't even a server. Retries
    /// only on that one message, bounded, so a *genuine* second writer
    /// would still fail this after the bound. The very first "is it really
    /// held" assertion right after opening a store (no retry there) stays
    /// strict — it is correct by construction, no fork involved.
    #[cfg(unix)]
    fn reopen(path: &Path, max_mb: u64) -> Result<Store, String> {
        for _ in 0..100 {
            match Store::open(path, max_mb) {
                Err(e) if e.contains("in use by another server") => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                other => return other,
            }
        }
        Store::open(path, max_mb)
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
        let s2 = reopen(&p, DEFAULT_MAX_MB).unwrap();
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
        let s2 = reopen(&p, DEFAULT_MAX_MB).unwrap();
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
        let s2 = reopen(&p, DEFAULT_MAX_MB).unwrap();
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
        let e = reopen(&p, DEFAULT_MAX_MB).unwrap_err();
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
        let e = reopen(&p, DEFAULT_MAX_MB).unwrap_err();
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
        let e = reopen(&p, DEFAULT_MAX_MB).unwrap_err();
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
    fn header_missing_created_refuses_to_open() {
        let p = store_path("no-created");
        mkdir(p.parent().unwrap());
        std::fs::write(&p, b"{\"anofox_state\":1}\n").unwrap();
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(e.contains("created"), "{e}");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn gaps_in_seq_are_accepted() {
        // Only *strictly increasing* is required, not contiguous — a gap
        // (1, 5) is not itself evidence of corruption (review of #530:
        // "documents the intent").
        let p = store_path("seq-gap");
        mkdir(p.parent().unwrap());
        std::fs::write(
            &p,
            "{\"anofox_state\":1,\"created\":\"x\"}\n\
             {\"v\":1,\"seq\":1,\"ts\":\"t\",\"user\":\"a\",\"auth\":\"oidc\",\"dashboard\":\"d\",\"submit\":\"s\",\"id\":\"i\",\"vars\":{}}\n\
             {\"v\":1,\"seq\":5,\"ts\":\"t\",\"user\":\"b\",\"auth\":\"oidc\",\"dashboard\":\"d\",\"submit\":\"s\",\"id\":\"i\",\"vars\":{}}\n",
        )
        .unwrap();
        let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s.global_seq(), 5);
        // The next append continues from the file's own last seq (6), not
        // from the record count (2) — no reuse of the gapped numbers.
        let r = s.append(new_rec("c", json!({}))).unwrap();
        assert_eq!(r.seq, 6);
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn unknown_field_in_a_record_refuses_to_open() {
        let p = store_path("unknown-field");
        mkdir(p.parent().unwrap());
        std::fs::write(
            &p,
            "{\"anofox_state\":1,\"created\":\"x\"}\n\
             {\"v\":1,\"seq\":1,\"ts\":\"t\",\"user\":\"a\",\"auth\":\"oidc\",\"dashboard\":\"d\",\"submit\":\"s\",\"id\":\"i\",\"vars\":{},\"extra\":1}\n",
        )
        .unwrap();
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(e.contains("unknown field"), "{e}");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_crlf_line_refuses_to_open() {
        let p = store_path("crlf");
        mkdir(p.parent().unwrap());
        std::fs::write(
            &p,
            "{\"anofox_state\":1,\"created\":\"x\"}\n\
             {\"v\":1,\"seq\":1,\"ts\":\"t\",\"user\":\"a\",\"auth\":\"oidc\",\"dashboard\":\"d\",\"submit\":\"s\",\"id\":\"i\",\"vars\":{}}\r\n",
        )
        .unwrap();
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(e.contains("carriage return"), "{e}");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    // ── BLOCKER regression (review of #530 #1): never mutate a path that
    // doesn't validate as our own store ──────────────────────────────────

    #[cfg(unix)]
    #[test]
    fn a_foreign_file_without_a_trailing_newline_is_left_untouched() {
        use std::os::unix::fs::PermissionsExt;
        let root = tmp("foreign-no-nl");
        mkdir(&root);
        let p = root.join("secret.token");
        let content = b"sk-not-a-store-just-a-secret-written-with-echo-dash-n".to_vec();
        std::fs::write(&p, &content).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        let before = std::fs::read(&p).unwrap();
        let mode_before = std::fs::metadata(&p).unwrap().permissions().mode();
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(e.contains("doesn't look like a state store"), "{e}");
        let after = std::fs::read(&p).unwrap();
        assert_eq!(
            before, after,
            "a non-store file must be byte-identical after a refused open"
        );
        let mode_after = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(
            mode_before, mode_after,
            "its permissions must be untouched too"
        );
        assert_eq!(content, after);
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_non_store_file_with_a_trailing_newline_is_left_untouched() {
        // Also covers the BLOCKER: the old code validated *after*
        // truncating/chmod-ing, so even a cleanly newline-terminated
        // foreign file (nothing to "truncate") still had its permissions
        // tightened before its content was ever checked.
        use std::os::unix::fs::PermissionsExt;
        let root = tmp("foreign-with-nl");
        mkdir(&root);
        let p = root.join("other.jsonl");
        let content = b"{\"not\":\"ours\"}\n{\"also\":\"not ours\"}\n".to_vec();
        std::fs::write(&p, &content).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mode_before = std::fs::metadata(&p).unwrap().permissions().mode();
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(
            e.contains("bad header line") || e.contains("missing 'anofox_state'"),
            "{e}"
        );
        let after = std::fs::read(&p).unwrap();
        assert_eq!(
            content, after,
            "a non-store file must be byte-identical after a refused open"
        );
        let mode_after = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(
            mode_before, mode_after,
            "its permissions (0644, loose) must be untouched too"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    // ── filesystem-attack negative tests (review of #530: missing tests) ──

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_final_path_component_is_refused() {
        let root = tmp("symlink-final");
        mkdir(&root);
        let real = root.join("elsewhere.jsonl");
        std::fs::write(&real, b"not a store\n").unwrap();
        let link = root.join("state.jsonl");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let e = Store::open(&link, DEFAULT_MAX_MB).unwrap_err();
        // O_NOFOLLOW on the openat makes this ELOOP (or similar), not our
        // own "doesn't look like a store" message.
        assert!(!e.is_empty());
        assert_eq!(std::fs::read(&real).unwrap(), b"not a store\n");
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_parent_directory_is_refused() {
        let root = tmp("symlink-parent");
        mkdir(&root);
        let real_dir = root.join("real");
        mkdir(&real_dir);
        let link_dir = root.join("link");
        std::os::unix::fs::symlink(&real_dir, &link_dir).unwrap();
        let p = link_dir.join("state.jsonl");
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(!e.is_empty());
        assert!(!real_dir.join("state.jsonl").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_world_writable_parent_without_sticky_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let root = tmp("world-writable");
        mkdir(&root);
        let dir = root.join("store");
        mkdir(&dir);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        let p = dir.join("state.jsonl");
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(e.contains("writable"), "{e}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_at_the_path_is_refused() {
        let p = store_path("is-a-dir");
        std::fs::create_dir(&p).unwrap();
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(!e.is_empty());
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_fifo_at_the_path_is_refused() {
        let p = store_path("is-a-fifo");
        let cpath = std::ffi::CString::new(p.to_str().unwrap()).unwrap();
        let r = unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) };
        assert_eq!(r, 0, "mkfifo failed: {}", std::io::Error::last_os_error());
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(e.contains("not a regular file"), "{e}");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[test]
    fn owner_is_acceptable_only_for_euid_or_root() {
        assert!(
            owner_is_acceptable(1000, 1000),
            "the server's own euid owns it"
        );
        assert!(owner_is_acceptable(0, 1000), "root-owned is always trusted");
        assert!(
            !owner_is_acceptable(1001, 1000),
            "a different, non-root local account owns it"
        );
        assert!(
            !owner_is_acceptable(1000, 0),
            "even running as root, a non-root owner is refused"
        );
    }

    /// A file genuinely owned by a different (non-root) uid — needs real
    /// privilege to set up, which this sandbox doesn't have as a plain
    /// user, so it's faked with a Docker container bind-mounting a
    /// throwaway, briefly world-writable directory and `chown`-ing the
    /// file to an unprivileged uid inside its own user namespace. `#
    /// [ignore]`d (not part of the default `cargo test` gate) since it
    /// needs Docker; run explicitly with `cargo test --lib -- --ignored
    /// a_file_owned_by_a_different_uid`. [`owner_is_acceptable_only_for_
    /// euid_or_root`] above tests the same decision without that
    /// dependency.
    #[cfg(unix)]
    #[test]
    #[ignore]
    fn a_file_owned_by_a_different_uid_is_refused() {
        let root = tmp("foreign-uid");
        mkdir(&root);
        let p = root.join("state.jsonl");
        // Briefly world-writable so the container (a different uid) can
        // write into it; restored before `Store::open` ever sees it, so
        // this isn't exercising the *directory* ownership check too.
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777)).unwrap();
        }
        let status = std::process::Command::new("docker")
            .args([
                "run",
                "--rm",
                "-u",
                "12345:12345",
                "-v",
                &format!("{}:/mnt", root.display()),
                "alpine",
                "sh",
                "-c",
                "printf 'not a store\\n' > /mnt/state.jsonl && chmod 666 /mnt/state.jsonl",
            ])
            .status();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        match status {
            Ok(s) if s.success() => {}
            other => panic!("docker setup failed (is Docker installed and usable?): {other:?}"),
        }
        let e = Store::open(&p, DEFAULT_MAX_MB).unwrap_err();
        assert!(e.contains("not owned by this server"), "{e}");
        std::fs::remove_dir_all(&root).ok();
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
        let e = reopen(&p, 0).unwrap_err();
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
        assert!(reopen(&p, DEFAULT_MAX_MB).is_ok());
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
        // A small (1 MiB) budget, filled with ~2 KiB records — several
        // succeed, then one is refused once the cap is hit.
        let s = Store::open(&p, 1).unwrap();
        let note = "x".repeat(2_000);
        let mut last_err = None;
        for i in 0..2000 {
            if let Err(e) = s.append(new_rec(&format!("u{i}"), json!({"note": note}))) {
                last_err = Some(e);
                break;
            }
        }
        let e = last_err.expect("expected the store to refuse once max_mb was reached");
        assert!(e.contains("full"), "{e}");
        // A capacity refusal is not an I/O error: no write was attempted,
        // so the store isn't marked failed and the file didn't grow
        // (review of #530: missing negative test).
        assert!(!s.is_failed(), "a full store is not a failed store");
        let size_before = std::fs::metadata(&p).unwrap().len();
        assert!(s
            .append(new_rec("one-more", json!({"note": note})))
            .is_err());
        let size_after = std::fs::metadata(&p).unwrap().len();
        assert_eq!(
            size_before, size_after,
            "a refused append must not touch the file"
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

    #[test]
    fn eviction_prefers_a_fully_refilled_bucket_over_the_oldest() {
        // 1/sec refill, burst 3, capacity 2 users. alice and bob both
        // drain their burst at t0 (oldest = alice); by t1 (1s later) bob
        // has fully refilled (he's touched no further since) while alice
        // is drained again by a second use — so the new entrant should
        // evict *bob* (fully refilled, carries no information), not
        // alice (the plain-oldest, but mid-penalty), unlike a plain FIFO
        // (review of #530, nits).
        let rl = RateLimiter::new(60, 3, 2);
        let t0 = std::time::Instant::now();
        for _ in 0..3 {
            rl.check("alice", t0).unwrap();
        }
        for _ in 0..3 {
            rl.check("bob", t0).unwrap();
        }
        let t1 = t0 + std::time::Duration::from_secs(60); // both fully refill
        rl.check("alice", t1).unwrap(); // alice: refills to 3, then -> 2 tokens
        rl.check("carol", t1).ok(); // forces an eviction (capacity 2)
        assert_eq!(rl.tracked_users(), 2);
        // bob (fully refilled, no pending state) was evicted, not alice —
        // alice keeps exactly the 2 tokens she had, not a fresh burst of 3
        // (which a wrong eviction, or recreating her bucket, would give).
        assert!(rl.check("alice", t1).is_ok()); // 2 -> 1
        assert!(rl.check("alice", t1).is_ok()); // 1 -> 0
        assert!(
            rl.check("alice", t1).is_err(),
            "alice must have exactly 2 tokens left, not a fresh burst — she was not evicted"
        );
    }

    #[test]
    fn tracked_users_never_exceeds_the_cap_under_churn() {
        let rl = RateLimiter::new(60, 1, 10);
        let t0 = std::time::Instant::now();
        for i in 0..1000 {
            rl.check(&format!("user{i}"), t0).ok();
            assert!(rl.tracked_users() <= 10, "cap exceeded at user{i}");
        }
        assert_eq!(rl.tracked_users(), 10);
    }
}
