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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

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
        // #517 S3, plan §5 "name resolution without a policy": read-back
        // (`viewer_state`) relies on an unqualified reference always
        // resolving to the per-connection TEMP table, never a same-named
        // object `options.search_path` points at. Verified against both
        // pinned DuckDB builds: a `SET search_path` never moves `temp`
        // out of first place, however many entries it names or whichever
        // schema they are — but this is pinning an *observed* behaviour,
        // not a documented guarantee, so a multi-entry search_path still
        // refuses rather than bet a viewer's read-back isolation on it
        // continuing to hold in a future DuckDB. A single-entry path
        // (today's only supported shape with an access policy, too) is
        // unaffected.
        if ctx.search_path_entries > 1 {
            return Err(
                "options.state needs options.search_path to name at most one database: \
                 whether an unqualified viewer_state always resolves to the server's own \
                 per-request temp (never something options.search_path points at) is \
                 pinned by a test against the shipped DuckDB builds, not guaranteed by \
                 DuckDB itself for an arbitrary search_path"
                    .into(),
            );
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
    /// Number of entries in `options.search_path` (0: unset, the default).
    pub search_path_entries: usize,
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
    // `Store::open` opens the parent with `O_NOFOLLOW` and refuses a
    // symlink outright (review of #530 residual nit 1) — `canonicalize`
    // below would otherwise silently follow one and validate whatever it
    // resolves to, passing this check only for `open` to then refuse for
    // a reason this function never told the operator about.
    let parent_symlink_meta = std::fs::symlink_metadata(parent_raw)
        .map_err(|e| format!("options.state.path: '{}': {e}", parent_raw.display()))?;
    if parent_symlink_meta.file_type().is_symlink() {
        return Err(format!(
            "options.state.path: '{}' is a symlink — point options.state.path at the real \
             directory, not a link to it",
            parent_raw.display()
        ));
    }
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

/// The result of an *applied* [`Store::erase_user`] (review of #559, nit:
/// a successful rewrite that then hit a non-fatal problem — the directory
/// `fsync` after the `rename`, or re-opening/re-locking the rewritten file
/// for this same `Store` handle — used to come back as a plain `Err`, so
/// `dashboard state erase`'s audit line recorded `outcome: "error"` even
/// though the erase itself had already landed on disk, durably, by the
/// time either of those could fail). `warning` carries that non-fatal
/// detail when there is one; its presence or absence is a real, structured
/// signal rather than the brittle `e.contains("was rewritten on disk")`
/// string match this replaces — the erase is unconditionally applied
/// (`removed` rows gone) whenever `Store::erase_user` returns `Ok` at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EraseOutcome {
    /// How many records were removed. `0` for a user with no rows — not
    /// an error, just nothing to do.
    pub removed: usize,
    /// Set when the erase itself succeeded but a step after it didn't.
    /// Still `outcome: "ok"` for an audit trail — the data is gone either
    /// way — but worth surfacing to the operator.
    pub warning: Option<String>,
}

impl Record {
    /// The exact on-disk shape, as a `Value` — `pub` so `dashboard state
    /// export` (S5) can print the same encoding the store itself writes,
    /// rather than hand-rolling a second copy of this field list that
    /// could drift from it.
    pub fn to_json(&self) -> serde_json::Value {
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
    /// The header line verbatim, without its trailing `\n` (see
    /// [`Index::header_line`]).
    header_line: String,
    /// `Arc<Record>`, not `Record` (#548 item 2): every record is wrapped
    /// once, here and in [`Index`], so [`Store::snapshot_all`]'s per-gen
    /// rebuild only ever clones pointers (and refcounts) under this lock,
    /// never the record's own `String`/JSON-map contents — that deep copy,
    /// when it's still needed at all, happens in
    /// [`Store::snapshot_all`] after the lock is released.
    records: Vec<Arc<Record>>,
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
    /// The absolute path this `Store` was opened on — kept so
    /// [`Self::erase_user`] can create its crash-safe temp file as a
    /// sibling of the real file and `rename` over it (review of #555, B2).
    path: PathBuf,
    /// [`Self::open_read_only`] (review of #555's should-fix): [`Self::append`]
    /// and [`Self::erase_user`] refuse outright on a read-only `Store`, in
    /// addition to that `Store`'s own fd never having write access at the
    /// OS level — belt and suspenders, since a mistaken call here would
    /// otherwise surface only as a confusing `EBADF`/`write` failure.
    read_only: bool,
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
    /// #548 item 2: the last trainer "all rows" snapshot
    /// ([`Self::snapshot_all`]) built, keyed by the `gen`/`limit` it was
    /// built at. Append-only storage means the rows at or below a past
    /// `gen` never change, so every later request at the same `gen` (every
    /// panel of one trainer page load, until the next submission bumps the
    /// global seq) gets this `Arc` cloned back to it instead of redoing
    /// the clone. [`Self::erase_user`] clears this directly (it mutates
    /// `records` without moving `gen`, the one way this store's content
    /// isn't append-only) — review of #599 N1: both that clear and
    /// [`Self::snapshot_all`]'s own (re)build run with [`Self::inner`]
    /// held the whole time, so the two can never interleave; see
    /// `snapshot_all`'s own doc for why that's affordable now.
    trainer_cache: Mutex<Option<TrainerSnapshot>>,
    /// A cheap, lock-free mirror of `next_seq.saturating_sub(1)` ([`Self::
    /// global_seq`]), updated on every [`Self::append`]. Lets
    /// [`Self::snapshot_all`]'s cache-*hit* path check "is the cache still
    /// current" without ever taking [`Self::inner`]'s mutex (a *miss*
    /// still does, to rebuild — see its own doc).
    gen_hint: AtomicU64,
}

/// [`Store::snapshot_all`]'s cache (#548 item 2). `rows` holds `Arc<Record>`
/// pointers, not owned `Record`s (review of #599 SF1/SF2): the whole point
/// of caching this is to stop paying for a deep copy of up to
/// `trainer_max_rows` records' own `String`/JSON-map content on every
/// miss — keeping a *second* deep copy of that alive in the cache
/// afterwards would be the same cost moved, not removed.
struct TrainerSnapshot {
    gen: u64,
    limit: usize,
    rows: Arc<Vec<Arc<Record>>>,
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

/// A raw fd that closes itself on drop — shared by [`Store::open_impl`]
/// (the parent directory, held across the `openat` it protects) and
/// [`Store::erase_user`] (same directory, reopened for its crash-safe
/// temp-file-then-`renameat` rewrite, review of #555 B2).
#[cfg(unix)]
struct Fd(i32);
#[cfg(unix)]
impl Drop for Fd {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}
#[cfg(unix)]
impl std::os::unix::io::AsRawFd for Fd {
    fn as_raw_fd(&self) -> i32 {
        self.0
    }
}

/// Open `path`'s parent directory `O_DIRECTORY|O_NOFOLLOW`, check it's
/// owned by this process (or root) and not group/world-writable, and
/// return it alongside `path`'s own file name and this process's `euid`
/// — shared by [`Store::open_impl`] and [`Store::erase_user`] (review of
/// #555 B2), so the two don't drift on what "the parent directory is
/// safe to `openat`/`renameat` into" means.
#[cfg(unix)]
fn open_checked_parent_dir(path: &Path) -> Result<(Fd, &str, u32), String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| format!("options.state.path: '{}' must name a file", path.display()))?;
    let parent_raw = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("/"),
    };

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
    Ok((dirfd, file_name, euid))
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
        Self::open_impl(path, max_mb, false)
    }

    /// Open an *existing* file for reading only (`dashboard state
    /// export`/the dry-run half of `erase`, review of #555's should-fix):
    /// never creates it, never truncates a torn tail, never tightens loose
    /// permissions, never writes a header into an empty file — a read must
    /// never mutate the file it's reading, full stop. A shared (`LOCK_SH`)
    /// `flock`, so two read-only opens (two concurrent exports) can
    /// coexist; it still conflicts with a live server's or `erase`'s
    /// exclusive lock, so this still refuses outright while either holds
    /// the file (same "in use by another server" message — true either
    /// way: something else has it open for writing).
    #[cfg(unix)]
    pub fn open_read_only(path: &Path, max_mb: u64) -> Result<Store, String> {
        Self::open_impl(path, max_mb, true)
    }

    #[cfg(unix)]
    fn open_impl(path: &Path, max_mb: u64, read_only: bool) -> Result<Store, String> {
        use std::ffi::CString;
        use std::io::{Read, Write};
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        use std::os::unix::io::{AsRawFd, FromRawFd};

        let max_bytes = max_mb.saturating_mul(1 << 20);
        if !path.is_absolute() {
            return Err("options.state.path must be an absolute path".into());
        }
        let (dirfd, file_name, euid) = open_checked_parent_dir(path)?;

        let name_cstr = CString::new(file_name)
            .map_err(|_| "options.state.path: the file name has a NUL byte".to_string())?;
        let open_flags = if read_only {
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC
        } else {
            libc::O_RDWR | libc::O_CREAT | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_CLOEXEC
        };
        let fd = unsafe { libc::openat(dirfd.0, name_cstr.as_ptr(), open_flags, 0o600) };
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
        // should be local storage. A read-only open takes a *shared* lock
        // instead (review of #555): it still conflicts with any writer's
        // exclusive lock (a live server, or `erase`), but not with another
        // reader (#555's read-only `export`/dry-run can run concurrently
        // with itself; it never conflicts with a second read-only open).
        let lock_kind = if read_only {
            libc::LOCK_SH
        } else {
            libc::LOCK_EX
        };
        let locked = unsafe { libc::flock(file.as_raw_fd(), lock_kind | libc::LOCK_NB) };
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
        // validation above failed). Never for a read-only open (review of
        // #555's should-fix): a read must never mutate what it's reading
        // — `dashboard state export`/`erase`'s dry run tolerate a torn
        // tail or loose permissions exactly as found, byte-identical,
        // rather than silently "fixing" a file nobody asked them to write.
        if !read_only {
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
        } else if torn_bytes > 0 {
            eprintln!(
                "[anofox-state] {}: a torn last line ({torn_bytes} bytes) was found and \
                 ignored (read-only open: the file is left exactly as it was)",
                path.display()
            );
        }

        let mut state = StoreState {
            file,
            header_line: index.header_line,
            records: index.records,
            by_user: index.by_user,
            next_seq: index.next_seq,
            size_bytes: index.size_bytes,
        };
        if !read_only && clean.is_empty() {
            let header_body = format!(
                "{{\"anofox_state\":{CURRENT_VERSION},\"created\":\"{}\"}}",
                crate::host::access::rfc3339(std::time::SystemTime::now())
            );
            let mut header = header_body.clone();
            header.push('\n');
            state
                .file
                .write_all(header.as_bytes())
                .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
            state
                .file
                .sync_data()
                .map_err(|e| format!("options.state.path: '{}': {e}", path.display()))?;
            state.size_bytes = header.len() as u64;
            state.header_line = header_body;
        }
        let gen_hint = AtomicU64::new(state.next_seq.saturating_sub(1));
        Ok(Store {
            inner: Mutex::new(state),
            max_bytes,
            path: path.to_path_buf(),
            read_only,
            failed: AtomicBool::new(false),
            #[cfg(test)]
            force_fail: AtomicBool::new(false),
            trainer_cache: Mutex::new(None),
            gen_hint,
        })
    }

    #[cfg(not(unix))]
    pub fn open(path: &Path, max_mb: u64) -> Result<Store, String> {
        let _ = (path, max_mb);
        Err("the state store needs a Unix host (flock)".into())
    }

    #[cfg(not(unix))]
    pub fn open_read_only(path: &Path, max_mb: u64) -> Result<Store, String> {
        let _ = (path, max_mb);
        Err("the state store needs a Unix host (flock)".into())
    }

    /// Append one submission, assigning its `seq`/`ts`. `fdatasync`s before
    /// returning `Ok` (plan §1: "an acknowledged submission is durable").
    #[cfg(unix)]
    pub fn append(&self, new: NewRecord) -> Result<Record, String> {
        use std::io::Write;
        if self.read_only {
            return Err("state store: opened read-only, cannot append".into());
        }
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
        let rec = Arc::new(rec);
        st.records.push(rec.clone());
        // #548 item 2: kept in step with `next_seq` so `snapshot_all`'s
        // cache-hit check never takes `self.inner`'s lock just to see
        // whether a submission landed since the cache was built.
        self.gen_hint.store(seq, Ordering::Release);
        Ok((*rec).clone())
    }

    /// Rewrite the file, dropping every record whose `user` is an exact
    /// (case-sensitive) match for `user` — the same string `claims.user`
    /// carries, never normalised, since the store never normalises it on
    /// the way in either. Keeps the header line and every other record
    /// byte-for-byte. Operator tooling only (`dashboard state erase`, plan
    /// §1 "Operator access", §10 D7: a data-subject erasure request) —
    /// never called from the serve path, which only ever appends.
    ///
    /// Remaining `seq` values are untouched: append-only only promises
    /// *strictly increasing*, never contiguous, so a gap left by an erase
    /// changes nothing for any other viewer's `gen` or cache key. Returns
    /// an [`EraseOutcome`] — `removed` is the number of records removed
    /// (`0` leaves the file untouched — no-op, not an error, so a caller
    /// can erase a user with no rows at all without special-casing it);
    /// `warning`, when set, means the erase itself still landed, in full,
    /// on disk, but a step strictly after that (the directory `fsync`, or
    /// re-opening/re-locking the rewritten file) did not — see the `Err`
    /// branch below for why that's `Ok` with a warning, never an `Err`.
    ///
    /// Caveat (documented in `docs/secure-serving.md`, not hidden): if the
    /// erased user held the *highest* `seq` in the file, a later restart
    /// recomputes `next_seq` from what's left on disk, so a future
    /// submission may be assigned a `seq` an erased record once had.
    /// Harmless for every live-server guarantee this plan makes — `seq`
    /// only orders one server run's own cache/consistency decisions, and
    /// `erase` only ever runs while the server is stopped (the file's
    /// `flock` enforces that), so no in-memory cache or warm connection
    /// survives across an erase to be confused by the reuse. Not a
    /// permanent, forensic identifier: the audit log (`options.audit`),
    /// when configured, is the durable record of when a submission or an
    /// erasure happened, never `seq` itself.
    ///
    /// Blocked by [`Self::failed`] like [`Self::append`] is: erasing
    /// against a store a previous write already corrupted would just make
    /// the on-disk file disagree with whatever this process still has in
    /// memory, the exact inconsistency `failed` exists to prevent.
    ///
    /// Crash-safe by construction (review of #555, B2 — the in-place
    /// `seek`+`write`+`truncate` this replaced could leave a half-written
    /// file on a crash mid-rewrite): writes the new content to a **fresh
    /// temp file** in the same directory (`O_EXCL|O_NOFOLLOW`, `0600`,
    /// `fchown`'d to the original file's owner so an erase run as a
    /// different uid — root, say — never silently reassigns ownership),
    /// `fsync`s it, then `rename`s it over the original — atomic on the
    /// same filesystem, so at every instant before the `rename` the
    /// original is untouched, and from the instant it completes the new
    /// content is the *only* content anyone can ever open at this path
    /// again. `fsync`s the directory afterwards too, so the rename itself
    /// survives a crash (a bare file `fsync` only guarantees the data,
    /// not that the directory entry pointing at it is durable). The
    /// original file's own `flock` is held the entire time (never
    /// released, never even touched) — no writer can interleave with
    /// this rewrite — and, once the rename lands, is handed off to a
    /// fresh fd opened on the (now-renamed) file at the same path, so a
    /// later call on this same `Store` keeps writing the real file, never
    /// the orphaned pre-erase inode this fd pointed at a moment earlier.
    ///
    /// `#[cfg(unix)]` for the same reason every other OS-level piece of
    /// this store is (`open_impl`, `open_checked_parent_dir`, `Fd`): the
    /// body below calls straight into `libc` (`fchown`, `openat`,
    /// `renameat`, `fsync`, …) and `std::os::unix`, neither of which
    /// exist for a non-unix target — `libc` itself is a
    /// `[target.'cfg(unix)'.dependencies]` dependency (Cargo.toml), so on
    /// `wasm32-unknown-unknown` the crate isn't even present to link
    /// against. Caught by CI's wasm lib build (`cargo build --lib
    /// --target wasm32-unknown-unknown … --features wasm`): this method
    /// originally had no gate at all, which compiled fine natively but
    /// broke that build with "unresolved import `libc`". The
    /// `#[cfg(not(unix))]` stub below exists only so `dashboard state
    /// erase` compiles on Windows (#591): `Store` can never exist on a
    /// non-unix target ([`Self::open`]/[`Self::open_read_only`] always
    /// return `Err` there), so the stub is unreachable.
    #[cfg(unix)]
    pub fn erase_user(&self, user: &str) -> Result<EraseOutcome, String> {
        use std::ffi::CString;
        use std::io::Write;
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::io::{AsRawFd, FromRawFd};

        if self.read_only {
            return Err("state store: opened read-only, cannot erase".into());
        }
        let mut st = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if self.failed.load(Ordering::SeqCst) {
            return Err("state store: unavailable (a previous write failed)".into());
        }
        let kept: Vec<Arc<Record>> = st
            .records
            .iter()
            .filter(|r| r.user != user)
            .cloned()
            .collect();
        let removed = st.records.len() - kept.len();
        if removed == 0 {
            return Ok(EraseOutcome {
                removed: 0,
                warning: None,
            });
        }
        let mut buf = st.header_line.clone();
        buf.push('\n');
        for r in &kept {
            buf.push_str(&r.to_line());
            buf.push('\n');
        }
        let new_size = buf.len() as u64;

        let orig_meta = st
            .file
            .metadata()
            .map_err(|e| format!("state store erase failed: {e}"))?;

        let do_it = || -> Result<(std::fs::File, Option<String>), String> {
            let (dirfd, file_name, _euid) = open_checked_parent_dir(&self.path)?;

            // A name nothing else should ever guess or race: this
            // process's pid + a nanosecond timestamp, and `O_EXCL` on top
            // (belt and suspenders — `O_EXCL` alone is already enough to
            // refuse a collision rather than silently open an existing
            // file, which is the one property that actually matters
            // here).
            let now_nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let tmp_name = format!(".{file_name}.erase-tmp-{}-{now_nanos}", std::process::id());
            let tmp_cstr = CString::new(tmp_name.as_bytes())
                .map_err(|_| "state store erase: temp file name has a NUL byte".to_string())?;
            let tmp_fd = unsafe {
                libc::openat(
                    dirfd.as_raw_fd(),
                    tmp_cstr.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if tmp_fd < 0 {
                return Err(format!(
                    "state store erase: creating a temp file next to '{}': {}",
                    self.path.display(),
                    std::io::Error::last_os_error()
                ));
            }
            let mut tmp_file = unsafe { std::fs::File::from_raw_fd(tmp_fd) };

            // Preserve the original owner (review of #555 B2): a fresh
            // file this process creates is owned by its own euid/egid,
            // which would silently reassign the store's ownership on an
            // erase run as a different (typically more privileged) user
            // than the one that normally owns it.
            if unsafe { libc::fchown(tmp_fd, orig_meta.uid(), orig_meta.gid()) } != 0 {
                let err = std::io::Error::last_os_error();
                unsafe { libc::unlinkat(dirfd.as_raw_fd(), tmp_cstr.as_ptr(), 0) };
                return Err(format!("state store erase: fchown the temp file: {err}"));
            }
            // Always `fchmod`, never conditioned on the original file's own
            // mode (review of #559, nit): `openat`'s `0o600` argument is
            // still subject to this process's umask, so an unusual umask
            // such as `0277` left the temp file at `0400` whenever the
            // original happened to already read `0600` — unreadable `O_RDWR`
            // by the very next server start. `fchmod` on the fd, not
            // `set_permissions` on a path, so this can't be raced onto a
            // different file between the two.
            if unsafe { libc::fchmod(tmp_fd, 0o600) } != 0 {
                let err = std::io::Error::last_os_error();
                unsafe { libc::unlinkat(dirfd.as_raw_fd(), tmp_cstr.as_ptr(), 0) };
                return Err(format!("state store erase: chmod the temp file: {err}"));
            }

            let wrote = (|| -> std::io::Result<()> {
                tmp_file.write_all(buf.as_bytes())?;
                tmp_file.sync_data()?;
                Ok(())
            })();
            if let Err(e) = wrote {
                unsafe { libc::unlinkat(dirfd.as_raw_fd(), tmp_cstr.as_ptr(), 0) };
                return Err(format!("state store erase: writing the temp file: {e}"));
            }

            // Review of #555 B2: deterministic fault injection standing in
            // for a crash in exactly the one window that matters — the
            // temp file is fully written and `fsync`'d, but `rename`
            // hasn't run yet. Left un-unlinked (a real crash wouldn't
            // clean it up either); the point of the test this exists for
            // is that the ORIGINAL file at `self.path` is untouched by
            // anything above this line.
            #[cfg(test)]
            if self.force_fail.load(Ordering::SeqCst) {
                return Err("state store erase: forced test failure before rename".into());
            }

            let orig_cstr = CString::new(file_name)
                .map_err(|_| "state store erase: file name has a NUL byte".to_string())?;
            // Atomic within one directory: at every instant up to here the
            // original is untouched; from here on, the new content is the
            // only content anyone can open at this path.
            if unsafe {
                libc::renameat(
                    dirfd.as_raw_fd(),
                    tmp_cstr.as_ptr(),
                    dirfd.as_raw_fd(),
                    orig_cstr.as_ptr(),
                )
            } != 0
            {
                let err = std::io::Error::last_os_error();
                unsafe { libc::unlinkat(dirfd.as_raw_fd(), tmp_cstr.as_ptr(), 0) };
                return Err(format!("state store erase: renaming into place: {err}"));
            }
            // The rename's directory-entry update needs its own fsync to
            // survive a crash — a file's own fsync (above) only makes its
            // *data* durable, not that a directory now points at it there.
            // Review of #559, nit 1: a failure here is NOT the same as a
            // failure above the `rename` — the erase already took effect,
            // on the one and only inode anyone can open at this path from
            // here on. Losing the directory fsync only means a crash
            // right now, before some *other* durability event flushes the
            // directory anyway, could in principle roll the rename back on
            // some filesystems. That's a warning on an otherwise-successful
            // erase, never an `Err` that would make the CLI's audit line
            // say `"error"` for an erase that, on every filesystem anyone
            // actually runs this on, already happened.
            let dir_fsync_warning = if unsafe { libc::fsync(dirfd.as_raw_fd()) } != 0 {
                Some(format!(
                    "state store erase: the file was rewritten on disk, but fsync-ing the \
                     directory afterwards failed: {} — the erase took effect; this only \
                     means that rename may not survive a crash happening right now",
                    std::io::Error::last_os_error()
                ))
            } else {
                None
            };

            // Re-open the file fresh at the same path (now the rewritten
            // one) and take over its `flock`, so this `Store` keeps
            // writing the real file on any later call — never the
            // orphaned pre-erase inode `st.file` still otherwise points
            // at. `O_NOFOLLOW`: `openat` on the SAME held `dirfd`, so
            // nothing between the rename and this open could have
            // swapped the entry for a symlink.
            let new_fd = unsafe {
                libc::openat(
                    dirfd.as_raw_fd(),
                    orig_cstr.as_ptr(),
                    libc::O_RDWR | libc::O_APPEND | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if new_fd < 0 {
                return Err(format!(
                    "state store erase: the file was rewritten on disk, but re-opening it \
                     failed: {} — this Store handle must not be used again (open a fresh \
                     one to confirm the erase)",
                    std::io::Error::last_os_error()
                ));
            }
            let new_file = unsafe { std::fs::File::from_raw_fd(new_fd) };
            if unsafe { libc::flock(new_fd, libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(format!(
                    "state store erase: the file was rewritten on disk, but re-locking it \
                     failed: {} — this Store handle must not be used again (open a fresh \
                     one to confirm the erase)",
                    std::io::Error::last_os_error()
                ));
            }
            Ok((new_file, dir_fsync_warning))
        };

        match do_it() {
            Ok((new_file, warning)) => {
                st.file = new_file;
                let mut by_user: HashMap<String, Vec<usize>> = HashMap::new();
                for (i, r) in kept.iter().enumerate() {
                    by_user.entry(r.user.clone()).or_default().push(i);
                }
                st.records = kept;
                st.by_user = by_user;
                st.size_bytes = new_size;
                // #548 item 2: an erase changes `records` without moving
                // `next_seq`/`gen` (the one way this store's content isn't
                // append-only) — drop any cached snapshot rather than let a
                // later `snapshot_all` at the same `gen` hand back erased
                // rows. `st` (this function's own `inner` lock) is still
                // held here, same as `snapshot_all`'s own rebuild-and-store
                // — review of #599 N1: that's what rules out a rebuild
                // racing this clear and re-storing the pre-erase rows,
                // not just a best-effort ordering.
                *self.trainer_cache.lock().unwrap_or_else(|p| p.into_inner()) = None;
                if let Some(w) = &warning {
                    eprintln!("[anofox-state] {w}");
                }
                Ok(EraseOutcome { removed, warning })
            }
            Err(e) => {
                // The on-disk file is already erased and durable by the
                // time a re-open/re-lock failure could happen (both come
                // strictly after the `rename` + directory `fsync`) — only
                // THIS Store handle's own fd is now unreliable, so refuse
                // any further use of it rather than risk writing through
                // a stale reference, while still telling the caller the
                // erase itself succeeded.
                self.failed.store(true, Ordering::SeqCst);
                if e.contains("was rewritten on disk") {
                    eprintln!("[anofox-state] {e}");
                    Ok(EraseOutcome {
                        removed,
                        warning: Some(e),
                    })
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Unreachable in practice: [`Store::open`] refuses on non-Unix hosts,
    /// so no `Store` exists there. Present so callers compile (#591).
    #[cfg(not(unix))]
    pub fn append(&self, new: NewRecord) -> Result<Record, String> {
        let _ = new;
        Err("the state store needs a Unix host (flock)".into())
    }

    /// See [`Self::append`]'s non-Unix stub (#591).
    #[cfg(not(unix))]
    pub fn erase_user(&self, user: &str) -> Result<EraseOutcome, String> {
        let _ = user;
        Err("the state store needs a Unix host (flock)".into())
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
            .map(|idxs| idxs.iter().map(|&i| (*st.records[i]).clone()).collect())
            .unwrap_or_default()
    }

    /// Every row, newest `limit` kept (plan §5 `trainer_max_rows`). Not
    /// cached (unlike [`Self::snapshot_all`]): this is the CLI export
    /// path's own read, one-off rather than once per panel of a live
    /// trainer page.
    pub fn all_rows(&self, limit: usize) -> Vec<Record> {
        let st = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let n = st.records.len();
        st.records[n.saturating_sub(limit)..]
            .iter()
            .map(|r| (**r).clone())
            .collect()
    }

    /// [`Self::user_count`] and [`Self::rows_for_user`], read together
    /// under one lock (#548 item 3, nit: a caller folding the first into a
    /// cache key and the second into what it actually inserts must never
    /// see two different, racing reads of the store — a submission
    /// landing between two separate locks could make a key claim a
    /// generation the inserted rows don't match).
    pub fn snapshot_for_user(&self, user: &str) -> (u64, Vec<Record>) {
        let st = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        match st.by_user.get(user) {
            Some(idxs) => (
                idxs.len() as u64,
                idxs.iter().map(|&i| (*st.records[i]).clone()).collect(),
            ),
            None => (0, Vec::new()),
        }
    }

    /// [`Self::global_seq`] and [`Self::all_rows`]'s rows (trainer read-back,
    /// plan §5 `trainer_max_rows`), read together under one lock — see
    /// [`Self::snapshot_for_user`] for why. Cached per `(gen, limit)`
    /// (`Self::trainer_cache`, #548 item 2): a trainer page's panels all
    /// request this at the same `gen` (nothing moves it between them,
    /// short of a learner's submission landing mid-page-load), so after
    /// the first one rebuilds it, every later one just clones the `Arc` —
    /// a cache *hit* takes no lock on `self.inner` at all.
    ///
    /// Review of #599 (SF1/SF2): a *miss* clones `Arc<Record>` pointers —
    /// `records: Vec<Arc<Record>>` — never each record's own `String`/
    /// JSON-map content, which is what made this expensive before #548
    /// item 2. That clone is now cheap enough (and the resulting cache
    /// entry small enough — pointers, not a second deep copy kept resident
    /// indefinitely) that it's done, and the cache entry stored, **while
    /// still holding `self.inner`** rather than released early: a served
    /// page's several panels missing at once (the common case: it fetches
    /// several in parallel, all well inside one rebuild's — now brief —
    /// duration) each pay only this cheap clone, never a deep one, so
    /// there is no "thundering herd of deep copies" left to avoid by
    /// dropping the lock early. Holding `inner` for the whole rebuild also
    /// fully serializes it against [`Self::erase_user`] (which needs the
    /// same lock to clear this cache) by construction, closing #599 N1
    /// without a separate epoch counter: whichever of the two acquires
    /// `inner` first finishes its own cache write before the other can
    /// start, so a rebuild can never re-store pre-erase rows after an
    /// erase has already cleared them. N2: a cache already holding a
    /// newer `gen` (stored by a *faster* concurrent rebuild while this one
    /// was still running — contention on `inner` can reorder completion,
    /// not just start, since this isn't single-flighted) is never
    /// overwritten by this slower one's older result.
    pub fn snapshot_all(&self, limit: usize) -> (u64, Arc<Vec<Arc<Record>>>) {
        let hint = self.gen_hint.load(Ordering::Acquire);
        {
            let cache = self.trainer_cache.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(c) = cache.as_ref() {
                if c.gen == hint && c.limit == limit {
                    return (c.gen, c.rows.clone());
                }
            }
        }
        // Cache miss: `gen` and the rows must still come from the same
        // lock (#548 item 3 nit) — `hint` above is only a fast-path guess
        // at whether a rebuild is needed, never the value actually
        // returned or cached.
        let st = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let gen = st.next_seq.saturating_sub(1);
        let n = st.records.len();
        let rows: Arc<Vec<Arc<Record>>> = Arc::new(st.records[n.saturating_sub(limit)..].to_vec());
        {
            let mut cache = self.trainer_cache.lock().unwrap_or_else(|p| p.into_inner());
            if cache.as_ref().is_none_or(|c| c.gen <= gen) {
                *cache = Some(TrainerSnapshot {
                    gen,
                    limit,
                    rows: rows.clone(),
                });
            }
        }
        drop(st);
        (gen, rows)
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
    /// The header line verbatim (without its trailing `\n`), empty when
    /// `clean` had no header line at all (a brand-new file) — `Store::open`
    /// fills this in once it writes a fresh header. Kept so `erase_user`
    /// can rewrite the file without inventing a new `created` timestamp
    /// for an existing store (plan §1 "Operator access", S5).
    header_line: String,
    records: Vec<Arc<Record>>,
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
            header_line: String::new(),
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
        records.push(Arc::new(rec));
    }

    Ok(Index {
        header_line: header_line.to_string(),
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

    #[cfg(unix)]
    #[test]
    fn check_path_rules_refuses_a_symlinked_parent_like_store_open_does() {
        // Residual nit from the #530 review: `check_path_rules` used to
        // `canonicalize` straight through a symlinked parent and validate
        // whatever it resolved to, passing a config `Store::open` (which
        // opens the parent with `O_NOFOLLOW`) would then refuse — fixed
        // so both agree, with the clearer message coming from the
        // earlier, config-time check.
        let root = tmp("symlink-parent-check");
        mkdir(&root);
        let dash = root.join("dashboards");
        mkdir(&dash);
        let real_dir = root.join("real");
        mkdir(&real_dir);
        let link_dir = root.join("link");
        std::os::unix::fs::symlink(&real_dir, &link_dir).unwrap();
        let e = check_path_rules(&link_dir.join("state.jsonl"), None, &dash).unwrap_err();
        assert!(e.contains("symlink"), "{e}");
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
            search_path_entries: 0,
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
    fn validate_start_refuses_a_multi_entry_search_path() {
        // #517 S3, plan §5: read-back relies on an unqualified
        // `viewer_state` always resolving to the per-request TEMP, which a
        // test against the shipped DuckDB builds pins — but only for a
        // single-entry (or unset) search_path; refuse rather than extend
        // that pin to an untested shape.
        let root = tmp("search-path");
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
        ctx.search_path_entries = 2;
        let e = o.validate_start(&ctx).unwrap_err();
        assert!(e.contains("options.search_path"), "{e}");
        // One entry (or none) is fine (checked by every other test using
        // `base_ctx`'s default of 0 — this just confirms 1 specifically).
        ctx.search_path_entries = 1;
        assert!(o.validate_start(&ctx).is_ok());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn temp_resolves_before_search_path_for_unqualified_names() {
        // #548 item 1: the pin itself, automated (`validate_start_refuses_
        // a_multi_entry_search_path`'s own refusal only names this test;
        // it didn't exist until now). A real DuckDB session: ATTACH two
        // other databases, SET a multi-entry search_path naming them
        // (never `temp`), create a TEMP table, then a same-named REAL
        // table in the first attached database — an unqualified reference
        // must still resolve to the TEMP one. If a future DuckDB build
        // ever changes this, this test fails (CI), not a viewer's
        // read-back isolation silently breaking in production.
        //
        // Review of #550 (#548 item 1 nit): no `duckdb` on PATH must FAIL
        // this test, never skip-pass it — this one exists specifically to
        // catch a security-relevant catalog-resolution change, and CI's
        // own `test (core incl. serve)` job always installs the pinned
        // CLI first (ci.yml), so "missing" here is itself a real problem
        // to surface, not a reason to pretend the property was checked.
        assert!(
            std::process::Command::new("duckdb")
                .arg("-version")
                .output()
                .is_ok(),
            "no `duckdb` on PATH: this security pin cannot be skipped — install the \
             pinned CLI (see ci.yml's \"Install DuckDB CLI\" step) rather than letting \
             this test silently not check anything"
        );
        let mut s = crate::host::duckdb_cli::SessionRunner::spawn(None).unwrap();
        s.run("ATTACH ':memory:' AS warehouse;").unwrap();
        s.run("ATTACH ':memory:' AS warehouse2;").unwrap();
        s.run("SET search_path = 'warehouse.main,warehouse2.main';")
            .unwrap();
        s.run("CREATE TEMP TABLE viewer_state (x INT);").unwrap();
        s.run("INSERT INTO viewer_state VALUES (1);").unwrap();
        s.run("CREATE TABLE warehouse.main.viewer_state (x INT);")
            .unwrap();
        s.run("INSERT INTO warehouse.main.viewer_state VALUES (999);")
            .unwrap();
        let rows = s.run("SELECT * FROM viewer_state;").unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(
            rows[0].get("x").and_then(serde_json::Value::as_i64),
            Some(1),
            "an unqualified viewer_state must resolve to the TEMP table, not \
             warehouse.main.viewer_state, however options.search_path is set: {rows:?}"
        );
    }

    /// #548 item 2: the trainer read-back cost review flagged. Not run by
    /// default (`cargo test` would otherwise pay this every run) — `cargo
    /// test --lib --release -- --ignored --nocapture
    /// bench_snapshot_all_for_a_busy_trainer_page`. Records are inserted
    /// directly into `Store`'s private state, bypassing `append`'s
    /// per-row `fdatasync` — that cost is unaffected by this fix and not
    /// what's being measured; only `snapshot_all`'s own clone, under
    /// contention from a busy trainer page's panels, is.
    ///
    /// Two measurements (review of #599 SF1: the first cut of this
    /// benchmark only called `snapshot_all` *sequentially*, so every call
    /// after the first was a cache hit by construction — not what a real
    /// served page does, which fetches several panels concurrently
    /// (`web/app.js`'s `lazyPool`, 6 in flight)):
    /// - Sequential, `PANELS` calls one after another: only the first is
    ///   ever a genuine miss.
    /// - `PANELS` calls launched together from `PANELS` threads, cache
    ///   cleared first so they all race for the same `(gen, limit)` entry
    ///   — the shape SF1 described, the one worth actually measuring.
    ///
    /// Before #548 item 2's fix, every one of these calls cloned up to
    /// `trainer_max_rows` records' own `String`/JSON-map content under
    /// `Store`'s own mutex (measured: ~155ms/call for 200k rows, release
    /// build, this box). After this PR (review of #599 SF1/SF2): a miss
    /// clones only `Arc<Record>` pointers, so even every one of several
    /// *concurrent* misses is cheap — there is no deep copy left to avoid
    /// duplicating, so holding `self.inner` for the whole rebuild (see
    /// `snapshot_all`'s own doc) costs only brief contention on that lock,
    /// not a `trainer_max_rows`-sized copy per racing caller.
    #[test]
    #[ignore]
    #[cfg(unix)]
    fn bench_snapshot_all_for_a_busy_trainer_page() {
        let root = tmp("bench-snapshot-all");
        mkdir(&root);
        let p = root.join("s.jsonl");
        let s = retry_own_open(|| Store::open(&p, 512));
        const ROWS: usize = 200_000;
        const PANELS: usize = 12;
        {
            let mut st = s.inner.lock().unwrap();
            for i in 0..ROWS {
                let rec = Record {
                    v: CURRENT_VERSION as u64,
                    seq: (i + 1) as u64,
                    ts: "2026-01-01T00:00:00Z".to_string(),
                    user: format!("user{}", i % 500),
                    auth: "oidc".to_string(),
                    dashboard: "tutorial".to_string(),
                    submit: "quiz_1".to_string(),
                    id: format!("{i:032x}"),
                    vars: serde_json::json!({
                        "q1": "an answer text, not a single char",
                        "q2": i as i64
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                };
                let idx = st.records.len();
                st.by_user.entry(rec.user.clone()).or_default().push(idx);
                st.records.push(Arc::new(rec));
            }
            st.next_seq = (ROWS + 1) as u64;
        }
        s.gen_hint.store(ROWS as u64, Ordering::SeqCst);

        // 1. Sequential.
        let t0 = std::time::Instant::now();
        let mut per_call = Vec::with_capacity(PANELS);
        for _ in 0..PANELS {
            let call0 = std::time::Instant::now();
            let (_gen, rows) = s.snapshot_all(ROWS);
            per_call.push(call0.elapsed());
            assert_eq!(rows.len(), ROWS);
        }
        let elapsed = t0.elapsed();
        eprintln!(
            "sequential: {PANELS} snapshot_all({ROWS}) calls at the same gen took {elapsed:?} \
             ({:?}/call); first call (rebuild) {:?}, 2nd..last (cache hits) {:?}",
            elapsed / PANELS as u32,
            per_call[0],
            &per_call[1..]
        );

        // 2. Concurrent: PANELS threads, launched together, all racing for
        // one freshly-cleared cache entry — the shape a served page's own
        // parallel panel fetches actually produce.
        *s.trainer_cache.lock().unwrap() = None;
        let t0 = std::time::Instant::now();
        let per_call: Vec<std::time::Duration> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..PANELS)
                .map(|_| {
                    scope.spawn(|| {
                        let call0 = std::time::Instant::now();
                        let (_gen, rows) = s.snapshot_all(ROWS);
                        assert_eq!(rows.len(), ROWS);
                        call0.elapsed()
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let elapsed = t0.elapsed();
        let slowest = per_call.iter().max().copied().unwrap_or_default();
        let fastest = per_call.iter().min().copied().unwrap_or_default();
        eprintln!(
            "concurrent: {PANELS} snapshot_all({ROWS}) calls from {PANELS} threads at once, \
             racing for one cleared cache entry, took {elapsed:?} wall-clock; \
             fastest call {fastest:?}, slowest {slowest:?}"
        );
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

    /// Retry an open that is expected to succeed (#579): each test fixture
    /// already gets its own unique directory (`tmp`'s pid+nanos suffix), so
    /// there is no per-test file to collide on. The flake traced to this
    /// instead — reproducible only running the *whole* `cargo test --lib`
    /// at `--test-threads=64` (never isolating just the two affected
    /// tests): `flock(LOCK_NB)` on one test's own, never-before-touched
    /// fd occasionally returns `EWOULDBLOCK` with *no* corresponding
    /// holder in `/proc/locks` for that inode. That's the ~700-test binary
    /// hammering `flock()` across many files from 64 threads at once on
    /// btrfs, not a conflict over this file — a real genuine conflict
    /// (the negative assertions below) stays held for the test's whole
    /// lifetime and would never clear on retry. So: retry a few times
    /// instead of serializing the whole suite or inventing a per-test
    /// resource that already exists.
    #[cfg(unix)]
    fn retry_own_open(open: impl Fn() -> Result<Store, String>) -> Store {
        let mut last = None;
        for _ in 0..50 {
            match open() {
                Ok(s) => return s,
                Err(e) if e.contains("in use by another server") => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                    last = Some(e);
                }
                Err(e) => panic!("unexpected open error: {e}"),
            }
        }
        panic!(
            "own open still refused after retries: {}",
            last.unwrap_or_default()
        );
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
    fn erase_user_drops_only_that_users_rows_and_survives_a_reload() {
        let p = store_path("erase-basic");
        {
            let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
            s.append(new_rec("alice", json!({"q1": "B"}))).unwrap();
            s.append(new_rec("bob", json!({"q1": "A"}))).unwrap();
            s.append(new_rec("alice", json!({"q1": "C"}))).unwrap();
            assert_eq!(s.erase_user("alice").unwrap().removed, 2);
            assert_eq!(s.user_count("alice"), 0);
            assert_eq!(s.rows_for_user("alice").len(), 0);
            // bob's row, and its seq, are untouched.
            assert_eq!(s.user_count("bob"), 1);
            assert_eq!(s.rows_for_user("bob")[0].seq, 2);
            assert_eq!(s.len(), 1);
        }
        // Reopen: the file on disk agrees — bob's row survives with its
        // original seq (gaps are fine; strictly increasing is all that's
        // promised). `next_seq` is recomputed from what's left on disk,
        // so a fresh append resumes right after the highest *surviving*
        // seq (2), not the highest ever assigned (3, erased alice's) —
        // the documented reuse caveat on `erase_user` itself.
        let s2 = reopen(&p, DEFAULT_MAX_MB).unwrap();
        assert_eq!(s2.len(), 1);
        assert_eq!(s2.rows_for_user("bob")[0].seq, 2);
        let r4 = s2.append(new_rec("carol", json!({"q1": "A"}))).unwrap();
        assert_eq!(r4.seq, 3);
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    /// #599 SF3 (a): a submission after a warm `snapshot_all` call is
    /// visible on the very next one — the cache is keyed on `gen`, which
    /// `append` moves, not left stale.
    #[cfg(unix)]
    #[test]
    fn snapshot_all_after_an_append_returns_the_new_gen_and_row() {
        let p = store_path("snapshot-all-append");
        let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        s.append(new_rec("alice", json!({"q1": "A"}))).unwrap();
        let (gen, rows) = s.snapshot_all(100); // warms the cache at gen 1
        assert_eq!(gen, 1);
        assert_eq!(rows.len(), 1, "{rows:?}");
        s.append(new_rec("bob", json!({"q1": "B"}))).unwrap();
        let (gen, rows) = s.snapshot_all(100);
        assert_eq!(
            gen, 2,
            "a submission must move the gen the cache is keyed on"
        );
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert_eq!(rows[1].user, "bob");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    /// #599 SF3 (b): an erase clears a cache warmed before it, so the very
    /// next `snapshot_all` at the *same* gen (erase never moves it) does
    /// not hand back the erased user's rows — the one case `gen` alone
    /// can't tell apart from "nothing changed".
    #[cfg(unix)]
    #[test]
    fn snapshot_all_after_an_erase_drops_the_erased_users_rows_from_a_warm_cache() {
        let p = store_path("snapshot-all-erase");
        let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        s.append(new_rec("alice", json!({"q1": "A"}))).unwrap();
        s.append(new_rec("bob", json!({"q1": "B"}))).unwrap();
        let (gen_before, rows) = s.snapshot_all(100); // warms the cache
        assert_eq!(rows.len(), 2, "{rows:?}");
        let removed = s.erase_user("alice").unwrap().removed;
        assert_eq!(removed, 1);
        let (gen_after, rows) = s.snapshot_all(100);
        assert_eq!(gen_after, gen_before, "erase_user never moves next_seq/gen");
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].user, "bob");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    /// #599 SF3 (c): a changed `limit` is a cache miss, not a stale hit —
    /// `trainer_max_rows` could in principle change between two calls
    /// (config reload is out of scope today, but the cache key already
    /// accounts for it; this pins that it keeps working).
    #[cfg(unix)]
    #[test]
    fn snapshot_all_with_a_different_limit_is_a_miss_not_a_stale_hit() {
        let p = store_path("snapshot-all-limit");
        let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        for i in 0..5 {
            s.append(new_rec(&format!("user{i}"), json!({"q1": "A"})))
                .unwrap();
        }
        let (_, rows) = s.snapshot_all(3);
        assert_eq!(rows.len(), 3, "{rows:?}");
        let (_, rows) = s.snapshot_all(5);
        assert_eq!(
            rows.len(),
            5,
            "a different limit must rebuild, not reuse the limit=3 entry: {rows:?}"
        );
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn erase_user_is_a_no_op_for_a_user_with_no_rows() {
        let p = store_path("erase-noop");
        let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        s.append(new_rec("bob", json!({"q1": "A"}))).unwrap();
        assert_eq!(s.erase_user("nobody-ever-submitted").unwrap().removed, 0);
        assert_eq!(s.len(), 1);
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn erase_user_preserves_the_original_header_line_verbatim() {
        let p = store_path("erase-header");
        {
            let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
            s.append(new_rec("alice", json!({"q1": "B"}))).unwrap();
            s.erase_user("alice").unwrap();
        }
        let raw = std::fs::read_to_string(&p).unwrap();
        let header_line = raw.lines().next().unwrap();
        let header: serde_json::Value = serde_json::from_str(header_line).unwrap();
        let created = header["created"].as_str().unwrap().to_string();
        assert!(!created.is_empty());
        // A reload still accepts its own rewritten file (same header,
        // nothing is "ours but unrecognisable" after an erase).
        let s2 = reopen(&p, DEFAULT_MAX_MB).unwrap();
        assert_eq!(s2.len(), 0);
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    /// Review of #555 B2: "add a test that kills the process between
    /// write and rename (or injects a failure there) and reopens". A real
    /// `kill -9` isn't reproducible in a unit test, so `force_fail` (the
    /// same hook `io_error_while_appending_flips_store_to_failed` uses)
    /// injects the failure at the exact point requested — the crash-safe
    /// rewrite's temp file is fully written and `fsync`'d, but `rename`
    /// has not run. Everything the old in-place `seek`+`write`+`truncate`
    /// got wrong is the property under test here: the ORIGINAL file must
    /// still be byte-identical, bob's row must still be there, and
    /// alice's rows must NOT have come back (nor ever have left — this
    /// failure happens before the original is touched at all).
    #[cfg(unix)]
    #[test]
    fn erase_user_crash_before_rename_leaves_the_original_byte_identical() {
        let p = store_path("erase-crash-before-rename");
        let before;
        {
            let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
            s.append(new_rec("alice", json!({"q1": "B"}))).unwrap();
            s.append(new_rec("bob", json!({"q1": "A"}))).unwrap();
            before = std::fs::read(&p).unwrap();

            s.set_force_fail(true);
            let e = s.erase_user("alice").unwrap_err();
            assert!(e.contains("forced test failure"), "{e}");
            // A Store an erase failed on refuses any further use (fails
            // closed, same as a failed append) — this handle is done;
            // what matters from here is only what's on disk.
            assert!(s.is_failed());
        }

        let after = std::fs::read(&p).unwrap();
        assert_eq!(
            before, after,
            "a crash before rename must leave the original file untouched, byte for byte"
        );

        // A fresh Store on the same path (the flock released when `s`
        // dropped above) sees exactly the pre-erase content: alice's row
        // is still there, never removed; bob's is unaffected either way.
        let reopened = reopen(&p, DEFAULT_MAX_MB).unwrap();
        assert_eq!(reopened.len(), 2);
        assert_eq!(reopened.user_count("alice"), 1);
        assert_eq!(reopened.user_count("bob"), 1);
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn erase_user_refuses_once_the_store_has_failed() {
        let p = store_path("erase-failed");
        let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
        s.append(new_rec("alice", json!({"q1": "B"}))).unwrap();
        s.force_fail.store(true, Ordering::SeqCst);
        assert!(s.append(new_rec("alice", json!({"q1": "C"}))).is_err());
        assert!(s.is_failed());
        let e = s.erase_user("alice").unwrap_err();
        assert!(e.contains("unavailable"), "{e}");
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

    /// Review of #555's should-fix #4: `export`/a dry-run `erase` must
    /// never mutate the file they're reading — no torn-tail truncation,
    /// no permission tightening, no header written into an empty file.
    /// `Store::open` (used by every other test above) does all three;
    /// `Store::open_read_only` must do none of them, on the exact same
    /// torn-tail fixture `torn_last_line_is_truncated_and_recovered` uses.
    #[cfg(unix)]
    #[test]
    fn open_read_only_never_mutates_a_torn_tail_or_loose_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let p = store_path("read-only-torn");
        {
            let s = Store::open(&p, DEFAULT_MAX_MB).unwrap();
            s.append(new_rec("alice", json!({"q1": "B"}))).unwrap();
        }
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            f.write_all(br#"{"v":1,"seq":2,"ts":"2026-01-01T00:00:00.000Z","user":"bob","auth":"oidc","dashboard":"t","submit":"q","id":"x","vars":{"#).unwrap();
        }
        // Loosen the permissions too — a read-only open must leave these
        // exactly as found, same as the torn tail.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let before = std::fs::read(&p).unwrap();
        let before_mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;

        let s = retry_own_open(|| Store::open_read_only(&p, DEFAULT_MAX_MB));
        assert_eq!(s.len(), 1, "the one complete record still reads back");
        assert_eq!(s.global_seq(), 1);
        drop(s);

        let after = std::fs::read(&p).unwrap();
        let after_mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            before, after,
            "a read-only open must not truncate the torn tail"
        );
        assert_eq!(
            before_mode, after_mode,
            "a read-only open must not touch permissions"
        );
        assert_eq!(after_mode, 0o644, "still loose — nothing tightened it");
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
    }

    /// `open_read_only`'s shared lock (review of #555): two read-only
    /// opens on the same path coexist (two concurrent exports), but a
    /// writable [`Store::open`] still refuses outright against either —
    /// the one guarantee that actually matters (no write ever races a
    /// read, or another write).
    #[cfg(unix)]
    #[test]
    fn open_read_only_locks_are_shared_among_themselves_but_still_exclude_a_writer() {
        let p = store_path("read-only-lock");
        {
            retry_own_open(|| Store::open(&p, DEFAULT_MAX_MB));
        }
        let r1 = retry_own_open(|| Store::open_read_only(&p, DEFAULT_MAX_MB));
        let r2 = retry_own_open(|| Store::open_read_only(&p, DEFAULT_MAX_MB));
        let w = Store::open(&p, DEFAULT_MAX_MB);
        assert!(
            w.is_err_and(|e| e.contains("in use by another server")),
            "a writable open must refuse while either read-only open is live"
        );
        drop(r1);
        drop(r2);
        // Both readers gone: a writer can now take the exclusive lock.
        retry_own_open(|| Store::open(&p, DEFAULT_MAX_MB));
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
