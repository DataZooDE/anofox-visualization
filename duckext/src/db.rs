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
        if timeout.is_zero() {
            return f(self);
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
        match r {
            Err(_) if fired => Err(format!(
                "query timed out after {:.1}s",
                timeout.as_secs_f64()
            )),
            other => other,
        }
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

/// Run a single-statement query whose rows are converted to JSON by DuckDB
/// (`to_json(row)`), returning a JSON array of row objects. `max_rows` caps the
/// result (exceeding it is an error, not a silent truncation).
pub fn json_rows(
    conn: &Conn,
    query: &str,
    params: &[&str],
    max_rows: usize,
) -> Result<String, String> {
    let q = query.trim().trim_end_matches(';');
    let wrapped = format!(
        "SELECT to_json(__anofox_row)::VARCHAR FROM (\n{q}\n) __anofox_row LIMIT {}",
        max_rows.saturating_add(1)
    );
    let mut res = conn.execute_prepared(&wrapped, params)?;
    let rows = res.string_rows(max_rows)?;
    let mut out = String::from("[");
    for (i, r) in rows.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(r.first().and_then(|v| v.as_deref()).unwrap_or("null"));
    }
    out.push(']');
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
}

impl Drop for Database {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { capi!(duckdb_close)(&mut self.0) }
        }
    }
}
