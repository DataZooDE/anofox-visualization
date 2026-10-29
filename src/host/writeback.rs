//! Shared write-back (`options.writeback`, #684 slice 2): config parsing and
//! validation, the writer check, `POST /api/save`'s request, and the change
//! set one save appends to the change log. Pure — the locked server
//! (`duckext/src/serve.rs`, `handle_save`) does the HTTP, the gates and the
//! append; this module is unit-tested on its own.
//!
//! The security model: a writer (a personal sign-in in `writers`) may write
//! the rows of the **declared** tables only, in their declared columns and
//! types — never client SQL. What a save writes comes from the dashboard's
//! own `::SAVE` statement (re-run on the server, as the viewer) plus the
//! page's edited values (`graph` or `form` in the request), each coerced to
//! its declared type here. The change log itself is the #517 [`Store`]
//! (`Store::open_writeback`): one line per save, `fdatasync`'d before the
//! `201`.
//!
//! [`Store`]: crate::host::state::Store

use crate::host::serving::{valid_dashboard_id, valid_ident, valid_submit_id};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A declared column's type — the few a model table needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColType {
    Varchar,
    Integer,
    Bigint,
    Double,
    Boolean,
    Date,
}

impl ColType {
    fn parse(s: &str) -> Option<ColType> {
        Some(match s.trim().to_ascii_uppercase().as_str() {
            "VARCHAR" | "TEXT" => ColType::Varchar,
            "INTEGER" | "INT" => ColType::Integer,
            "BIGINT" => ColType::Bigint,
            "DOUBLE" => ColType::Double,
            "BOOLEAN" | "BOOL" => ColType::Boolean,
            "DATE" => ColType::Date,
            _ => return None,
        })
    }

    /// The DuckDB type name (slice 3 materialises the tables with it).
    pub fn sql(self) -> &'static str {
        match self {
            ColType::Varchar => "VARCHAR",
            ColType::Integer => "INTEGER",
            ColType::Bigint => "BIGINT",
            ColType::Double => "DOUBLE",
            ColType::Boolean => "BOOLEAN",
            ColType::Date => "DATE",
        }
    }
}

/// One `options.writeback.tables` entry: its key columns and every column a
/// save may write, in declaration order.
#[derive(Debug, Clone, PartialEq)]
pub struct TableDecl {
    pub key: Vec<String>,
    pub columns: Vec<(String, ColType)>,
}

impl TableDecl {
    fn col(&self, name: &str) -> Option<ColType> {
        self.columns
            .iter()
            .find(|(c, _)| c == name)
            .map(|(_, t)| *t)
    }
}

/// Parsed, structurally valid `options.writeback`.
#[derive(Debug, Clone, PartialEq)]
pub struct WritebackOptions {
    pub path: PathBuf,
    pub dashboards: Vec<String>,
    pub writer_groups: Vec<String>,
    pub writer_users: Vec<String>,
    pub tables: BTreeMap<String, TableDecl>,
    pub max_rows: usize,
    pub text_max_chars: usize,
    pub rate_per_minute: u32,
    pub rate_burst: u32,
    pub max_mb: u64,
}

pub const DEFAULT_MAX_ROWS: usize = 2000;
pub const MAX_MAX_ROWS: usize = 100_000;
pub const DEFAULT_RATE_PER_MINUTE: u32 = 12;
pub const DEFAULT_RATE_BURST: u32 = 5;
pub const DEFAULT_MAX_MB: u64 = 256;
/// `POST /api/save`'s own body cap: a whole edited graph fits; nothing
/// else ever flows through this endpoint.
pub const SAVE_MAX_BODY: usize = 1 << 20;

const KEYS: &str = "path, dashboards, writers, tables, max_rows, text_max_chars, rate, max_mb";

impl WritebackOptions {
    pub fn parse(v: &Value) -> Result<WritebackOptions, String> {
        let m = v.as_object().ok_or("options.writeback must be an object")?;
        let str_list = |val: &Value, key: &str| -> Result<Vec<String>, String> {
            val.as_array()
                .ok_or_else(|| format!("options.writeback.{key} must be an array of strings"))?
                .iter()
                .map(|x| {
                    x.as_str().map(str::to_string).ok_or_else(|| {
                        format!("options.writeback.{key}: every entry must be a string")
                    })
                })
                .collect()
        };
        let pos = |val: &Value, key: &str| -> Result<u64, String> {
            val.as_u64()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("options.writeback.{key} must be a positive integer"))
        };
        let mut o = WritebackOptions {
            path: PathBuf::new(),
            dashboards: Vec::new(),
            writer_groups: Vec::new(),
            writer_users: Vec::new(),
            tables: BTreeMap::new(),
            max_rows: DEFAULT_MAX_ROWS,
            text_max_chars: crate::host::state::DEFAULT_TEXT_MAX_CHARS,
            rate_per_minute: DEFAULT_RATE_PER_MINUTE,
            rate_burst: DEFAULT_RATE_BURST,
            max_mb: DEFAULT_MAX_MB,
        };
        let mut has_path = false;
        for (k, val) in m {
            match k.as_str() {
                "path" => {
                    let p = val
                        .as_str()
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .ok_or("options.writeback.path must be a file path")?;
                    o.path = PathBuf::from(p);
                    has_path = true;
                }
                "dashboards" => o.dashboards = str_list(val, "dashboards")?,
                "writers" => {
                    let wm = val.as_object().ok_or(
                        "options.writeback.writers must be an object like \
                         {\"groups\": […], \"users\": […]}",
                    )?;
                    if let Some(other) = wm
                        .keys()
                        .find(|k| !matches!(k.as_str(), "groups" | "users"))
                    {
                        return Err(format!(
                            "options.writeback.writers: unknown key '{other}' (groups, users)"
                        ));
                    }
                    if let Some(g) = wm.get("groups") {
                        o.writer_groups = str_list(g, "writers.groups")?;
                    }
                    if let Some(u) = wm.get("users") {
                        o.writer_users = str_list(u, "writers.users")?;
                    }
                }
                "tables" => {
                    let tm = val.as_object().ok_or(
                        "options.writeback.tables must be an object: table name → \
                         {\"key\": […], \"columns\": {name: type}}",
                    )?;
                    for (name, decl) in tm {
                        o.tables.insert(name.clone(), parse_table(name, decl)?);
                    }
                }
                "max_rows" => o.max_rows = (pos(val, "max_rows")? as usize).min(MAX_MAX_ROWS),
                "text_max_chars" => {
                    o.text_max_chars = (pos(val, "text_max_chars")? as usize)
                        .min(crate::host::state::MAX_TEXT_MAX_CHARS)
                }
                "rate" => {
                    let rm = val.as_object().ok_or(
                        "options.writeback.rate must be an object like \
                         {\"per_minute\": 12, \"burst\": 5}",
                    )?;
                    if let Some(other) = rm
                        .keys()
                        .find(|k| !matches!(k.as_str(), "per_minute" | "burst"))
                    {
                        return Err(format!(
                            "options.writeback.rate: unknown key '{other}' (per_minute, burst)"
                        ));
                    }
                    if let Some(v) = rm.get("per_minute") {
                        o.rate_per_minute = pos(v, "rate.per_minute")?.min(10_000) as u32;
                    }
                    if let Some(v) = rm.get("burst") {
                        o.rate_burst = pos(v, "rate.burst")?.min(10_000) as u32;
                    }
                }
                "max_mb" => o.max_mb = pos(val, "max_mb")?.min(1 << 20),
                other => {
                    return Err(format!("options.writeback: unknown key '{other}' ({KEYS})"));
                }
            }
        }
        if !has_path {
            return Err("options.writeback.path is required".into());
        }
        if o.dashboards.is_empty() {
            return Err("options.writeback.dashboards must name at least one dashboard".into());
        }
        if o.writer_groups.is_empty() && o.writer_users.is_empty() {
            return Err("options.writeback.writers needs groups or users \
                        (otherwise no one could ever save)"
                .into());
        }
        if o.tables.is_empty() {
            return Err("options.writeback.tables must declare at least one table".into());
        }
        Ok(o)
    }

    /// The start-time refusals that need the rest of the server's config.
    pub fn validate_start(&self, ctx: &StartContext) -> Result<(), String> {
        if !ctx.has_auth {
            return Err(
                "options.writeback needs options.auth: every save carries a \
                        signed-in writer's identity"
                    .into(),
            );
        }
        if ctx.embed_without_frame_ancestors {
            return Err("options.writeback with an embed login needs an explicit \
                 options.frame_ancestors — without it any site may frame the save button"
                .into());
        }
        for d in &self.dashboards {
            if !ctx.known_dashboards.iter().any(|k| k == d) {
                return Err(format!(
                    "options.writeback names dashboard '{d}', which this server doesn't serve"
                ));
            }
        }
        if ctx.state_path.is_some_and(|p| p == self.path) {
            return Err("options.writeback.path must not be options.state.path".into());
        }
        crate::host::state::check_path_rules(&self.path, ctx.data_dir, ctx.dashboard_dir)
            .map_err(|e| e.replace("options.state.path", "options.writeback.path"))
    }

    /// May this signed-in user save? (The caller has already refused API
    /// keys and reserved/shared identities.)
    pub fn is_writer(&self, user: &str, groups: &[String]) -> bool {
        self.writer_users.iter().any(|u| u == user)
            || groups
                .iter()
                .any(|g| self.writer_groups.iter().any(|w| w == g))
    }

    pub fn enabled_for(&self, dashboard: &str) -> bool {
        self.dashboards.iter().any(|d| d == dashboard)
    }
}

fn parse_table(name: &str, decl: &Value) -> Result<TableDecl, String> {
    if !valid_ident(name) {
        return Err(format!(
            "options.writeback.tables: '{name}' is not a plain table name"
        ));
    }
    let o = decl.as_object().ok_or_else(|| {
        format!("options.writeback.tables.{name} must be {{\"key\": […], \"columns\": {{…}}}}")
    })?;
    if let Some(other) = o.keys().find(|k| !matches!(k.as_str(), "key" | "columns")) {
        return Err(format!(
            "options.writeback.tables.{name}: unknown key '{other}' (key, columns)"
        ));
    }
    let cm = o
        .get("columns")
        .and_then(Value::as_object)
        .filter(|c| !c.is_empty())
        .ok_or_else(|| {
            format!("options.writeback.tables.{name}.columns must map column names to types")
        })?;
    let mut columns = Vec::with_capacity(cm.len());
    for (c, t) in cm {
        if !valid_ident(c) {
            return Err(format!(
                "options.writeback.tables.{name}: '{c}' is not a plain column name"
            ));
        }
        let ty = t.as_str().and_then(ColType::parse).ok_or_else(|| {
            format!(
                "options.writeback.tables.{name}.columns.{c}: the type must be one of \
                 VARCHAR, INTEGER, BIGINT, DOUBLE, BOOLEAN, DATE"
            )
        })?;
        columns.push((c.clone(), ty));
    }
    let key: Vec<String> = o
        .get("key")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            format!("options.writeback.tables.{name}.key must be an array of column names")
        })?
        .iter()
        .map(|k| k.as_str().map(str::to_string))
        .collect::<Option<_>>()
        .ok_or_else(|| {
            format!("options.writeback.tables.{name}.key must be an array of column names")
        })?;
    if key.is_empty() {
        return Err(format!(
            "options.writeback.tables.{name}.key must name at least one column"
        ));
    }
    for k in &key {
        if !columns.iter().any(|(c, _)| c == k) {
            return Err(format!(
                "options.writeback.tables.{name}.key: '{k}' is not one of its columns"
            ));
        }
    }
    Ok(TableDecl { key, columns })
}

/// What [`WritebackOptions::validate_start`] needs from the server.
pub struct StartContext<'a> {
    pub has_auth: bool,
    pub embed_without_frame_ancestors: bool,
    pub data_dir: Option<&'a Path>,
    pub dashboard_dir: &'a Path,
    pub known_dashboards: &'a [String],
    pub state_path: Option<&'a Path>,
}

// ─────────────────────────────────────────────────────────────── request ──

/// `POST /api/save`'s body: `{"dashboard", "panel", "id", "vars"}` like
/// `/api/submit` (the `::SAVE` statement re-runs with `vars`), plus the
/// edited values — `graph` (an edited `::DIAGRAM`'s `{nodes, edges}`) or
/// `form` (a `::FORM`'s values) with its row `key` — and `base`, the change
/// log generation the page was showing (recorded; slice 4 refuses a stale one).
#[derive(Debug, Clone)]
pub struct SaveRequest {
    pub dashboard: String,
    pub panel: usize,
    pub id: String,
    pub base: Option<u64>,
    pub graph: Option<Value>,
    pub form: Option<Map<String, Value>>,
    pub key: Option<String>,
}

pub fn parse_save_request(body: &str) -> Result<SaveRequest, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad request JSON: {e}"))?;
    let o = v.as_object().ok_or("request must be a JSON object")?;
    for k in o.keys() {
        if !matches!(
            k.as_str(),
            "dashboard" | "panel" | "id" | "vars" | "base" | "graph" | "form" | "key"
        ) {
            return Err(format!("unknown request field '{k}'"));
        }
    }
    // The same variable rules as a panel request (the `::SAVE` re-runs with them).
    crate::host::serving::parse_vars_object(o.get("vars"))?;
    let dashboard = o
        .get("dashboard")
        .and_then(Value::as_str)
        .filter(|d| valid_dashboard_id(d))
        .ok_or("'dashboard' must be a dashboard id")?
        .to_string();
    let panel = o
        .get("panel")
        .and_then(Value::as_u64)
        .ok_or("'panel' must be a non-negative integer")? as usize;
    let id = o
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| valid_submit_id(id))
        .ok_or("'id' must be 1 to 64 hex digits")?
        .to_string();
    let base = match o.get("base") {
        None | Some(Value::Null) => None,
        Some(b) => Some(b.as_u64().ok_or("'base' must be a non-negative integer")?),
    };
    let graph = o.get("graph").filter(|g| !g.is_null()).cloned();
    let form = match o.get("form") {
        None | Some(Value::Null) => None,
        Some(f) => Some(f.as_object().cloned().ok_or("'form' must be an object")?),
    };
    let key = match o.get("key") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(_) => return Err("'key' must be a string".into()),
    };
    Ok(SaveRequest {
        dashboard,
        panel,
        id,
        base,
        graph,
        form,
        key,
    })
}

// ───────────────────────────────────────────────────────────── change set ──

/// What the dashboard's `::SAVE` row says to save (read on the server from
/// the re-run statement, never from the request).
#[derive(Debug, Clone, PartialEq)]
pub enum SaveTarget {
    /// `::SAVE_GRAPH`: nodes into `tables[0]`, edges into `tables[1]`.
    Graph { nodes: String, edges: String },
    /// `::SAVE_FORM`: one row keyed by `id` into the table.
    Form { table: String },
}

/// Read a `::SAVE` row's `::SAVE_GRAPH` / `::SAVE_FORM` / `::SAVE_TABLES`
/// cell values into a [`SaveTarget`] over declared tables only.
pub fn save_target(
    opts: &WritebackOptions,
    graph_var: &str,
    form_var: &str,
    tables: &str,
) -> Result<SaveTarget, String> {
    let names: Vec<&str> = tables
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();
    for t in &names {
        if !opts.tables.contains_key(*t) {
            return Err(format!(
                "'{t}' is not a table this server accepts saves into"
            ));
        }
    }
    match (graph_var.trim().is_empty(), form_var.trim().is_empty()) {
        (false, true) => match names.as_slice() {
            [n, e] => Ok(SaveTarget::Graph {
                nodes: n.to_string(),
                edges: e.to_string(),
            }),
            _ => Err("a graph saves into two tables: nodes, edges".into()),
        },
        (true, false) => match names.as_slice() {
            [t] => Ok(SaveTarget::Form {
                table: t.to_string(),
            }),
            _ => Err("a form saves into one table".into()),
        },
        _ => Err("a ::SAVE names either ::SAVE_GRAPH or ::SAVE_FORM".into()),
    }
}

/// Build the change set one save appends: `{"changes": [{"table", "op",
/// "rows"}…], "base": n|null}`. A graph replaces both its tables whole; a
/// form upserts one row. Every value is coerced to its declared type; an
/// undeclared field, a missing key or a duplicate key refuses the save.
pub fn build_changes(
    opts: &WritebackOptions,
    target: &SaveTarget,
    req: &SaveRequest,
) -> Result<Map<String, Value>, String> {
    let max = opts.max_rows;
    let text = opts.text_max_chars;
    let mut changes = Vec::new();
    match target {
        SaveTarget::Graph { nodes, edges } => {
            let g = req
                .graph
                .as_ref()
                .ok_or("this ::SAVE saves a graph: 'graph' is missing")?;
            let arr = |k: &str| -> Result<&Vec<Value>, String> {
                g.get(k)
                    .and_then(Value::as_array)
                    .ok_or_else(|| format!("'graph.{k}' must be an array"))
            };
            let (ns, es) = (arr("nodes")?, arr("edges")?);
            if ns.len() + es.len() > max {
                return Err(format!("a save writes at most {max} rows"));
            }
            let nd = &opts.tables[nodes];
            let ed = &opts.tables[edges];
            let mut node_rows = Vec::with_capacity(ns.len());
            for n in ns {
                let n = n.as_object().ok_or("every graph node must be an object")?;
                // The graph's own shape; a field the table doesn't declare
                // (other than `id`) is simply not stored.
                let mut fields = Map::new();
                for f in ["id", "type", "label", "stage"] {
                    if f == "id" || nd.col(f).is_some() {
                        fields.insert(f.into(), n.get(f).cloned().unwrap_or(Value::Null));
                    }
                }
                node_rows.push(typed_row(nodes, nd, &fields, text)?);
            }
            let mut edge_rows = Vec::with_capacity(es.len());
            for e in es {
                let e = e.as_object().ok_or("every graph edge must be an object")?;
                let mut fields = Map::new();
                for f in ["from", "to"] {
                    fields.insert(f.into(), e.get(f).cloned().unwrap_or(Value::Null));
                }
                edge_rows.push(typed_row(edges, ed, &fields, text)?);
            }
            unique_keys(nodes, nd, &node_rows)?;
            unique_keys(edges, ed, &edge_rows)?;
            changes.push(change(nodes, "replace", node_rows));
            changes.push(change(edges, "replace", edge_rows));
        }
        SaveTarget::Form { table } => {
            let d = &opts.tables[table];
            if d.key != ["id"] {
                return Err(format!(
                    "'{table}' must be keyed by id to take a form's row (options.writeback)"
                ));
            }
            let vals = req
                .form
                .as_ref()
                .ok_or("this ::SAVE saves a form: 'form' is missing")?;
            let key = req
                .key
                .as_deref()
                .filter(|k| !k.is_empty())
                .ok_or("pick the row to save first ('key' is empty)")?;
            let mut fields = vals.clone();
            fields.insert("id".into(), Value::String(key.to_string()));
            changes.push(change(
                table,
                "upsert",
                vec![typed_row(table, d, &fields, text)?],
            ));
        }
    }
    let mut out = Map::new();
    out.insert("changes".into(), Value::Array(changes));
    out.insert("base".into(), req.base.map_or(Value::Null, Value::from));
    Ok(out)
}

fn change(table: &str, op: &str, rows: Vec<Value>) -> Value {
    serde_json::json!({"table": table, "op": op, "rows": rows})
}

/// One row: every field declared and coerced, every key column non-null.
fn typed_row(
    table: &str,
    decl: &TableDecl,
    fields: &Map<String, Value>,
    text_max: usize,
) -> Result<Value, String> {
    let mut row = Map::new();
    for (f, v) in fields {
        let ty = decl
            .col(f)
            .ok_or_else(|| format!("'{f}' is not a column of '{table}' (options.writeback)"))?;
        row.insert(
            f.clone(),
            coerce(ty, v, text_max).map_err(|e| format!("{table}.{f}: {e}"))?,
        );
    }
    for k in &decl.key {
        if row.get(k).is_none_or(Value::is_null) {
            return Err(format!("{table}.{k}: the key must have a value"));
        }
    }
    Ok(Value::Object(row))
}

fn unique_keys(table: &str, decl: &TableDecl, rows: &[Value]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for r in rows {
        let k: Vec<String> = decl.key.iter().map(|c| r[c].to_string()).collect();
        if !seen.insert(k.clone()) {
            return Err(format!("{table}: key {} appears twice", k.join(", ")));
        }
    }
    Ok(())
}

/// A JSON value as `ty`, or why not. `null` stays `null`.
fn coerce(ty: ColType, v: &Value, text_max: usize) -> Result<Value, String> {
    if v.is_null() {
        return Ok(Value::Null);
    }
    let as_text = || match v {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    };
    match ty {
        ColType::Varchar => {
            let s = match v {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => return Err("expected text".into()),
            };
            if s.chars().count() > text_max {
                return Err(format!("longer than {text_max} characters"));
            }
            Ok(Value::String(s))
        }
        ColType::Integer | ColType::Bigint => {
            let n = match v {
                Value::Number(n) => n.as_i64().or_else(|| {
                    n.as_f64()
                        .filter(|f| f.fract() == 0.0 && f.abs() < 9.0e15)
                        .map(|f| f as i64)
                }),
                _ => as_text().and_then(|s| s.parse::<i64>().ok()),
            }
            .ok_or("expected a whole number")?;
            if ty == ColType::Integer && i32::try_from(n).is_err() {
                return Err("out of INTEGER range".into());
            }
            Ok(Value::from(n))
        }
        ColType::Double => {
            let f = match v {
                Value::Number(n) => n.as_f64(),
                _ => as_text().and_then(|s| s.parse::<f64>().ok()),
            }
            .filter(|f| f.is_finite())
            .ok_or("expected a number")?;
            Ok(Value::from(f))
        }
        ColType::Boolean => match v {
            Value::Bool(b) => Ok(Value::Bool(*b)),
            Value::String(s) if s.eq_ignore_ascii_case("true") => Ok(Value::Bool(true)),
            Value::String(s) if s.eq_ignore_ascii_case("false") => Ok(Value::Bool(false)),
            _ => Err("expected true or false".into()),
        },
        ColType::Date => {
            let s = v
                .as_str()
                .map(str::trim)
                .ok_or("expected a date (YYYY-MM-DD)")?;
            if valid_date(s) {
                Ok(Value::String(s.to_string()))
            } else {
                Err("expected a date (YYYY-MM-DD)".into())
            }
        }
    }
}

fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().ok();
    let (Some(y), Some(m), Some(d)) = (num(0..4), num(5..7), num(8..10)) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d) && s[..4].bytes().all(|c| c.is_ascii_digit())
}

// ─────────────────────────────────────────────────────────── materialise ──

/// One declared table as the change log leaves it (#684 slice 3).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableNow {
    /// A graph save replaced the table: `rows` is all of it, the base is
    /// not read. Otherwise `rows` are merged over the base by key.
    pub replaced: bool,
    /// Replaced: every row. Merged: one row per key, only the fields saved
    /// (later saves win per field).
    pub rows: Vec<Map<String, Value>>,
}

/// The change log folded into each declared table's current state, oldest
/// save first. A change naming a table the config no longer declares is
/// skipped (the config is the authority, not old records).
pub fn fold(
    opts: &WritebackOptions,
    records: &[std::sync::Arc<crate::host::state::Record>],
) -> BTreeMap<String, TableNow> {
    let mut out: BTreeMap<String, TableNow> = BTreeMap::new();
    for rec in records {
        let Some(changes) = rec.vars.get("changes").and_then(Value::as_array) else {
            continue;
        };
        for ch in changes {
            let (Some(t), Some(op), Some(rows)) = (
                ch.get("table").and_then(Value::as_str),
                ch.get("op").and_then(Value::as_str),
                ch.get("rows").and_then(Value::as_array),
            ) else {
                continue;
            };
            let Some(decl) = opts.tables.get(t) else {
                continue;
            };
            let rows = rows.iter().filter_map(|r| r.as_object().cloned());
            let now = out.entry(t.to_string()).or_default();
            match op {
                "replace" => {
                    now.replaced = true;
                    now.rows = rows.collect();
                }
                "upsert" => {
                    for row in rows {
                        let k = row_key(decl, &row);
                        match now.rows.iter_mut().find(|r| row_key(decl, r) == k) {
                            Some(have) => have.extend(row),
                            None => now.rows.push(row),
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn row_key(decl: &TableDecl, row: &Map<String, Value>) -> Vec<String> {
    decl.key
        .iter()
        .map(|k| row.get(k).map_or_else(|| "null".into(), Value::to_string))
        .collect()
}

/// Does `sql` name `table` as a word (any case)? Decides which dashboards
/// get the saved tables — a false positive only costs a materialisation.
pub fn references(sql: &str, table: &str) -> bool {
    let (hay, needle) = (sql.to_ascii_lowercase(), table.to_ascii_lowercase());
    let word = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    hay.match_indices(&needle).any(|(i, _)| {
        !word(hay[..i].chars().next_back()) && !word(hay[i + needle.len()..].chars().next())
    })
}

/// The staging table [`materialise_statements`] reads the saved rows from:
/// `(t VARCHAR, row VARCHAR)`, filled through an Appender (never SQL text).
pub const STAGE: &str = "__anofox_writeback";

/// The statements that build `table`'s TEMP table on a fresh connection,
/// after [`STAGE`] holds its rows: from the saved rows alone (replaced),
/// or the base table (`base_cols`: its columns, `None` when there is no
/// base) cast to the declared types with the saved rows merged by key.
/// Names are validated identifiers, types come from [`ColType`]; no value
/// appears in the SQL.
pub fn materialise_statements(
    table: &str,
    decl: &TableDecl,
    now: &TableNow,
    base_cols: Option<&[String]>,
) -> Vec<String> {
    let q = |c: &str| format!("\"{c}\"");
    let from_row = |c: &str, ty: ColType| {
        format!(
            "CAST(json_extract_string(r.\"row\", '$.{c}') AS {})",
            ty.sql()
        )
    };
    let staged = format!(
        "(SELECT CAST(\"row\" AS JSON) AS \"row\" FROM temp.main.{STAGE} WHERE t = '{table}') r"
    );
    let all = decl
        .columns
        .iter()
        .map(|(c, _)| q(c))
        .collect::<Vec<_>>()
        .join(", ");
    let typed = decl
        .columns
        .iter()
        .map(|(c, ty)| format!("{} AS {}", from_row(c, *ty), q(c)))
        .collect::<Vec<_>>()
        .join(", ");
    if now.replaced {
        return vec![format!(
            "CREATE TEMP TABLE {} AS SELECT {typed} FROM {staged}",
            q(table)
        )];
    }
    let mut out = vec![match base_cols {
        // Bound before the TEMP table exists, so the inner name is the base.
        Some(have) => {
            let cols = decl
                .columns
                .iter()
                .map(|(c, ty)| {
                    if have.iter().any(|h| h.eq_ignore_ascii_case(c)) {
                        format!("CAST({} AS {}) AS {}", q(c), ty.sql(), q(c))
                    } else {
                        format!("CAST(NULL AS {}) AS {}", ty.sql(), q(c))
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "CREATE TEMP TABLE {} AS SELECT {cols} FROM {}",
                q(table),
                q(table)
            )
        }
        None => {
            let cols = decl
                .columns
                .iter()
                .map(|(c, ty)| format!("{} {}", q(c), ty.sql()))
                .collect::<Vec<_>>()
                .join(", ");
            format!("CREATE TEMP TABLE {} ({cols})", q(table))
        }
    }];
    if now.rows.is_empty() {
        return out;
    }
    let dest = format!("temp.main.{}", q(table));
    let key_match = decl
        .key
        .iter()
        .map(|k| {
            let ty = decl.col(k).expect("a key is a declared column");
            format!("d.{} = {}", q(k), from_row(k, ty))
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    let sets = decl
        .columns
        .iter()
        .filter(|(c, _)| !decl.key.contains(c))
        .map(|(c, ty)| {
            format!(
                "{} = CASE WHEN json_exists(r.\"row\", '$.{c}') THEN {} ELSE d.{} END",
                q(c),
                from_row(c, *ty),
                q(c)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    if !sets.is_empty() {
        out.push(format!(
            "UPDATE {dest} AS d SET {sets} FROM {staged} WHERE {key_match}"
        ));
    }
    out.push(format!(
        "INSERT INTO {dest} ({all}) SELECT {typed} FROM {staged} \
         WHERE NOT EXISTS (SELECT 1 FROM {dest} d WHERE {key_match})"
    ));
    out
}

/// Base columns a save would hide: every column of the base table must be
/// declared, or a panel reading the saved table would lose it.
pub fn undeclared_base_columns(decl: &TableDecl, base_cols: &[String]) -> Vec<String> {
    base_cols
        .iter()
        .filter(|b| !decl.columns.iter().any(|(c, _)| c.eq_ignore_ascii_case(b)))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn opts() -> WritebackOptions {
        WritebackOptions::parse(&json!({
            "path": "/srv/wb/changes.jsonl",
            "dashboards": ["twin"],
            "writers": {"groups": ["planners"]},
            "tables": {
                "plan_nodes": {"key": ["id"], "columns": {"id": "VARCHAR", "type": "VARCHAR", "label": "VARCHAR", "stage": "INTEGER"}},
                "plan_edges": {"key": ["from", "to"], "columns": {"from": "VARCHAR", "to": "VARCHAR"}},
                "plan_params": {"key": ["id"], "columns": {"id": "VARCHAR", "lt": "DOUBLE", "active": "BOOLEAN", "start": "DATE"}}
            }
        }))
        .unwrap()
    }

    fn req(extra: Value) -> SaveRequest {
        let mut b = json!({"dashboard": "twin", "panel": 3, "id": "ab12", "vars": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        parse_save_request(&b.to_string()).unwrap()
    }

    #[test]
    fn parse_refuses_what_would_be_unsafe_or_useless() {
        let base = json!({"path": "/a/b", "dashboards": ["d"], "writers": {"users": ["u"]},
                          "tables": {"t": {"key": ["id"], "columns": {"id": "VARCHAR"}}}});
        assert!(WritebackOptions::parse(&base).is_ok());
        let bad = |patch: Value, needle: &str| {
            let mut v = base.clone();
            for (k, x) in patch.as_object().unwrap() {
                if x.is_null() {
                    v.as_object_mut().unwrap().remove(k);
                } else {
                    v[k] = x.clone();
                }
            }
            let e = WritebackOptions::parse(&v).unwrap_err();
            assert!(e.contains(needle), "{e}");
        };
        bad(json!({"path": null}), "path is required");
        bad(json!({"writers": {}}), "needs groups or users");
        bad(json!({"dashboards": []}), "at least one dashboard");
        bad(json!({"tables": {}}), "at least one table");
        bad(
            json!({"tables": {"x y": {"key": ["id"], "columns": {"id": "VARCHAR"}}}}),
            "not a plain table name",
        );
        bad(
            json!({"tables": {"t": {"key": ["id"], "columns": {"id": "BLOB"}}}}),
            "the type must be",
        );
        bad(
            json!({"tables": {"t": {"key": ["nope"], "columns": {"id": "VARCHAR"}}}}),
            "is not one of its columns",
        );
        bad(
            json!({"tables": {"t": {"key": ["id"], "columns": {"id\"": "VARCHAR"}}}}),
            "not a plain column name",
        );
        bad(json!({"sql": "DROP TABLE x"}), "unknown key 'sql'");
    }

    #[test]
    fn writers_by_user_or_group() {
        let o = opts();
        assert!(o.is_writer("alice", &["planners".into()]));
        assert!(!o.is_writer("alice", &["viewers".into()]));
        assert!(!o.is_writer("alice", &[]));
    }

    #[test]
    fn save_request_rejects_unknown_fields_and_bad_ids() {
        assert!(
            parse_save_request(r#"{"dashboard":"twin","panel":1,"id":"ab","sql":"x"}"#)
                .unwrap_err()
                .contains("unknown request field 'sql'")
        );
        assert!(parse_save_request(r#"{"dashboard":"twin","panel":1,"id":"zz"}"#).is_err());
        assert!(
            parse_save_request(r#"{"dashboard":"twin","panel":1,"id":"ab","base":-1}"#).is_err()
        );
    }

    #[test]
    fn targets_name_declared_tables_only() {
        let o = opts();
        assert_eq!(
            save_target(&o, "graph", "", "plan_nodes, plan_edges").unwrap(),
            SaveTarget::Graph {
                nodes: "plan_nodes".into(),
                edges: "plan_edges".into()
            }
        );
        assert!(save_target(&o, "graph", "", "plan_nodes").is_err());
        assert!(save_target(&o, "", "lt", "secrets")
            .unwrap_err()
            .contains("not a table this server"));
        assert!(save_target(&o, "g", "f", "plan_params").is_err());
        assert!(save_target(&o, "", "", "plan_params").is_err());
    }

    #[test]
    fn a_graph_replaces_both_tables_with_typed_rows() {
        let o = opts();
        let t = save_target(&o, "graph", "", "plan_nodes, plan_edges").unwrap();
        let r = req(json!({"base": 7, "graph": {
            "nodes": [{"id": "a", "type": "supplier", "label": "A", "stage": "1", "x": 5},
                      {"id": "b", "type": "plant", "label": "B"}],
            "edges": [{"from": "a", "to": "b"}]}}));
        let c = build_changes(&o, &t, &r).unwrap();
        assert_eq!(c["base"], json!(7));
        assert_eq!(c["changes"][0]["op"], "replace");
        assert_eq!(
            c["changes"][0]["rows"][0],
            json!({"id": "a", "type": "supplier", "label": "A", "stage": 1})
        );
        assert_eq!(c["changes"][0]["rows"][1]["stage"], Value::Null);
        assert_eq!(c["changes"][1]["rows"][0], json!({"from": "a", "to": "b"}));
    }

    #[test]
    fn a_graph_refuses_duplicates_missing_keys_and_too_many_rows() {
        let mut o = opts();
        let t = save_target(&o, "graph", "", "plan_nodes, plan_edges").unwrap();
        let dup = req(json!({"graph": {"nodes": [{"id": "a"}, {"id": "a"}], "edges": []}}));
        assert!(build_changes(&o, &t, &dup)
            .unwrap_err()
            .contains("appears twice"));
        let nokey = req(json!({"graph": {"nodes": [{"label": "A"}], "edges": []}}));
        assert!(build_changes(&o, &t, &nokey)
            .unwrap_err()
            .contains("the key must have a value"));
        o.max_rows = 1;
        let big =
            req(json!({"graph": {"nodes": [{"id": "a"}], "edges": [{"from": "a", "to": "a"}]}}));
        assert!(build_changes(&o, &t, &big)
            .unwrap_err()
            .contains("at most 1 rows"));
        assert!(build_changes(&o, &t, &req(json!({})))
            .unwrap_err()
            .contains("'graph' is missing"));
    }

    #[test]
    fn a_form_upserts_one_typed_row_keyed_by_id() {
        let o = opts();
        let t = save_target(&o, "", "lt", "plan_params").unwrap();
        let r = req(
            json!({"key": "sup_a", "form": {"lt": "4.5", "active": true, "start": "2026-02-28"}}),
        );
        let c = build_changes(&o, &t, &r).unwrap();
        assert_eq!(c["changes"][0]["op"], "upsert");
        assert_eq!(
            c["changes"][0]["rows"][0],
            json!({"id": "sup_a", "lt": 4.5, "active": true, "start": "2026-02-28"})
        );
        let undeclared = req(json!({"key": "a", "form": {"cost": 1}}));
        assert!(build_changes(&o, &t, &undeclared)
            .unwrap_err()
            .contains("'cost' is not a column"));
        let nokey = req(json!({"form": {"lt": 1}}));
        assert!(build_changes(&o, &t, &nokey)
            .unwrap_err()
            .contains("pick the row"));
        let bad_date = req(json!({"key": "a", "form": {"start": "2026-02-30"}}));
        assert!(build_changes(&o, &t, &bad_date).is_err());
        let bad_num = req(json!({"key": "a", "form": {"lt": "NaN"}}));
        assert!(build_changes(&o, &t, &bad_num).is_err());
    }

    #[test]
    fn coercion_caps_text_and_integer_range() {
        assert!(coerce(ColType::Varchar, &json!("abcd"), 3).is_err());
        assert_eq!(
            coerce(ColType::Varchar, &json!(12), 3).unwrap(),
            json!("12")
        );
        assert!(coerce(ColType::Integer, &json!(3_000_000_000i64), 10).is_err());
        assert_eq!(
            coerce(ColType::Bigint, &json!(3_000_000_000i64), 10).unwrap(),
            json!(3_000_000_000i64)
        );
        assert_eq!(coerce(ColType::Integer, &json!(2.0), 10).unwrap(), json!(2));
        assert!(coerce(ColType::Integer, &json!(2.5), 10).is_err());
        assert!(coerce(ColType::Boolean, &json!(1), 10).is_err());
        assert!(valid_date("2024-02-29") && !valid_date("2023-02-29") && !valid_date("2024-13-01"));
    }

    fn rec(changes: Value) -> std::sync::Arc<crate::host::state::Record> {
        std::sync::Arc::new(crate::host::state::Record {
            v: 1,
            seq: 1,
            ts: String::new(),
            ts_micros: None,
            user: "u".into(),
            auth: String::new(),
            dashboard: "twin".into(),
            submit: "s".into(),
            id: "a".into(),
            vars: json!({"changes": changes, "base": null})
                .as_object()
                .unwrap()
                .clone(),
        })
    }

    #[test]
    fn fold_replaces_then_merges_per_key_and_field() {
        let o = opts();
        let log = [
            rec(
                json!([{"table": "plan_params", "op": "upsert", "rows": [{"id": "a", "lt": 1.0}]}]),
            ),
            rec(
                json!([{"table": "plan_params", "op": "upsert", "rows": [{"id": "a", "active": true}]}]),
            ),
            rec(
                json!([{"table": "plan_params", "op": "upsert", "rows": [{"id": "b", "lt": 2.0}]}]),
            ),
            rec(
                json!([{"table": "plan_nodes", "op": "replace", "rows": [{"id": "x"}]},
                       {"table": "gone", "op": "replace", "rows": [{"id": "y"}]}]),
            ),
            rec(
                json!([{"table": "plan_nodes", "op": "upsert", "rows": [{"id": "x", "label": "X"}, {"id": "z"}]}]),
            ),
        ];
        let f = fold(&o, &log);
        let p = &f["plan_params"];
        assert!(!p.replaced);
        assert_eq!(
            Value::from(p.rows.clone()),
            json!([{"id": "a", "lt": 1.0, "active": true}, {"id": "b", "lt": 2.0}])
        );
        let n = &f["plan_nodes"];
        assert!(n.replaced);
        assert_eq!(
            Value::from(n.rows.clone()),
            json!([{"id": "x", "label": "X"}, {"id": "z"}])
        );
        assert!(
            !f.contains_key("gone"),
            "a table no longer declared is skipped"
        );
    }

    #[test]
    fn references_matches_whole_words_only() {
        assert!(references("SELECT * FROM Plan_Nodes WHERE x", "plan_nodes"));
        assert!(references("FROM \"plan_nodes\";", "plan_nodes"));
        assert!(!references("SELECT * FROM plan_nodes_old", "plan_nodes"));
        assert!(!references("SELECT my_plan_nodes", "plan_nodes"));
    }

    #[test]
    fn materialise_sql_casts_to_declared_types_and_never_carries_values() {
        let o = opts();
        let d = &o.tables["plan_params"];
        let merged = TableNow {
            replaced: false,
            rows: vec![json!({"id": "a'; DROP", "lt": 1})
                .as_object()
                .unwrap()
                .clone()],
        };
        let s =
            materialise_statements("plan_params", d, &merged, Some(&["id".into(), "lt".into()]));
        assert_eq!(s.len(), 3);
        assert!(s[0].starts_with("CREATE TEMP TABLE \"plan_params\" AS SELECT CAST(\"id\" AS VARCHAR) AS \"id\", CAST(\"lt\" AS DOUBLE) AS \"lt\", CAST(NULL AS BOOLEAN) AS \"active\""), "{}", s[0]);
        assert!(
            s[1].starts_with(
                "UPDATE temp.main.\"plan_params\" AS d SET \"lt\" = CASE WHEN json_exists"
            ),
            "{}",
            s[1]
        );
        assert!(s[2].contains("WHERE NOT EXISTS"), "{}", s[2]);
        assert!(
            s.iter().all(|x| !x.contains("DROP")),
            "values never reach the SQL"
        );
        let none = materialise_statements("plan_params", d, &TableNow::default(), None);
        assert_eq!(none, vec!["CREATE TEMP TABLE \"plan_params\" (\"id\" VARCHAR, \"lt\" DOUBLE, \"active\" BOOLEAN, \"start\" DATE)".to_string()]);
        let rep = materialise_statements(
            "plan_params",
            d,
            &TableNow {
                replaced: true,
                rows: vec![],
            },
            Some(&[]),
        );
        assert_eq!(rep.len(), 1);
        assert!(rep[0].contains("FROM (SELECT CAST(\"row\" AS JSON) AS \"row\" FROM temp.main.__anofox_writeback WHERE t = 'plan_params') r"), "{}", rep[0]);
        assert_eq!(
            undeclared_base_columns(d, &["ID".into(), "cost".into()]),
            vec!["cost".to_string()]
        );
    }
}
