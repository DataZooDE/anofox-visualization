//! A live DuckDB 2.0 Quack source for a locked server's snapshot (#352
//! prototype): `options.source` builds (part of) the snapshot by copying from
//! a remote `quack_serve` instance instead of (or alongside) the local
//! session, so a locked dashboard can serve data that lives on another
//! DuckDB 2.0 process.
//!
//! **Quack never enters the serving instance or the user's session.** Every
//! build opens its own, throwaway staging instance (`Database::open`,
//! extension autoload/autoinstall off): `LOAD quack`, a `CREATE TEMPORARY
//! SECRET` (never `ATTACH … (TOKEN …)`, which leaks the token into
//! `duckdb_databases().options`), `ATTACH '<uri>' AS …`, then the snapshot
//! file itself `(READ_WRITE)`. Both are `DETACH`ed and the secret is dropped
//! (best effort) before the staging instance closes; the snapshot file then
//! goes through the existing `READ_ONLY` + `lock_configuration` path exactly
//! like a local database (`build_snapshot` in `serve.rs`). The locked,
//! serving instance itself never loads `quack` — see
//! `docs/secure-serving.md`, "Quack source".
//!
//! The token is read fresh for every build (`read_token`), never cached and
//! never logged: every error on the way out is passed through [`redact`]
//! first, and [`SourceOpts`]'s `Debug` never prints it (only the *name* of
//! the env var / file it comes from).
//!
//! `copy_from_quack`'s `cancel` parameter plus `opts.timeout` are enforced by
//! a watchdog thread on a **second** staging connection: interrupting the
//! client's own connection does not stop a query already running on the
//! remote side (#352 fact 5), so the watchdog also runs `quack_cancel` there
//! before `duckdb_interrupt`ing the first connection.

use crate::db::{Conn, Database};
use crate::serve::fk_order;
use anofox_visualization::host::serving::{quote_ident, sql_string_literal, valid_ident};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Remote views: materialise them as tables in the snapshot (Quack exposes no
/// `CREATE VIEW` DDL to replay, only the query result — fact #2), or leave
/// them out.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Views {
    Materialize,
    Skip,
}

/// `options.source` of a locked server: build (part of) the snapshot from a
/// live Quack server. Its `Debug` never prints the token — only where it
/// comes from (`token_env`'s name, or `token_file`'s path).
#[derive(Clone)]
pub struct SourceOpts {
    /// `quack:host:port` (checked to start with `quack:` at parse time).
    pub uri: String,
    pub token_env: Option<String>,
    pub token_file: Option<PathBuf>,
    /// The database name the snapshot attaches the copy under (`options.source.as`).
    pub r#as: String,
    /// Schemas to copy (empty = every schema the source exposes).
    pub schemas: Vec<String>,
    pub views: Views,
    /// A deadline for the whole build (`timeout_s`).
    pub timeout: Duration,
    /// `tls.fingerprint` — reserved for a non-local source (open question:
    /// not enforced by this prototype; see docs/secure-serving.md).
    pub tls_fingerprint: Option<String>,
    /// `max_rows` — per-table row budget (security review L1/L3): a hostile
    /// or careless source cannot push an unbounded table into the snapshot.
    pub max_rows: u64,
    /// `max_bytes` — cumulative snapshot-file size budget, checked after
    /// each table/view (L3): bounds disk use during a build.
    pub max_bytes: u64,
}

/// `options.source.max_rows` default: generous, but not unbounded.
const DEFAULT_MAX_ROWS: u64 = 50_000_000;
/// `options.source.max_bytes` default: 10 GiB.
const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024 * 1024;

impl std::fmt::Debug for SourceOpts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceOpts")
            .field("uri", &self.uri)
            .field("token_env", &self.token_env)
            .field("token_file", &self.token_file)
            .field("as", &self.r#as)
            .field("schemas", &self.schemas)
            .field("views", &matches!(self.views, Views::Materialize))
            .field("timeout", &self.timeout)
            .field("tls_fingerprint_set", &self.tls_fingerprint.is_some())
            .field("max_rows", &self.max_rows)
            .field("max_bytes", &self.max_bytes)
            .finish()
    }
}

/// Parse `options.source` (a JSON object). Never reads the token — that
/// happens fresh per build, in [`read_token`].
pub fn parse(val: &serde_json::Value) -> Result<SourceOpts, String> {
    let m = val
        .as_object()
        .ok_or("options.source must be a JSON object")?;
    let (mut uri, mut token_env, mut token_file, mut as_name) = (None, None, None, None);
    let (mut schemas, mut views, mut tls_fingerprint) = (Vec::new(), Views::Materialize, None);
    let mut timeout = Duration::from_secs(600);
    let (mut max_rows, mut max_bytes) = (DEFAULT_MAX_ROWS, DEFAULT_MAX_BYTES);
    for (k, v) in m {
        match k.as_str() {
            "uri" => {
                let u = v
                    .as_str()
                    .ok_or("options.source.uri must be a string")?
                    .trim();
                if u.starts_with("quack://") {
                    return Err(
                        "options.source.uri: use 'quack:host:port' (no '//'), not 'quack://…'"
                            .into(),
                    );
                }
                if !u.starts_with("quack:") {
                    return Err("options.source.uri must start with 'quack:'".into());
                }
                uri = Some(u.to_string());
            }
            "token_env" => {
                let e = v
                    .as_str()
                    .filter(|e| !e.trim().is_empty())
                    .ok_or("options.source.token_env must be a non-empty string")?;
                token_env = Some(e.to_string());
            }
            "token_file" => {
                let p = v
                    .as_str()
                    .filter(|p| !p.trim().is_empty())
                    .ok_or("options.source.token_file must be a non-empty string")?;
                token_file = Some(PathBuf::from(p));
            }
            "token" => {
                return Err(
                    "options.source.token is refused — use token_env or token_file so the \
                     credential is never written into this SQL or logged with it"
                        .into(),
                )
            }
            "as" => {
                let a = v
                    .as_str()
                    .filter(|a| valid_ident(a))
                    .ok_or("options.source.as must be a database identifier")?;
                as_name = Some(a.to_string());
            }
            "schemas" => {
                let arr = v
                    .as_array()
                    .ok_or("options.source.schemas must be a list of schema names")?;
                for s in arr {
                    let s = s
                        .as_str()
                        .filter(|s| valid_ident(s))
                        .ok_or("options.source.schemas: invalid schema name")?;
                    schemas.push(s.to_string());
                }
            }
            "views" => {
                views = match v.as_str() {
                    Some("materialize") => Views::Materialize,
                    Some("skip") => Views::Skip,
                    _ => {
                        return Err(
                            "options.source.views must be \"materialize\" or \"skip\"".into()
                        )
                    }
                };
            }
            "timeout_s" => {
                let n = v
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or("options.source.timeout_s must be a positive integer")?;
                timeout = Duration::from_secs(n.min(86_400));
            }
            "max_rows" => {
                max_rows = v
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or("options.source.max_rows must be a positive integer")?;
            }
            "max_bytes" => {
                max_bytes = v
                    .as_u64()
                    .filter(|n| *n > 0)
                    .ok_or("options.source.max_bytes must be a positive integer")?;
            }
            "tls" => {
                let t = v
                    .as_object()
                    .ok_or("options.source.tls must be an object like {\"fingerprint\": \"…\"}")?;
                let fp = t
                    .get("fingerprint")
                    .and_then(|f| f.as_str())
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .ok_or("options.source.tls.fingerprint must be a non-empty string")?;
                tls_fingerprint = Some(fp.to_string());
            }
            other => {
                return Err(format!(
                    "options.source: unknown key '{other}' (uri, token_env, token_file, as, \
                     schemas, views, timeout_s, tls, max_rows, max_bytes)"
                ))
            }
        }
    }
    let uri = uri.ok_or("options.source.uri is required")?;
    match (&token_env, &token_file) {
        (Some(_), Some(_)) => {
            return Err("options.source: exactly one of token_env / token_file, not both".into())
        }
        (None, None) => {
            return Err(
                "options.source: exactly one of token_env or token_file is required \
                 (token_file is recommended — an env var can be read by any pre-lock SQL \
                 in the host session)"
                    .into(),
            )
        }
        _ => {}
    }
    Ok(SourceOpts {
        uri,
        token_env,
        token_file,
        r#as: as_name.unwrap_or_else(|| "source".to_string()),
        schemas,
        views,
        timeout,
        tls_fingerprint,
        max_rows,
        max_bytes,
    })
}

/// Read the source token fresh (never cached across builds): the env var, or
/// the file (trimmed; refuses an empty file). Refuses a token containing a
/// control character or a single quote (security review I1): such a token
/// would appear quote-doubled in the `CREATE TEMPORARY SECRET` literal, and
/// [`redact`] only reliably strips the raw form.
pub fn read_token(opts: &SourceOpts) -> Result<String, String> {
    let (t, where_) = if let Some(name) = &opts.token_env {
        (
            std::env::var(name).map_err(|_| {
                format!("options.source.token_env: environment variable '{name}' is not set")
            })?,
            format!("options.source.token_env ('{name}')"),
        )
    } else if let Some(path) = &opts.token_file {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            format!(
                "options.source.token_file: cannot read '{}': {e}",
                path.display()
            )
        })?;
        (
            raw,
            format!("options.source.token_file ('{}')", path.display()),
        )
    } else {
        return Err("options.source: no token source configured".into());
    };
    let t = t.trim();
    if t.is_empty() {
        return Err(format!("{where_} is empty"));
    }
    if t.chars().any(|c| c.is_control() || c == '\'') {
        return Err(format!(
            "{where_}: the token must not contain a control character or a single quote"
        ));
    }
    // security review R1: redact()'s partial-prefix match is simplest (and
    // provably panic-free) on single-byte characters; Quack tokens are
    // ASCII in practice, so refuse anything else outright.
    if !t.is_ascii() {
        return Err(format!("{where_}: the token must be ASCII"));
    }
    Ok(t.to_string())
}

/// Replace every occurrence of `token` in `err` — the token must never reach
/// stderr, the request log, `/healthz`, a page, a panel or an app-config
/// (`docs/secure-serving.md`, "Quack source"). Also strips the quote-doubled
/// form the token takes inside the `CREATE TEMPORARY SECRET` SQL literal (in
/// case a raw-form replace above already changed the string and left the
/// literal's own escaping visible), and the longest prefix of the token that
/// still appears — DuckDB's `LINE 1: …` error context can cut a long literal
/// short, leaving only a prefix of the token in an otherwise-redacted message
/// (security review I1).
pub fn redact(err: &str, token: &str) -> String {
    if token.is_empty() {
        return err.to_string();
    }
    let mut out = err.replace(token, "<redacted>");
    let quoted = token.replace('\'', "''");
    if quoted != token {
        out = out.replace(&quoted, "<redacted>");
    }
    // security review R1: `&token[..n]` on a byte offset panics if it lands
    // inside a multi-byte character — iterate char boundaries instead of
    // bytes. (`read_token` also refuses a non-ASCII token outright, so this
    // is belt and suspenders: `char_indices` costs nothing extra either way.)
    if token.len() >= 8 {
        let boundaries: Vec<usize> = token
            .char_indices()
            .map(|(i, _)| i)
            .filter(|&i| i >= 8)
            .collect();
        for &n in boundaries.iter().rev() {
            if out.contains(&token[..n]) {
                out = out.replace(&token[..n], "<redacted>");
                break;
            }
        }
    }
    out
}

/// Build the snapshot file for a Quack source: a dedicated staging instance
/// that `LOAD`s `quack`, attaches the source under a temporary secret,
/// attaches the new snapshot file, copies schema + data (+ materialised
/// views), then tears both attachments and the secret down.
///
/// `dir`: the snapshot's own private directory (the file is created inside
/// it). `cancel`: checked by the internal watchdog alongside `opts.timeout`;
/// set it from the outside (an `anofox_serve_stop` in flight) to cancel a
/// build early.
///
/// Returns the database name to attach the file under (`opts.as`), the file
/// path, and non-fatal warnings (lost FKs/macros/sequences, skipped views,
/// schemas the source exposes but `options.source.schemas` didn't ask for).
pub fn copy_from_quack(
    dir: &Path,
    opts: &SourceOpts,
    cancel: &AtomicBool,
) -> Result<(String, PathBuf, Vec<String>), String> {
    let token = read_token(opts)?;
    build(dir, opts, &token, cancel).map_err(|e| redact(&e, &token))
}

fn build(
    dir: &Path,
    opts: &SourceOpts,
    token: &str,
    cancel: &AtomicBool,
) -> Result<(String, PathBuf, Vec<String>), String> {
    let mut warnings = Vec::new();
    // 1. A throwaway staging instance: nothing auto-loads or auto-installs,
    // and only `quack` (loaded explicitly, next) may load at all.
    let db = Database::open_quack_staging()?;
    let stg = db.connect()?;
    stg.exec("LOAD quack").map_err(|e| {
        format!(
            "could not LOAD quack in the staging instance — install it first (INSTALL quack \
             from the repository; it is never autoloaded or autoinstalled here): {}",
            e.lines().next().unwrap_or("")
        )
    })?;
    // quack pools client connections per host:port, process-wide (observed:
    // reusing a connection that went stale when the source died can hang a
    // later build against a *different* table of a *restarted* source on
    // the same port, even from a brand-new staging Database). Clear it at
    // the start of every build so a build never inherits another one's
    // pooled state.
    let _ = stg.query("SELECT * FROM quack_clear_cache()");

    // 2. A named TEMPORARY SECRET (never `ATTACH … (TOKEN …)`, which leaks
    // the token into duckdb_databases().options) + the remote attachment.
    let alias = format!("__anofox_src_{}", sv_random());
    stg.exec(&format!(
        "CREATE TEMPORARY SECRET {} (TYPE quack, TOKEN {})",
        quote_ident(&alias),
        sql_string_literal(token)
    ))?;
    stg.exec(&format!(
        "ATTACH {} AS {}",
        sql_string_literal(&opts.uri),
        quote_ident(&alias)
    ))?;

    // 3. The snapshot file, attached READ_WRITE (dropped/detached below
    // regardless of outcome; the caller only sees it on success).
    let file = dir.join(format!("db_quack_{}.duckdb", sv_random()));
    let f = file.to_string_lossy().into_owned();
    let dst_alias = "__anofox_quack_dst";
    let attach_dst = stg.exec(&format!(
        "ATTACH {} AS {} (READ_WRITE)",
        sql_string_literal(&f),
        dst_alias
    ));

    let size_fired = AtomicBool::new(false);
    let result = attach_dst.and_then(|_| {
        // 4. The watchdog: a SECOND connection on the same staging instance
        // (`Conn` is `Send`, not `Sync` — owned by the watchdog thread, not
        // borrowed, since a `duckdb_connection` is one-at-a-time). It also
        // polls the snapshot file's size every interval (security review
        // R3): checking `max_bytes` only *between* tables let a single huge
        // table or view overshoot it by gigabytes before the build noticed.
        let stg2 = db.connect()?;
        let intr = stg.interrupter();
        let deadline = Instant::now() + opts.timeout;
        let finished = AtomicBool::new(false);
        let watchdog_alias = alias.clone();
        let (max_bytes, file_for_check, size_fired_ref) =
            (opts.max_bytes, file.clone(), &size_fired);
        std::thread::scope(|scope| {
            let finished_ref = &finished;
            scope.spawn(move || {
                let size_exceeded = || {
                    let over = std::fs::metadata(&file_for_check)
                        .map(|m| m.len() > max_bytes)
                        .unwrap_or(false);
                    if over {
                        size_fired_ref.store(true, Ordering::SeqCst);
                    }
                    over
                };
                if wait_for_fire(
                    finished_ref,
                    cancel,
                    deadline,
                    Duration::from_millis(50),
                    &size_exceeded,
                ) {
                    cancel_remote(&stg2, &watchdog_alias);
                    intr.interrupt();
                }
            });
            let r = copy_schema_and_data(&stg, &alias, dst_alias, &file, opts, &mut warnings);
            finished.store(true, Ordering::SeqCst);
            r
        })
    });

    // 5. Best-effort teardown either way: DETACH both, DROP the secret.
    let _ = stg.exec(&format!("DETACH {dst_alias}"));
    let _ = stg.exec(&format!("DETACH {}", quote_ident(&alias)));
    let _ = stg.exec(&format!("DROP SECRET {}", quote_ident(&alias)));

    if size_fired.load(Ordering::SeqCst) {
        let _ = result; // the interrupt's own error is less useful than this.
        return Err(format!(
            "options.source.max_bytes ({}) exceeded during the copy — cancelled",
            opts.max_bytes
        ));
    }
    result?;
    warnings.push(format!(
        "quack source '{}': every table is materialised by CREATE TABLE … AS SELECT — foreign \
         keys, indexes, macros, sequences, primary keys, NOT NULL and generated-column formulas \
         are not copied (irrelevant for a READ_ONLY snapshot; values still match)",
        opts.r#as
    ));
    Ok((opts.r#as.clone(), file, warnings))
}

/// Poll every `interval` until either `finished` is set (the copy returned on
/// its own: no need to cancel, returns `false`) or `cancel` is set, `deadline`
/// passes, or `extra_fire` says to (the watchdog must fire: returns `true`).
/// `extra_fire` is the `max_bytes` file-size check (security review R3) when
/// called from [`build`]; pure timing logic otherwise, kept free of any
/// DuckDB call so it can be unit-tested without a host API.
fn wait_for_fire(
    finished: &AtomicBool,
    cancel: &AtomicBool,
    deadline: Instant,
    interval: Duration,
    extra_fire: &dyn Fn() -> bool,
) -> bool {
    loop {
        if finished.load(Ordering::SeqCst) {
            return false;
        }
        if cancel.load(Ordering::SeqCst) || Instant::now() >= deadline || extra_fire() {
            return true;
        }
        std::thread::sleep(interval);
    }
}

/// Ask a second connection to cancel the query the first one is running on
/// `alias` (`quack_cancel(catalog, quack_connection_id(alias))`). Best
/// effort: an error here just means the local `duckdb_interrupt` that
/// follows is the only thing that fires.
fn cancel_remote(stg2: &Conn, alias: &str) {
    let lit = sql_string_literal(alias);
    let Ok(mut r) = stg2.query(&format!("SELECT quack_connection_id({lit})")) else {
        return;
    };
    let Ok(rows) = r.string_rows(1) else { return };
    let Some(cid) = rows
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next().flatten())
    else {
        return;
    };
    let _ = stg2.exec(&format!(
        "SELECT * FROM quack_cancel({lit}, {})",
        sql_string_literal(&cid)
    ));
}

/// One `CREATE TABLE … AS SELECT *` per table (security review L1): never
/// `COPY FROM DATABASE … (SCHEMA)`, which would replay the source's DDL
/// verbatim — including a generated column's *expression*, which DuckDB
/// evaluates lazily on every read. Replayed into the **serving** instance,
/// that expression would run there, past any access policy's
/// `policy_functions.txt` denylist (the policy only ever sees the panel SQL
/// calling it, never a generated column doing so). CTAS instead materialises
/// every column's *current value*, generated or not: the serving instance
/// never evaluates a source-chosen expression. The trade-off: primary keys,
/// `NOT NULL`, indexes, foreign keys, macros and sequences are not carried
/// over — irrelevant for a READ_ONLY snapshot, documented in
/// secure-serving.md. `opts.max_rows` / `opts.max_bytes` bound a hostile or
/// oversized source (L3): checked per table / after each table and view.
fn copy_schema_and_data(
    stg: &Conn,
    src: &str,
    dst: &str,
    dst_file: &Path,
    opts: &SourceOpts,
    warnings: &mut Vec<String>,
) -> Result<(), String> {
    stg.exec("BEGIN")?;
    let r = (|| -> Result<(), String> {
        let tables = list(stg, src, "duckdb_tables()", "table_name", &opts.schemas)?;
        let edges: Vec<(String, String, String)> = Vec::new(); // Quack exposes no FKs.
        let mut schemas_created = std::collections::BTreeSet::new();
        for i in fk_order(&tables, &edges) {
            let (schema, table) = &tables[i];
            if !schema.eq_ignore_ascii_case("main") && schemas_created.insert(schema.clone()) {
                stg.exec(&format!(
                    "CREATE SCHEMA IF NOT EXISTS {dst}.{}",
                    quote_ident(schema)
                ))?;
            }
            let n = row_count(stg, src, schema, table)?;
            if n > opts.max_rows {
                return Err(format!(
                    "{schema}.{table}: {n} rows exceeds options.source.max_rows ({})",
                    opts.max_rows
                ));
            }
            let qual = |db: &str| format!("{}.{}.{}", db, quote_ident(schema), quote_ident(table));
            let (from, to) = (qual(src), qual(dst));
            stg.exec(&format!("CREATE TABLE {to} AS SELECT * FROM {from}"))
                .map_err(|e| format!("{schema}.{table}: {}", e.lines().next().unwrap_or("")))?;
            check_max_bytes(dst_file, opts.max_bytes, &format!("table {schema}.{table}"))?;
        }
        let views = list(stg, src, "duckdb_views()", "view_name", &opts.schemas)?;
        for (schema, view) in views {
            match opts.views {
                Views::Skip => warnings.push(format!(
                    "quack source '{}': view {schema}.{view} skipped (options.source.views = \"skip\")",
                    opts.r#as
                )),
                Views::Materialize => {
                    if !schema.eq_ignore_ascii_case("main") && schemas_created.insert(schema.clone())
                    {
                        stg.exec(&format!(
                            "CREATE SCHEMA IF NOT EXISTS {dst}.{}",
                            quote_ident(&schema)
                        ))?;
                    }
                    let from = format!(
                        "{}.{}.{}",
                        quote_ident(src),
                        quote_ident(&schema),
                        quote_ident(&view)
                    );
                    let to = format!("{}.{}.{}", dst, quote_ident(&schema), quote_ident(&view));
                    // A view has no row count to check up front (it's a
                    // query, not a table) — security review R3: LIMIT it to
                    // one more than max_rows and refuse if that extra row
                    // actually came back, so a view can't be used to get
                    // around the table row budget.
                    let limit = opts.max_rows.saturating_add(1);
                    stg.exec(&format!("CREATE TABLE {to} AS SELECT * FROM {from} LIMIT {limit}"))
                        .map_err(|e| {
                            format!("view {schema}.{view}: {}", e.lines().next().unwrap_or(""))
                        })?;
                    check_max_bytes(dst_file, opts.max_bytes, &format!("view {schema}.{view}"))?;
                    let copied = row_count(stg, dst, &schema, &view)?;
                    if copied > opts.max_rows {
                        return Err(format!(
                            "view {schema}.{view}: at least {copied} rows exceeds options.source.max_rows ({})",
                            opts.max_rows
                        ));
                    }
                    warnings.push(format!(
                        "quack source '{}': view {schema}.{view} materialised as a table (its definition is not replayed)",
                        opts.r#as
                    ));
                }
            }
        }
        Ok(())
    })();
    match r {
        Ok(()) => stg.exec("COMMIT"),
        Err(e) => {
            let _ = stg.exec("ROLLBACK");
            Err(e)
        }
    }
}

/// `SELECT count(*)` of `src.schema.table`, checked against `max_rows`
/// *before* copying it (security review L3).
fn row_count(stg: &Conn, src: &str, schema: &str, table: &str) -> Result<u64, String> {
    let q = format!(
        "SELECT count(*)::VARCHAR FROM {}.{}.{}",
        quote_ident(src),
        quote_ident(schema),
        quote_ident(table)
    );
    stg.query(&q)?
        .string_rows(1)?
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next().flatten())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("{schema}.{table}: could not read its row count"))
}

/// The snapshot file's on-disk size, checked against `max_bytes` after every
/// table/view (security review L3) — a hostile or oversized source cannot
/// fill the disk the snapshot directory lives on.
fn check_max_bytes(dst_file: &Path, max_bytes: u64, after: &str) -> Result<(), String> {
    let size = std::fs::metadata(dst_file)
        .map(|m| m.len())
        .map_err(|e| format!("could not stat the snapshot file after {after}: {e}"))?;
    if size > max_bytes {
        return Err(format!(
            "the snapshot file reached {size} bytes after {after}, exceeding options.source.max_bytes ({max_bytes})"
        ));
    }
    Ok(())
}

/// `(schema, name)` pairs of `src`'s tables or views, restricted to
/// `schemas` when non-empty.
fn list(
    stg: &Conn,
    src: &str,
    from: &str,
    name_col: &str,
    schemas: &[String],
) -> Result<Vec<(String, String)>, String> {
    let lit = sql_string_literal(src);
    let rows = stg
        .query(&format!(
            "SELECT schema_name::VARCHAR, {name_col}::VARCHAR FROM {from} \
             WHERE database_name = {lit} AND NOT internal ORDER BY 1, 2"
        ))?
        .string_rows(100_000)?;
    Ok(rows
        .into_iter()
        .filter_map(|r| Some((r[0].clone()?, r[1].clone()?)))
        .filter(|(s, _)| schemas.is_empty() || schemas.iter().any(|w| w.eq_ignore_ascii_case(s)))
        .collect())
}

/// A short random suffix for staging names (aliases, file names) — delegates
/// to the serving module's CSPRNG-backed token so this module doesn't need
/// its own randomness source.
fn sv_random() -> String {
    anofox_visualization::host::serving::random_token()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn src(json: &str) -> Result<SourceOpts, String> {
        parse(&serde_json::from_str(json).unwrap())
    }

    #[test]
    fn uri_must_carry_the_quack_prefix() {
        let err = src(r#"{"uri":"host:9494","token_env":"T"}"#).unwrap_err();
        assert!(err.contains("must start with 'quack:'"), "{err}");
        let hint = src(r#"{"uri":"quack://host:9494","token_env":"T"}"#).unwrap_err();
        assert!(hint.contains("quack:host:port"), "{hint}");
        assert!(src(r#"{"uri":"quack:host:9494","token_env":"T"}"#).is_ok());
    }

    #[test]
    fn exactly_one_of_token_env_or_token_file() {
        let neither = src(r#"{"uri":"quack:h:1"}"#).unwrap_err();
        assert!(
            neither.contains("exactly one of token_env or token_file"),
            "{neither}"
        );
        let both = src(r#"{"uri":"quack:h:1","token_env":"T","token_file":"/tmp/t"}"#).unwrap_err();
        assert!(both.contains("not both"), "{both}");
        assert!(src(r#"{"uri":"quack:h:1","token_env":"T"}"#).is_ok());
        assert!(src(r#"{"uri":"quack:h:1","token_file":"/tmp/t"}"#).is_ok());
    }

    #[test]
    fn a_literal_token_key_is_refused() {
        let err = src(r#"{"uri":"quack:h:1","token":"secret"}"#).unwrap_err();
        assert!(err.contains("options.source.token is refused"), "{err}");
    }

    #[test]
    fn as_defaults_to_source_and_must_be_an_identifier() {
        let o = src(r#"{"uri":"quack:h:1","token_env":"T"}"#).unwrap();
        assert_eq!(o.r#as, "source");
        let err = src(r#"{"uri":"quack:h:1","token_env":"T","as":"bad; drop"}"#).unwrap_err();
        assert!(err.contains("options.source.as"), "{err}");
        let o = src(r#"{"uri":"quack:h:1","token_env":"T","as":"warehouse"}"#).unwrap();
        assert_eq!(o.r#as, "warehouse");
    }

    #[test]
    fn views_and_unknown_keys() {
        assert!(matches!(
            src(r#"{"uri":"quack:h:1","token_env":"T","views":"skip"}"#)
                .unwrap()
                .views,
            Views::Skip
        ));
        let err = src(r#"{"uri":"quack:h:1","token_env":"T","views":"nope"}"#).unwrap_err();
        assert!(err.contains("materialize"), "{err}");
        let err = src(r#"{"uri":"quack:h:1","token_env":"T","bogus":1}"#).unwrap_err();
        assert!(err.contains("unknown key 'bogus'"), "{err}");
    }

    #[test]
    fn schema_names_are_validated_identifiers() {
        let err =
            src(r#"{"uri":"quack:h:1","token_env":"T","schemas":["ok","bad name"]}"#).unwrap_err();
        assert!(err.contains("invalid schema name"), "{err}");
    }

    #[test]
    fn debug_never_prints_the_token() {
        // The token itself never reaches SourceOpts (read fresh per build by
        // `read_token`) — only where it comes from.
        let o = src(r#"{"uri":"quack:h:1","token_env":"VERY_SECRET_ENV_NAME"}"#).unwrap();
        let d = format!("{o:?}");
        assert!(d.contains("VERY_SECRET_ENV_NAME"), "{d}");
        let o = src(r#"{"uri":"quack:h:1","token_file":"/etc/anofox/src-token"}"#).unwrap();
        let d = format!("{o:?}");
        assert!(d.contains("src-token"), "{d}");
    }

    #[test]
    fn redact_removes_every_occurrence() {
        let err = "ATTACH failed: bad token s3cr3t-xyz near s3cr3t-xyz again";
        let r = redact(err, "s3cr3t-xyz");
        assert!(!r.contains("s3cr3t-xyz"), "{r}");
        assert_eq!(r.matches("<redacted>").count(), 2, "{r}");
        // An empty token (misconfiguration) must not turn the whole message
        // into "<redacted>" (`str::replace("", x)` would insert `x`
        // between every byte) — `redact` special-cases it.
        assert_eq!(redact(err, ""), err);
    }

    #[test]
    fn read_token_rejects_an_empty_file_and_trims_whitespace() {
        let dir = std::env::temp_dir().join(format!(
            "anofox-src-token-test-{}",
            anofox_visualization::host::serving::random_token()
        ));
        std::fs::create_dir(&dir).unwrap();
        let empty = dir.join("empty");
        std::fs::write(&empty, "   \n").unwrap();
        let padded = dir.join("padded");
        std::fs::write(&padded, "  topsecret \n").unwrap();
        let opts_empty = src(&format!(
            r#"{{"uri":"quack:h:1","token_file":{:?}}}"#,
            empty.to_string_lossy()
        ))
        .unwrap();
        let err = read_token(&opts_empty).unwrap_err();
        assert!(err.contains("is empty"), "{err}");
        let opts_padded = src(&format!(
            r#"{{"uri":"quack:h:1","token_file":{:?}}}"#,
            padded.to_string_lossy()
        ))
        .unwrap();
        assert_eq!(read_token(&opts_padded).unwrap(), "topsecret");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_token_refuses_a_quote_or_a_control_character() {
        let dir = std::env::temp_dir().join(format!(
            "anofox-src-token-ctrl-test-{}",
            anofox_visualization::host::serving::random_token()
        ));
        std::fs::create_dir(&dir).unwrap();
        for (name, content) in [("quote", "has'quote"), ("ctrl", "has\ncontrol")] {
            let p = dir.join(name);
            std::fs::write(&p, content).unwrap();
            let opts = src(&format!(
                r#"{{"uri":"quack:h:1","token_file":{:?}}}"#,
                p.to_string_lossy()
            ))
            .unwrap();
            let err = read_token(&opts).unwrap_err();
            assert!(
                err.contains("control character") || err.contains("single quote"),
                "{err}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_token_refuses_a_non_ascii_token() {
        // security review R1's own repro token.
        let dir = std::env::temp_dir().join(format!(
            "anofox-src-token-ascii-test-{}",
            anofox_visualization::host::serving::random_token()
        ));
        std::fs::create_dir(&dir).unwrap();
        let p = dir.join("token");
        std::fs::write(&p, "tok-deadbeefö").unwrap();
        let opts = src(&format!(
            r#"{{"uri":"quack:h:1","token_file":{:?}}}"#,
            p.to_string_lossy()
        ))
        .unwrap();
        let err = read_token(&opts).unwrap_err();
        assert!(err.contains("ASCII"), "{err}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn redact_does_not_panic_on_a_non_ascii_token_even_if_called_directly() {
        // security review R1: `redact` itself must never panic, even though
        // `read_token` now refuses a non-ASCII token before anything reaches
        // here — defense in depth for the function the review's repro hit.
        let token = "tok-deadbeefö-rest-of-it-is-long-enough-to-truncate";
        let err = format!("some error mentioning {token} partially cut off at byte 17ish");
        let r = redact(&err, token); // must not panic
        assert!(!r.contains(token), "{r}");
    }

    #[test]
    fn redact_strips_the_quote_doubled_and_truncated_forms_too() {
        // security review I1: the token is embedded as a SQL string literal
        // (`'` doubled), and DuckDB's `LINE 1: ...` context can cut a long
        // literal short, leaving only a prefix of the token visible.
        let token = "s3cr'3t-token-with-a-quote";
        let quoted = token.replace('\'', "''");
        let err = format!("CREATE TEMPORARY SECRET x (TYPE quack, TOKEN '{quoted}')");
        let r = redact(&err, token);
        assert!(!r.contains(&quoted), "{r}");
        assert!(!r.contains(token), "{r}");

        let long_token = "a-very-long-token-value-that-gets-cut-short-by-duckdb";
        let truncated = &long_token[..30];
        let err2 = format!("LINE 1: ...TOKEN '{truncated}");
        let r2 = redact(&err2, long_token);
        assert!(!r2.contains(truncated), "{r2}");
    }

    #[test]
    fn max_rows_and_max_bytes_have_sensible_defaults_and_are_validated() {
        let o = src(r#"{"uri":"quack:h:1","token_env":"T"}"#).unwrap();
        assert!(o.max_rows > 0);
        assert!(o.max_bytes > 0);
        let o2 =
            src(r#"{"uri":"quack:h:1","token_env":"T","max_rows":5,"max_bytes":100}"#).unwrap();
        assert_eq!(o2.max_rows, 5);
        assert_eq!(o2.max_bytes, 100);
        let err = src(r#"{"uri":"quack:h:1","token_env":"T","max_rows":0}"#).unwrap_err();
        assert!(err.contains("max_rows"), "{err}");
        let err2 = src(r#"{"uri":"quack:h:1","token_env":"T","max_bytes":0}"#).unwrap_err();
        assert!(err2.contains("max_bytes"), "{err2}");
    }

    #[test]
    fn the_watchdog_fires_at_the_deadline_not_before() {
        let finished = AtomicBool::new(false);
        let cancel = AtomicBool::new(false);
        // Not yet: `finished` flips before the (generous) deadline.
        let deadline = Instant::now() + Duration::from_secs(10);
        let t0 = Instant::now();
        std::thread::scope(|scope| {
            let f = &finished;
            scope.spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                f.store(true, Ordering::SeqCst);
            });
            assert!(!wait_for_fire(
                &finished,
                &cancel,
                deadline,
                Duration::from_millis(5),
                &|| false
            ));
        });
        assert!(
            t0.elapsed() < Duration::from_secs(1),
            "did not return promptly on finish"
        );

        // Fires once the deadline passes, with nothing marking it finished.
        let finished = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_millis(30);
        let t0 = Instant::now();
        assert!(wait_for_fire(
            &finished,
            &cancel,
            deadline,
            Duration::from_millis(5),
            &|| false
        ));
        let elapsed = t0.elapsed();
        assert!(
            elapsed >= Duration::from_millis(25),
            "fired too early: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "fired too late: {elapsed:?}"
        );

        // Fires immediately when `cancel` is already set, deadline or not.
        let finished = AtomicBool::new(false);
        cancel.store(true, Ordering::SeqCst);
        let far = Instant::now() + Duration::from_secs(600);
        let t0 = Instant::now();
        assert!(wait_for_fire(
            &finished,
            &cancel,
            far,
            Duration::from_millis(5),
            &|| false
        ));
        assert!(
            t0.elapsed() < Duration::from_millis(500),
            "cancel should fire promptly"
        );
    }

    #[test]
    fn the_watchdog_also_fires_when_extra_fire_says_so() {
        // security review R3: the max_bytes file-size poll plugs in here.
        let finished = AtomicBool::new(false);
        let cancel = AtomicBool::new(false);
        let far = Instant::now() + Duration::from_secs(600);
        let t0 = Instant::now();
        assert!(wait_for_fire(
            &finished,
            &cancel,
            far,
            Duration::from_millis(5),
            &|| true
        ));
        assert!(
            t0.elapsed() < Duration::from_millis(500),
            "extra_fire should fire promptly, well before the far deadline"
        );
    }
}
