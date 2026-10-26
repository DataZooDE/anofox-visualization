//! Thin, owned wrappers over the DuckDB C extension API (resolved through the
//! `api()` table — nothing links libduckdb). Every handle is released on Drop.

use crate::api;
use crate::ffi::*;
use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::mpsc;
use std::time::Duration;

macro_rules! capi {
    ($f:ident) => {
        (api()
            .$f
            .expect(concat!("DuckDB C API is missing ", stringify!($f))))
    };
}
pub(crate) use capi;

fn cstring(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| "SQL text contains a NUL byte".to_string())
}

unsafe fn cstr_lossy(p: *const std::os::raw::c_char, fallback: &str) -> String {
    if p.is_null() {
        fallback.to_string()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

/// Read a `duckdb_string_t` (≤12 bytes inline, else a pointer).
pub(crate) unsafe fn read_string(s: &duckdb_string_t) -> &[u8] {
    let len = s.value.pointer.length as usize;
    let p = if len <= 12 {
        s.value.inlined.inlined.as_ptr() as *const u8
    } else {
        s.value.pointer.ptr as *const u8
    };
    std::slice::from_raw_parts(p, len)
}

/// Is row `i` valid in a validity mask (null mask = all valid)?
pub(crate) unsafe fn row_valid(validity: *mut u64, i: idx_t) -> bool {
    validity.is_null() || (*validity.add((i / 64) as usize) >> (i % 64)) & 1 == 1
}

/// An owned query result.
pub struct QueryResult(duckdb_result);

impl QueryResult {
    /// All rows of a result whose columns are VARCHAR (cast in SQL), as strings.
    pub fn string_rows(&mut self, max_rows: usize) -> Result<Vec<Vec<Option<String>>>, String> {
        let mut out = Vec::new();
        unsafe {
            loop {
                let chunk = capi!(duckdb_fetch_chunk)(self.0);
                if chunk.is_null() {
                    break;
                }
                let n = capi!(duckdb_data_chunk_get_size)(chunk);
                let ncol = capi!(duckdb_data_chunk_get_column_count)(chunk);
                let mut cols = Vec::new();
                for c in 0..ncol {
                    let v = capi!(duckdb_data_chunk_get_vector)(chunk, c);
                    cols.push((
                        capi!(duckdb_vector_get_data)(v) as *const duckdb_string_t,
                        capi!(duckdb_vector_get_validity)(v),
                    ));
                }
                for i in 0..n {
                    if out.len() >= max_rows {
                        let mut ch = chunk;
                        capi!(duckdb_destroy_data_chunk)(&mut ch);
                        return Err(format!("result exceeds the row limit ({max_rows})"));
                    }
                    let row = cols
                        .iter()
                        .map(|(data, validity)| {
                            (!data.is_null() && row_valid(*validity, i)).then(|| {
                                String::from_utf8_lossy(read_string(&*data.add(i as usize)))
                                    .into_owned()
                            })
                        })
                        .collect();
                    out.push(row);
                }
                let mut ch = chunk;
                capi!(duckdb_destroy_data_chunk)(&mut ch);
            }
        }
        Ok(out)
    }
}

impl QueryResult {
    /// Call `f` with the first column of every row (a VARCHAR column; `None`
    /// for NULL), chunk by chunk, without collecting the rows.
    pub fn for_each_first(
        &mut self,
        mut f: impl FnMut(Option<&[u8]>) -> Result<(), String>,
    ) -> Result<(), String> {
        unsafe {
            loop {
                let mut chunk = capi!(duckdb_fetch_chunk)(self.0);
                if chunk.is_null() {
                    return Ok(());
                }
                let n = capi!(duckdb_data_chunk_get_size)(chunk);
                let v = capi!(duckdb_data_chunk_get_vector)(chunk, 0);
                let data = capi!(duckdb_vector_get_data)(v) as *const duckdb_string_t;
                let validity = capi!(duckdb_vector_get_validity)(v);
                let mut r = Ok(());
                for i in 0..n {
                    let val = (!data.is_null() && row_valid(validity, i))
                        .then(|| read_string(&*data.add(i as usize)));
                    r = f(val);
                    if r.is_err() {
                        break;
                    }
                }
                capi!(duckdb_destroy_data_chunk)(&mut chunk);
                r?;
            }
        }
    }
}

impl Drop for QueryResult {
    fn drop(&mut self) {
        unsafe { capi!(duckdb_destroy_result)(&mut self.0) }
    }
}

/// A connection. `owned` connections are disconnected on Drop.
pub struct Conn {
    pub(crate) raw: duckdb_connection,
    owned: bool,
}

// A duckdb_connection may be used from any thread, one at a time (callers
// serialise access with a Mutex or use one connection per request).
unsafe impl Send for Conn {}

impl Conn {
    /// Wrap a connection we own (disconnect on drop).
    pub unsafe fn owned(raw: duckdb_connection) -> Conn {
        Conn { raw, owned: true }
    }

    pub fn connect(db: duckdb_database) -> Result<Conn, String> {
        let mut raw: duckdb_connection = ptr::null_mut();
        unsafe {
            if capi!(duckdb_connect)(db, &mut raw) != duckdb_state::DuckDBSuccess {
                return Err("could not open a DuckDB connection".into());
            }
            Ok(Conn::owned(raw))
        }
    }

    /// Run one or more statements for effect (`duckdb_query`).
    pub fn exec(&self, sql: &str) -> Result<(), String> {
        self.query(sql).map(|_| ())
    }

    /// Run SQL and return the (last statement's) result.
    pub fn query(&self, sql: &str) -> Result<QueryResult, String> {
        let c = cstring(sql)?;
        unsafe {
            let mut r: duckdb_result = std::mem::zeroed();
            let rc = capi!(duckdb_query)(self.raw, c.as_ptr(), &mut r);
            let res = QueryResult(r);
            if rc != duckdb_state::DuckDBSuccess {
                let mut res = res;
                return Err(cstr_lossy(
                    capi!(duckdb_result_error)(&mut res.0),
                    "query failed",
                ));
            }
            Ok(res)
        }
    }

    /// Prepare exactly ONE statement (DuckDB refuses to prepare several), bind
    /// `params` as VARCHAR parameters, and execute it.
    pub fn execute_prepared(&self, sql: &str, params: &[&str]) -> Result<QueryResult, String> {
        let p = Prepared::new(self, sql)?;
        p.bind_strs(params)?;
        p.execute()
    }

    /// Number of statements DuckDB's own parser finds in `sql`.
    pub fn count_statements(&self, sql: &str) -> Result<usize, String> {
        Ok(self.extract(sql)?.count)
    }

    /// Parse with DuckDB (`duckdb_extract_statements`).
    pub fn extract(&self, sql: &str) -> Result<Extracted, String> {
        let c = cstring(sql)?;
        unsafe {
            let mut ex: duckdb_extracted_statements = ptr::null_mut();
            let n = capi!(duckdb_extract_statements)(self.raw, c.as_ptr(), &mut ex);
            let e = Extracted {
                raw: ex,
                count: n as usize,
            };
            if n == 0 {
                let err = capi!(duckdb_extract_statements_error)(ex);
                if !err.is_null() {
                    return Err(cstr_lossy(err, "parse error"));
                }
            }
            Ok(e)
        }
    }

    /// Interrupt the running query (safe to call from another thread).
    pub fn interrupter(&self) -> Interrupter {
        Interrupter(self.raw)
    }

    /// Run `f` with a watchdog that interrupts this connection after `timeout`
    /// (zero = no timeout). An interrupted query surfaces as a timeout error.
    pub fn with_deadline<T>(
        &self,
        timeout: Duration,
        f: impl FnOnce(&Conn) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_deadline_fired(timeout, f).0
    }

    /// [`Conn::with_deadline`], also telling whether the watchdog fired —
    /// even when `f` still returned `Ok` (it finished as the deadline hit), a
    /// fired interrupt may be pending on the connection, so it must not be
    /// reused.
    pub fn with_deadline_fired<T>(
        &self,
        timeout: Duration,
        f: impl FnOnce(&Conn) -> Result<T, String>,
    ) -> (Result<T, String>, bool) {
        if timeout.is_zero() {
            return (f(self), false);
        }
        let (tx, rx) = mpsc::channel::<()>();
        let intr = self.interrupter();
        let watchdog = std::thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(timeout) {
                intr.interrupt();
                true
            } else {
                false
            }
        });
        let r = f(self);
        let _ = tx.send(());
        let fired = watchdog.join().unwrap_or(false);
        let r = match r {
            Err(_) if fired => Err(format!(
                "query timed out after {:.1}s",
                timeout.as_secs_f64()
            )),
            other => other,
        };
        (r, fired)
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        if self.owned && !self.raw.is_null() {
            unsafe { capi!(duckdb_disconnect)(&mut self.raw) }
        }
    }
}

/// A handle that can interrupt a connection from another thread.
#[derive(Clone, Copy)]
pub struct Interrupter(duckdb_connection);
unsafe impl Send for Interrupter {}
impl Interrupter {
    pub fn interrupt(&self) {
        unsafe { capi!(duckdb_interrupt)(self.0) }
    }
}

/// Statements extracted by DuckDB's parser.
pub struct Extracted {
    raw: duckdb_extracted_statements,
    pub count: usize,
}

impl Extracted {
    pub fn prepare(&self, conn: &Conn, i: usize) -> Result<Prepared, String> {
        unsafe {
            let mut st: duckdb_prepared_statement = ptr::null_mut();
            let rc =
                capi!(duckdb_prepare_extracted_statement)(conn.raw, self.raw, i as idx_t, &mut st);
            let p = Prepared(st);
            if rc != duckdb_state::DuckDBSuccess {
                return Err(cstr_lossy(
                    capi!(duckdb_prepare_error)(p.0),
                    "prepare failed",
                ));
            }
            Ok(p)
        }
    }
}

impl Drop for Extracted {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { capi!(duckdb_destroy_extracted)(&mut self.raw) }
        }
    }
}

/// A prepared statement.
pub struct Prepared(duckdb_prepared_statement);

impl Prepared {
    pub fn new(conn: &Conn, sql: &str) -> Result<Prepared, String> {
        let c = cstring(sql)?;
        unsafe {
            let mut st: duckdb_prepared_statement = ptr::null_mut();
            let rc = capi!(duckdb_prepare)(conn.raw, c.as_ptr(), &mut st);
            let p = Prepared(st);
            if rc != duckdb_state::DuckDBSuccess {
                return Err(cstr_lossy(
                    capi!(duckdb_prepare_error)(p.0),
                    "prepare failed",
                ));
            }
            Ok(p)
        }
    }

    pub fn statement_type(&self) -> duckdb_statement_type {
        unsafe { capi!(duckdb_prepared_statement_type)(self.0) }
    }

    pub fn bind_strs(&self, params: &[&str]) -> Result<(), String> {
        for (i, v) in params.iter().enumerate() {
            let c = cstring(v)?;
            unsafe {
                if capi!(duckdb_bind_varchar)(self.0, (i + 1) as idx_t, c.as_ptr())
                    != duckdb_state::DuckDBSuccess
                {
                    return Err(format!("could not bind parameter {}", i + 1));
                }
            }
        }
        Ok(())
    }

    pub fn execute(&self) -> Result<QueryResult, String> {
        unsafe {
            let mut r: duckdb_result = std::mem::zeroed();
            let rc = capi!(duckdb_execute_prepared)(self.0, &mut r);
            let mut res = QueryResult(r);
            if rc != duckdb_state::DuckDBSuccess {
                return Err(cstr_lossy(
                    capi!(duckdb_result_error)(&mut res.0),
                    "execute failed",
                ));
            }
            Ok(res)
        }
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { capi!(duckdb_destroy_prepare)(&mut self.0) }
        }
    }
}

/// An Appender (#600): DuckDB's bulk-load path for one table, bypassing SQL
/// text entirely — no `INSERT ... VALUES (…), (…), …` string to build,
/// parse or bind. Used instead of [`Prepared`] wherever a caller is
/// inserting many rows it already has as typed values, not running a
/// query a user wrote.
pub struct Appender(duckdb_appender);

impl Appender {
    /// Names the catalog/schema explicitly (`duckdb_appender_create_ext`)
    /// — pass `None`/`None` to leave them to the connection's own default
    /// catalog/`search_path`, same as `duckdb_appender_create` would, but
    /// review of #611 N2: there is no separate, catalog-less constructor
    /// here, because the one caller this type exists for must never use
    /// one. For a `TEMP` table (#600: `viewer_state`), the connection's
    /// own current default can resolve to a different, `READ_ONLY`-
    /// attached catalog's own idea of the current database instead
    /// (unlike ordinary SQL, which already resolves an unqualified name
    /// to `temp` first — #548 item 1's own pinned test) — observed,
    /// against a real locked server, to error "Cannot append to a
    /// readonly database" rather than silently resolving to the wrong
    /// table.
    pub fn new_in(
        conn: &Conn,
        catalog: Option<&str>,
        schema: Option<&str>,
        table: &str,
    ) -> Result<Appender, String> {
        let t = cstring(table)?;
        let cat_c = catalog.map(cstring).transpose()?;
        let schema_c = schema.map(cstring).transpose()?;
        let cat_ptr = cat_c.as_ref().map_or(ptr::null(), |c| c.as_ptr());
        let schema_ptr = schema_c.as_ref().map_or(ptr::null(), |c| c.as_ptr());
        unsafe {
            let mut raw: duckdb_appender = ptr::null_mut();
            let rc = capi!(duckdb_appender_create_ext)(
                conn.raw,
                cat_ptr,
                schema_ptr,
                t.as_ptr(),
                &mut raw,
            );
            let a = Appender(raw);
            if rc != duckdb_state::DuckDBSuccess {
                return Err(if raw.is_null() {
                    format!("could not create an appender for '{table}'")
                } else {
                    cstr_lossy(capi!(duckdb_appender_error)(a.0), "appender create failed")
                });
            }
            Ok(a)
        }
    }

    fn check(&self, rc: duckdb_state, what: &str) -> Result<(), String> {
        if rc == duckdb_state::DuckDBSuccess {
            Ok(())
        } else {
            Err(unsafe { cstr_lossy(capi!(duckdb_appender_error)(self.0), what) })
        }
    }

    pub fn begin_row(&self) -> Result<(), String> {
        self.check(
            unsafe { capi!(duckdb_appender_begin_row)(self.0) },
            "begin_row failed",
        )
    }

    pub fn end_row(&self) -> Result<(), String> {
        self.check(
            unsafe { capi!(duckdb_appender_end_row)(self.0) },
            "end_row failed",
        )
    }

    pub fn append_null(&self) -> Result<(), String> {
        self.check(
            unsafe { capi!(duckdb_append_null)(self.0) },
            "append_null failed",
        )
    }

    pub fn append_int64(&self, v: i64) -> Result<(), String> {
        self.check(
            unsafe { capi!(duckdb_append_int64)(self.0, v) },
            "append_int64 failed",
        )
    }

    pub fn append_timestamp_micros(&self, micros: i64) -> Result<(), String> {
        self.check(
            unsafe { capi!(duckdb_append_timestamp)(self.0, duckdb_timestamp { micros }) },
            "append_timestamp failed",
        )
    }

    pub fn append_varchar(&self, v: &str) -> Result<(), String> {
        let c = cstring(v)?;
        self.check(
            unsafe { capi!(duckdb_append_varchar)(self.0, c.as_ptr()) },
            "append_varchar failed",
        )
    }

    /// `VARCHAR[]`, each element either a string or SQL `NULL` (#548 item 3
    /// nit: a `NULL` list item must stay a real `NULL`, not an empty
    /// string) — the one column [`Appender`]'s simple per-value functions
    /// above have no direct call for, so this builds one `duckdb_value`
    /// per element plus the list `duckdb_value` itself
    /// (`duckdb_create_list_value`), appends that, and destroys every
    /// value it created, in the same call.
    pub fn append_varchar_list(&self, items: &[Option<&str>]) -> Result<(), String> {
        // Checked up front, before any `duckdb_value` is created: a NUL
        // byte in one item (rare — a literal NUL inside recorded text)
        // must not leave an earlier item's already-created `duckdb_value`
        // leaked by an early return partway through the loop below.
        let cstrs: Vec<Option<CString>> = items
            .iter()
            .map(|it| it.map(cstring).transpose())
            .collect::<Result<_, _>>()?;
        unsafe {
            let mut elem_type = capi!(duckdb_create_logical_type)(duckdb_type::DUCKDB_TYPE_VARCHAR);
            let mut values: Vec<duckdb_value> = cstrs
                .iter()
                .map(|c| match c {
                    Some(c) => capi!(duckdb_create_varchar)(c.as_ptr()),
                    None => capi!(duckdb_create_null_value)(),
                })
                .collect();
            let mut list_val = capi!(duckdb_create_list_value)(
                elem_type,
                values.as_mut_ptr(),
                values.len() as idx_t,
            );
            // #611 N1: `duckdb_create_list_value` returns NULL on failure
            // (a NULL element value, or a type mismatch) — unreachable
            // today (every element above is a valid UTF-8 `&str` turned
            // into a real `duckdb_value`, and `elem_type` always matches),
            // but `duckdb_append_value` would otherwise dereference a
            // NULL `duckdb_value` if that ever stopped holding. Clean up
            // what was already allocated and refuse instead of calling it.
            if list_val.is_null() {
                for v in &mut values {
                    capi!(duckdb_destroy_value)(v);
                }
                capi!(duckdb_destroy_logical_type)(&mut elem_type);
                return Err("could not build the list value to append".into());
            }
            let rc = capi!(duckdb_append_value)(self.0, list_val);
            let err = if rc != duckdb_state::DuckDBSuccess {
                Some(cstr_lossy(
                    capi!(duckdb_appender_error)(self.0),
                    "append_value (list) failed",
                ))
            } else {
                None
            };
            capi!(duckdb_destroy_value)(&mut list_val);
            for v in &mut values {
                capi!(duckdb_destroy_value)(v);
            }
            capi!(duckdb_destroy_logical_type)(&mut elem_type);
            match err {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
    }

    /// Flush and close explicitly, so a failure on the final batch's write
    /// (disk full, a constraint violation DuckDB only catches at close)
    /// surfaces as an `Err` here — not silently swallowed by [`Drop`],
    /// which only destroys the handle (closing it first if this was never
    /// called, but discarding whatever that implicit close returns).
    pub fn close(&self) -> Result<(), String> {
        self.check(
            unsafe { capi!(duckdb_appender_close)(self.0) },
            "appender close failed",
        )
    }
}

impl Drop for Appender {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { capi!(duckdb_appender_destroy)(&mut self.0) };
        }
    }
}

/// Run a single-statement query whose rows are converted to JSON by DuckDB
/// (`to_json(row)`), returning a JSON array of row objects. `max_rows` caps the
/// result (exceeding it is an error, not a silent truncation). The array is
/// written row by row. Before its buffer grows, `grow(len, wanted)` is asked
/// for a capacity for a `len`-byte result: it returns the capacity to allocate
/// (`len..=wanted`), or an `Err` that stops the query (a size cap, a memory
/// budget). The result has no spare capacity.
pub fn json_rows(
    conn: &Conn,
    query: &str,
    params: &[&str],
    max_rows: usize,
    grow: &mut dyn FnMut(usize, usize) -> Result<usize, String>,
) -> Result<String, String> {
    let q = query.trim().trim_end_matches(';');
    let wrapped = format!(
        "SELECT to_json(__anofox_row)::VARCHAR FROM (\n{q}\n) __anofox_row LIMIT {}",
        max_rows.saturating_add(1)
    );
    let mut res = conn.execute_prepared(&wrapped, params)?;
    let mut out = String::new();
    let mut push = |s: &str| -> Result<(), String> {
        let len = out.len() + s.len();
        if len > out.capacity() {
            let cap = grow(len, len.max(out.capacity() * 2).max(4096))?.max(len);
            out.reserve_exact(cap - out.len());
        }
        out.push_str(s);
        Ok(())
    };
    push("[")?;
    let mut rows = 0usize;
    res.for_each_first(|v| {
        if rows == max_rows {
            return Err(format!("result exceeds the row limit ({max_rows})"));
        }
        if rows > 0 {
            push(",")?;
        }
        rows += 1;
        // Non-finite doubles (a sum that overflowed, 'nan'::DOUBLE) come out
        // of to_json as bare Infinity / NaN, which is not JSON: make them null.
        let row = String::from_utf8_lossy(v.unwrap_or(b"null"));
        push(&anofox_visualization::sql::sanitize_json_numbers(&row))
    })?;
    push("]")?;
    out.shrink_to_fit();
    Ok(out)
}

/// An owned database handle (closed on Drop — after all connections are gone).
pub struct Database(pub(crate) duckdb_database);
unsafe impl Send for Database {}
unsafe impl Sync for Database {}

impl Database {
    /// Open `path` with the given `(key, value)` config options.
    pub fn open(path: &str, config: &[(&str, &str)]) -> Result<Database, String> {
        unsafe {
            let mut cfg: duckdb_config = ptr::null_mut();
            if capi!(duckdb_create_config)(&mut cfg) != duckdb_state::DuckDBSuccess {
                return Err("could not create a DuckDB config".into());
            }
            for (k, v) in config {
                let (kc, vc) = (cstring(k)?, cstring(v)?);
                if capi!(duckdb_set_config)(cfg, kc.as_ptr(), vc.as_ptr())
                    != duckdb_state::DuckDBSuccess
                {
                    capi!(duckdb_destroy_config)(&mut cfg);
                    return Err(format!("could not set config option {k}"));
                }
            }
            let pc = cstring(path)?;
            let mut db: duckdb_database = ptr::null_mut();
            let mut err: *mut std::os::raw::c_char = ptr::null_mut();
            let rc = capi!(duckdb_open_ext)(pc.as_ptr(), &mut db, cfg, &mut err);
            capi!(duckdb_destroy_config)(&mut cfg);
            if rc != duckdb_state::DuckDBSuccess {
                let msg = cstr_lossy(err, "could not open database");
                if !err.is_null() {
                    capi!(duckdb_free)(err as *mut std::ffi::c_void);
                }
                return Err(msg);
            }
            Ok(Database(db))
        }
    }

    pub fn connect(&self) -> Result<Conn, String> {
        Conn::connect(self.0)
    }

    /// A throwaway `:memory:` staging instance for a Quack source build
    /// (#352): extension autoload/autoinstall and community extensions are
    /// off, so nothing loads here except `quack` itself, loaded explicitly
    /// right after by the caller. `LOAD quack` on the result fails clearly
    /// ("install it first: INSTALL quack") when it isn't on disk yet, rather
    /// than DuckDB's generic "extension not found" — autoinstall is off on
    /// purpose, so a staging build never reaches out to a repository itself.
    pub fn open_quack_staging() -> Result<Database, String> {
        Database::open(
            ":memory:",
            &[
                ("autoinstall_known_extensions", "false"),
                ("autoload_known_extensions", "false"),
                ("allow_community_extensions", "false"),
                // Security review R2: this instance only ever drives one
                // remote Quack stream at a time; DuckDB's default thread
                // count otherwise means an abandoned build (left running
                // past its deadline, #352 M1) leaks on the order of 100
                // threads instead of a handful.
                ("threads", "2"),
            ],
        )
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { capi!(duckdb_close)(&mut self.0) }
        }
    }
}
