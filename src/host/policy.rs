//! Access control phase 2 (issue #49): row and column policies enforced by the
//! locked dashboard server. DuckDB has no users, GRANT or row-level security,
//! so the server rewrites the dashboards' SQL itself.
//!
//! * A [`Policy`] (a JSON file next to the dashboards) names every table a
//!   viewer may read, with a row filter (a SQL condition that reads the
//!   viewer's claims only through `getvariable('__claim_…')`) and column rules
//!   (`deny`, `null`, `mask` or `hash`, unless the viewer is in an allowed
//!   group or is an allowed user).
//! * [`Enforcer::compile`] checks the policy against a snapshot.
//!   [`Enforcer::rewrite_script`] then rewrites every statement of a dashboard
//!   once: DuckDB parses it (`json_serialize_sql`), every table reference is
//!   replaced by a per-request view `temp.main.__anofox_rls_<i>`, and DuckDB
//!   prints the result (`json_deserialize_sql`). The output is parsed again
//!   and verified: only those views, the dashboard's own temp objects and
//!   renamed CTEs may remain.
//! * Per request, [`Enforcer::view_statements`] creates the views for the
//!   viewer: `SELECT * EXCLUDE (…) REPLACE (…) FROM (SELECT * FROM <snapshot
//!   table> WHERE <rows>)`. The column decision uses the viewer's claims; the
//!   row filter reads them from the session variables at query time. A claim
//!   is never spliced into SQL text.
//!
//! **Fail closed.** A statement is refused, and the server does not start, if
//! it uses a table without a policy, a name qualified with a database, a table
//! function outside a short allowlist (`query()`, `query_table()`, the
//! `duckdb_*()` catalog functions, file readers unless the policy allows
//! them), a macro whose body reads tables, `SHOW`/`DESCRIBE` of a bare name,
//! time travel (`AT (…)`), or a statement other than a query, `CREATE TEMP
//! TABLE|VIEW … AS <query>` or `SET VARIABLE`. CTE names (which DuckDB parses
//! as table references) are renamed to `__anofox_cte_<n>`, so a reference
//! that this module takes for a CTE can only bind to that CTE: if DuckDB
//! disagrees the statement fails, it never reaches a table.
//!
//! Pure apart from the [`Duck`] callback that runs DuckDB's parser and catalog
//! queries, so the extension, the `dashboard --check` lint and the tests share
//! it.

use crate::host::serving::{quote_ident, random_token, sql_string_literal, valid_ident};
use crate::sql::lex::{self, TokKind};
use serde_json::{Map, Value};
use std::collections::{BTreeSet, HashSet};

pub use crate::host::access::CLAIM_PREFIX;
/// Names starting with this are reserved for the server (views, CTEs).
pub const RESERVED_PREFIX: &str = "__anofox";
/// The per-request view of policy table `i` is `temp.main.__anofox_rls_<i>`.
pub const VIEW_PREFIX: &str = "__anofox_rls_";
/// A CTE is renamed to `__anofox_cte_<n>`.
pub const CTE_PREFIX: &str = "__anofox_cte_";

/// Table functions a dashboard may always call: generators over values.
const SAFE_TABLE_FUNCTIONS: [&str; 3] = ["range", "generate_series", "unnest"];
/// File readers a policy may allow (`"table_functions"`). Files have no row
/// or column policy; DuckDB still confines them to the server's data folder.
const FILE_READERS: [&str; 13] = [
    "read_csv",
    "read_csv_auto",
    "read_parquet",
    "parquet_scan",
    "read_json",
    "read_json_auto",
    "read_ndjson",
    "read_ndjson_auto",
    "read_json_objects",
    "read_text",
    "read_blob",
    "read_xlsx",
    "glob",
];

/// What a viewer outside a column's allow list gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mask {
    /// The column is not in the view: a panel that names it fails.
    Deny,
    /// NULL (of the column's type).
    Null,
    /// `'****'` (NULL stays NULL).
    Mask,
    /// A keyed hash (16 hex digits): equal values stay equal, so grouping and
    /// counting distinct still work, but the value can't be looked up.
    Hash,
}

impl Mask {
    fn parse(s: &str) -> Option<Mask> {
        match s {
            "deny" => Some(Mask::Deny),
            "null" => Some(Mask::Null),
            "mask" => Some(Mask::Mask),
            "hash" => Some(Mask::Hash),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mask::Deny => "deny",
            Mask::Null => "null",
            Mask::Mask => "mask",
            Mask::Hash => "hash",
        }
    }
}

/// A column rule: viewers in `groups` or `users` see the value, everyone else
/// gets `otherwise`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnRule {
    pub column: String,
    pub groups: Vec<String>,
    pub users: Vec<String>,
    pub otherwise: Mask,
}

impl ColumnRule {
    pub fn allows(&self, v: &Viewer) -> bool {
        v.user.as_ref().is_some_and(|u| self.users.contains(u))
            || v.groups.iter().any(|g| self.groups.contains(g))
    }

    /// Who sees the value, for messages ("groups hr" / "nobody").
    pub fn audience(&self) -> String {
        let mut who = Vec::new();
        if !self.groups.is_empty() {
            who.push(format!("groups {}", self.groups.join(", ")));
        }
        if !self.users.is_empty() {
            who.push(format!("users {}", self.users.join(", ")));
        }
        if who.is_empty() {
            "nobody".into()
        } else {
            who.join("; ")
        }
    }
}

/// The policy of one table (or view) of the snapshot's default database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TablePolicy {
    pub schema: String,
    pub table: String,
    /// A SQL condition over the table's columns; `None` = every row.
    pub rows: Option<String>,
    pub columns: Vec<ColumnRule>,
}

impl TablePolicy {
    /// `table`, or `schema.table` outside `main` (for messages and keys).
    pub fn name(&self) -> String {
        if self.schema.eq_ignore_ascii_case("main") {
            self.table.clone()
        } else {
            format!("{}.{}", self.schema, self.table)
        }
    }

    pub fn rule(&self, column: &str) -> Option<&ColumnRule> {
        self.columns
            .iter()
            .find(|c| c.column.eq_ignore_ascii_case(column))
    }
}

/// A parsed policy file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub tables: Vec<TablePolicy>,
    /// Extra table functions allowed (file readers only).
    pub table_functions: Vec<String>,
    /// Key of the `hash` mask: random per parse (so per server, stable
    /// across snapshot refreshes), never from the file.
    pub hash_key: String,
}

/// The viewer a request runs for (the claims of issue #48). Without a login
/// every viewer is anonymous: no user, no groups.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Viewer {
    pub user: Option<String>,
    pub groups: Vec<String>,
}

fn str_list(v: &Value, what: &str) -> Result<Vec<String>, String> {
    v.as_array()
        .ok_or_else(|| format!("{what} must be a list of strings"))?
        .iter()
        .map(|x| {
            x.as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("{what} must be a list of non-empty strings"))
        })
        .collect()
}

impl Policy {
    /// Parse a policy file:
    ///
    /// ```json
    /// {"tables": {
    ///    "sales": {"rows": "region IN (SELECT unnest(getvariable('__claim_regions')))",
    ///              "columns": {"margin": {"allow_groups": ["finance"], "else": "null"},
    ///                          "email": "hash"}},
    ///    "regions": {"rows": true}},
    ///  "table_functions": ["read_parquet"]}
    /// ```
    ///
    /// Unknown keys are errors, so a typo never silently drops a rule.
    pub fn parse(json: &str) -> Result<Policy, String> {
        let v: Value = serde_json::from_str(json).map_err(|e| format!("policy: bad JSON: {e}"))?;
        let o = v.as_object().ok_or("policy must be a JSON object")?;
        let mut tables = Vec::new();
        let mut table_functions = Vec::new();
        for (k, val) in o {
            match k.as_str() {
                "tables" => {
                    let t = val
                        .as_object()
                        .ok_or("policy.tables must map table names to rules")?;
                    for (name, rule) in t {
                        tables.push(parse_table(name, rule)?);
                    }
                }
                "table_functions" => {
                    for f in str_list(val, "policy.table_functions")? {
                        let f = f.to_ascii_lowercase();
                        if !FILE_READERS.contains(&f.as_str()) {
                            return Err(format!(
                                "policy.table_functions: '{f}' can't be allowed (only file readers: {})",
                                FILE_READERS.join(", ")
                            ));
                        }
                        table_functions.push(f);
                    }
                }
                other => {
                    return Err(format!(
                        "policy: unknown key '{other}' (tables, table_functions)"
                    ))
                }
            }
        }
        let mut seen = HashSet::new();
        for t in &tables {
            let key = (t.schema.to_ascii_lowercase(), t.table.to_ascii_lowercase());
            if !seen.insert(key) {
                return Err(format!("policy: table '{}' is listed twice", t.name()));
            }
        }
        Ok(Policy {
            tables,
            table_functions,
            hash_key: random_token(),
        })
    }

    /// The policy of `schema.table` (`schema` empty = `main`), any case.
    pub fn find(&self, schema: &str, table: &str) -> Option<usize> {
        let schema = if schema.is_empty() { "main" } else { schema };
        self.tables.iter().position(|t| {
            t.schema.eq_ignore_ascii_case(schema) && t.table.eq_ignore_ascii_case(table)
        })
    }
}

fn parse_table(name: &str, rule: &Value) -> Result<TablePolicy, String> {
    let (schema, table) = match name.split_once('.') {
        Some((s, t)) => (s, t),
        None => ("main", name),
    };
    if !valid_ident(schema) || !valid_ident(table) {
        return Err(format!(
            "policy.tables: '{name}' must be a table name or schema.table ([A-Za-z_][A-Za-z0-9_]*)"
        ));
    }
    if table.to_ascii_lowercase().starts_with(RESERVED_PREFIX) {
        return Err(format!("policy.tables: '{name}' is a reserved name"));
    }
    let o = rule
        .as_object()
        .ok_or_else(|| format!("policy.tables.{name} must be an object"))?;
    let mut rows = None;
    let mut columns = Vec::new();
    let mut have_rows = false;
    for (k, v) in o {
        match k.as_str() {
            "rows" => {
                have_rows = true;
                rows = match v {
                    Value::Bool(true) => None,
                    Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
                    _ => {
                        return Err(format!(
                            "policy.tables.{name}.rows must be a SQL condition or true (every row)"
                        ))
                    }
                }
            }
            "columns" => {
                let c = v.as_object().ok_or_else(|| {
                    format!("policy.tables.{name}.columns must map column names to rules")
                })?;
                for (col, r) in c {
                    columns.push(parse_column(name, col, r)?);
                }
            }
            other => {
                return Err(format!(
                    "policy.tables.{name}: unknown key '{other}' (rows, columns)"
                ))
            }
        }
    }
    // Explicit, so "I forgot the row filter" can't look like "every row".
    if !have_rows {
        return Err(format!(
            "policy.tables.{name}.rows is required (a SQL condition, or true for every row)"
        ));
    }
    Ok(TablePolicy {
        schema: schema.to_string(),
        table: table.to_string(),
        rows,
        columns,
    })
}

fn parse_column(table: &str, col: &str, r: &Value) -> Result<ColumnRule, String> {
    let at = format!("policy.tables.{table}.columns.{col}");
    if col.is_empty() || col.len() > 128 || col.chars().any(char::is_control) {
        return Err(format!("{at}: invalid column name"));
    }
    let bad_mask = || format!("{at}: must be deny, null, mask or hash");
    let mut rule = ColumnRule {
        column: col.to_string(),
        groups: Vec::new(),
        users: Vec::new(),
        otherwise: Mask::Null,
    };
    match r {
        Value::String(s) => rule.otherwise = Mask::parse(s).ok_or_else(bad_mask)?,
        Value::Object(o) => {
            for (k, v) in o {
                match k.as_str() {
                    "allow_groups" => rule.groups = str_list(v, &format!("{at}.allow_groups"))?,
                    "allow_users" => rule.users = str_list(v, &format!("{at}.allow_users"))?,
                    "else" => {
                        rule.otherwise = v.as_str().and_then(Mask::parse).ok_or_else(bad_mask)?
                    }
                    other => {
                        return Err(format!(
                            "{at}: unknown key '{other}' (allow_groups, allow_users, else)"
                        ))
                    }
                }
            }
        }
        _ => return Err(bad_mask()),
    }
    Ok(rule)
}

/// Runs DuckDB for the policy engine: `sql` is one statement whose columns
/// are all VARCHAR; the rows come back as text.
pub trait Duck {
    fn rows(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>, String>;
}

impl<F: FnMut(&str) -> Result<Vec<Vec<Option<String>>>, String>> Duck for F {
    fn rows(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
        self(sql)
    }
}

fn first_value(duck: &mut dyn Duck, sql: &str) -> Result<String, String> {
    duck.rows(sql)?
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next().flatten())
        .ok_or_else(|| "no result".to_string())
}

/// Parse one SELECT statement with DuckDB. Anything else (several statements,
/// DDL, `PIVOT …` statements, syntax errors) is an error.
fn serialize(duck: &mut dyn Duck, sql: &str) -> Result<Value, String> {
    let q = format!(
        "SELECT json_serialize_sql({})::VARCHAR",
        sql_string_literal(sql)
    );
    let text = first_value(duck, &q)?;
    let v: Value =
        serde_json::from_str(&text).map_err(|e| format!("unreadable parse tree: {e}"))?;
    if v.get("error").and_then(Value::as_bool) != Some(false) {
        let msg = v
            .get("error_message")
            .and_then(Value::as_str)
            .unwrap_or("parse error");
        return Err(if msg.contains("Only SELECT") {
            "only queries can be checked against the policy (PIVOT statements, too, are not supported: aggregate with FILTER or CASE instead)".into()
        } else {
            msg.to_string()
        });
    }
    match v.get("statements").and_then(Value::as_array) {
        Some(s) if s.len() == 1 => Ok(v),
        _ => Err("expected exactly one statement".into()),
    }
}

fn deserialize(duck: &mut dyn Duck, ast: &Value) -> Result<String, String> {
    let q = format!(
        "SELECT json_deserialize_sql({}::JSON)::VARCHAR",
        sql_string_literal(&ast.to_string())
    );
    first_value(duck, &q)
}

/// How a walk treats table references.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A dashboard statement: tables → per-request views.
    Rewrite,
    /// The output of `Rewrite`, parsed again: only views, temp objects of the
    /// dashboard and renamed CTEs may be referenced.
    Verify,
    /// A policy's row filter: tables → the snapshot's own tables; it may only
    /// read claim variables.
    Rows,
}

struct Walker<'a> {
    mode: Mode,
    enf: &'a Enforcer,
    /// Temp tables / views the dashboard's setup creates (lower case).
    temps: &'a HashSet<String>,
    /// Visible CTEs, innermost last: (lower-case name, new name).
    scopes: Vec<Vec<(String, String)>>,
    next_cte: usize,
    /// Policy tables used.
    used: BTreeSet<usize>,
    /// Column names referenced (lower case, last part) — for the lint.
    columns: BTreeSet<String>,
}

fn s<'v>(o: &'v Map<String, Value>, k: &str) -> &'v str {
    o.get(k).and_then(Value::as_str).unwrap_or("")
}

impl Walker<'_> {
    fn new<'a>(mode: Mode, enf: &'a Enforcer, temps: &'a HashSet<String>) -> Walker<'a> {
        Walker {
            mode,
            enf,
            temps,
            scopes: Vec::new(),
            next_cte: 0,
            used: BTreeSet::new(),
            columns: BTreeSet::new(),
        }
    }

    fn walk(&mut self, v: &mut Value) -> Result<(), String> {
        match v {
            Value::Array(a) => a.iter_mut().try_for_each(|x| self.walk(x)),
            Value::Object(o) => self.object(o),
            _ => Ok(()),
        }
    }

    fn object(&mut self, o: &mut Map<String, Value>) -> Result<(), String> {
        let ty = s(o, "type").to_string();
        if let Some(f) = o.get("function_name").and_then(Value::as_str) {
            self.function(f, o)?;
        }
        if ty == "COLUMN_REF" {
            if let Some(Value::String(c)) = o
                .get("column_names")
                .and_then(Value::as_array)
                .and_then(|a| a.last())
            {
                self.columns.insert(c.to_ascii_lowercase());
            }
        }
        // A table reference: BASE_TABLE, JOIN, SUBQUERY, … (expressions have
        // a "class"; query nodes have no "alias"). Recognised by its type as
        // well as its shape, so a serializer that leaves out default-valued
        // keys can't hide one.
        let is_ref = !o.contains_key("class")
            && (o.contains_key("table_name")
                || (o.contains_key("alias") && o.contains_key("sample"))
                || matches!(ty.as_str(), "BASE_TABLE" | "TABLE_FUNCTION" | "SHOW_REF"));
        if o.contains_key("table_name") && !matches!(ty.as_str(), "BASE_TABLE" | "SHOW_REF") {
            return Err(format!("unsupported table reference ({ty})"));
        }
        if is_ref {
            match ty.as_str() {
                "BASE_TABLE" => return self.table(o),
                "TABLE_FUNCTION" => self.table_function(o)?,
                "SHOW_REF" => {
                    if o.get("query").is_none_or(Value::is_null) {
                        return Err(
                            "SHOW / DESCRIBE / SUMMARIZE of a bare name is not allowed (use DESCRIBE SELECT * FROM t)"
                                .into(),
                        );
                    }
                }
                "JOIN" | "SUBQUERY" | "EXPRESSION_LIST" | "EMPTY" | "PIVOT" => {}
                other => return Err(format!("unsupported table reference ({other})")),
            }
        }
        if (o.contains_key("cte_map") || o.contains_key("modifiers") || ty.ends_with("_NODE"))
            && !matches!(
                ty.as_str(),
                "SELECT_NODE" | "SET_OPERATION_NODE" | "RECURSIVE_CTE_NODE"
            )
        {
            return Err(format!("unsupported query ({ty})"));
        }
        // CTEs: visible in the whole node (its other CTEs, and in its own body
        // when it is recursive). Generous on purpose: a renamed reference can
        // only ever bind to that CTE.
        let mut pushed = false;
        if let Some(cte) = o.get_mut("cte_map") {
            let mut cte = cte.take();
            let r = self.ctes(&mut cte);
            o.insert("cte_map".into(), cte);
            pushed = r?;
        }
        if ty == "RECURSIVE_CTE_NODE" {
            let name = s(o, "cte_name").to_string();
            let new = self
                .cte(&name)
                .ok_or_else(|| format!("recursive CTE '{name}' outside its WITH"))?;
            o.insert("cte_name".into(), Value::String(new));
        }
        let mut r = Ok(());
        for (k, x) in o.iter_mut() {
            if k != "cte_map" {
                r = self.walk(x);
                if r.is_err() {
                    break;
                }
            }
        }
        if pushed {
            self.scopes.pop();
        }
        r
    }

    /// Enter a `cte_map`: rename its CTEs and walk their bodies. Returns
    /// whether a scope was pushed.
    fn ctes(&mut self, cte: &mut Value) -> Result<bool, String> {
        let Some(map) = cte.get_mut("map").and_then(Value::as_array_mut) else {
            return Ok(false);
        };
        if map.is_empty() {
            return Ok(false);
        }
        let mut frame = Vec::new();
        for e in map.iter() {
            let key = e.get("key").and_then(Value::as_str).unwrap_or("");
            let new = match self.mode {
                Mode::Verify => {
                    if !key.starts_with(CTE_PREFIX) {
                        return Err(format!("CTE '{key}' was not renamed"));
                    }
                    key.to_string()
                }
                _ => {
                    self.next_cte += 1;
                    format!("{CTE_PREFIX}{}", self.next_cte)
                }
            };
            frame.push((key.to_ascii_lowercase(), new));
        }
        for (i, e) in map.iter_mut().enumerate() {
            if let Some(o) = e.as_object_mut() {
                o.insert("key".into(), Value::String(frame[i].1.clone()));
            }
            // In its own body a CTE is itself only when it is recursive;
            // otherwise its name is the table (`WITH sales AS (FROM sales)`).
            let recursive = e.pointer("/value/query/node/type").and_then(Value::as_str)
                == Some("RECURSIVE_CTE_NODE");
            let mut visible = frame.clone();
            if !recursive {
                visible.remove(i);
            }
            self.scopes.push(visible);
            let r = e.get_mut("value").map_or(Ok(()), |body| self.walk(body));
            self.scopes.pop();
            r?;
        }
        self.scopes.push(frame);
        Ok(true)
    }

    fn cte(&self, name: &str) -> Option<String> {
        let lower = name.to_ascii_lowercase();
        self.scopes
            .iter()
            .rev()
            .flat_map(|f| f.iter())
            .find(|(n, _)| *n == lower)
            .map(|(_, new)| new.clone())
    }

    fn table(&mut self, o: &mut Map<String, Value>) -> Result<(), String> {
        let (catalog, schema, name) = (
            s(o, "catalog_name").to_string(),
            s(o, "schema_name").to_string(),
            s(o, "table_name").to_string(),
        );
        if o.get("at_clause").is_some_and(|a| !a.is_null()) {
            return Err(format!("'{name}': time travel (AT …) is not allowed"));
        }
        let shown = [catalog.as_str(), schema.as_str(), name.as_str()]
            .iter()
            .filter(|p| !p.is_empty())
            .copied()
            .collect::<Vec<_>>()
            .join(".");
        let lower = name.to_ascii_lowercase();
        let set = |o: &mut Map<String, Value>, catalog: &str, schema: &str, table: &str| {
            if s(o, "alias").is_empty() {
                // Keep `name.col` working after the rename.
                o.insert("alias".into(), Value::String(name.clone()));
            }
            o.insert("catalog_name".into(), Value::String(catalog.into()));
            o.insert("schema_name".into(), Value::String(schema.into()));
            o.insert("table_name".into(), Value::String(table.into()));
        };
        match self.mode {
            Mode::Verify => {
                let ok = if catalog.is_empty() && schema.is_empty() {
                    lower.starts_with(CTE_PREFIX) && self.cte(&name).is_some()
                } else {
                    catalog == "temp"
                        && schema == "main"
                        && (self.temps.contains(&lower)
                            || lower
                                .strip_prefix(VIEW_PREFIX)
                                .and_then(|i| i.parse::<usize>().ok())
                                .is_some_and(|i| i < self.enf.policy.tables.len()))
                };
                if !ok {
                    return Err(format!("internal: rewrite left table '{shown}'"));
                }
            }
            Mode::Rows => {
                if catalog.is_empty() && schema.is_empty() {
                    if let Some(new) = self.cte(&name) {
                        set(o, "", "", &new);
                        return Ok(());
                    }
                }
                if catalog.is_empty() {
                    let schema = if schema.is_empty() { "main" } else { &schema };
                    set(o, &self.enf.default_db, schema, &name);
                }
            }
            Mode::Rewrite => {
                if !catalog.is_empty() {
                    return Err(format!(
                        "'{shown}': names qualified with a database are not allowed under the access policy"
                    ));
                }
                if schema.is_empty() {
                    if let Some(new) = self.cte(&name) {
                        set(o, "", "", &new);
                        return Ok(());
                    }
                    if self.temps.contains(&lower) {
                        set(o, "temp", "main", &name);
                        return Ok(());
                    }
                }
                let Some(i) = self.enf.policy.find(&schema, &name) else {
                    return Err(format!("table '{shown}' has no access policy"));
                };
                self.used.insert(i);
                set(o, "temp", "main", &format!("{VIEW_PREFIX}{i}"));
            }
        }
        Ok(())
    }

    fn table_function(&mut self, o: &Map<String, Value>) -> Result<(), String> {
        let f = o.get("function").and_then(Value::as_object);
        let name = f
            .map(|f| s(f, "function_name"))
            .unwrap_or("")
            .to_ascii_lowercase();
        let qualified =
            f.is_some_and(|f| !s(f, "schema").is_empty() || !s(f, "catalog").is_empty());
        let ok = !qualified
            && (SAFE_TABLE_FUNCTIONS.contains(&name.as_str())
                || self.enf.policy.table_functions.contains(&name));
        if ok {
            Ok(())
        } else {
            Err(format!(
                "table function '{name}()' is not allowed under the access policy"
            ))
        }
    }

    fn function(&mut self, name: &str, o: &Map<String, Value>) -> Result<(), String> {
        let lower = name.to_ascii_lowercase();
        if self.enf.denied_functions.contains(&lower) {
            return Err(format!(
                "'{name}()' is a macro: its body would run outside the access policy"
            ));
        }
        if self.mode == Mode::Rows && lower == "getvariable" {
            let arg = o
                .get("children")
                .and_then(Value::as_array)
                .filter(|c| c.len() == 1)
                .and_then(|c| c[0].get("value"))
                .filter(|v| v.pointer("/type/id").and_then(Value::as_str) == Some("VARCHAR"))
                .and_then(|v| v.get("value"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !arg.to_ascii_lowercase().starts_with(CLAIM_PREFIX) {
                return Err(
                    "a row filter may only read claims: getvariable('__claim_…') with a literal name"
                        .into(),
                );
            }
        }
        Ok(())
    }
}

/// A policy checked against one snapshot.
#[derive(Debug, Clone)]
pub struct Enforcer {
    pub policy: Policy,
    default_db: String,
    /// Per policy table: the view body before masking (`SELECT * FROM
    /// "db"."schema"."t" WHERE …`, every name qualified).
    bodies: Vec<String>,
    /// Lower-case names of macros whose body reads tables.
    denied_functions: HashSet<String>,
}

/// One dashboard, rewritten.
#[derive(Debug, Clone, Default)]
pub struct Script {
    /// `(is_setup, sql)` like the input, rewritten.
    pub stmts: Vec<(bool, String)>,
    /// Policy tables any statement uses (their views are created per request).
    pub tables: Vec<usize>,
    /// Per statement: (policy tables, column names referenced) — for the lint.
    pub uses: Vec<(BTreeSet<usize>, BTreeSet<String>)>,
}

/// The dashboard statements a locked server accepts under a policy or with a
/// data folder: queries, `CREATE [OR REPLACE] TEMP TABLE|VIEW <name> AS
/// <query>` and `SET VARIABLE <name> = <expr>`.
enum Setup<'a> {
    Query,
    Create {
        head: String,
        name: String,
        body: &'a str,
    },
    SetVariable {
        name: String,
        expr: &'a str,
    },
}

fn ident_text(src: &str, t: &lex::Token) -> Option<String> {
    match t.kind {
        TokKind::Word => Some(t.text(src).to_string()),
        TokKind::QuotedIdent => {
            let q = t.text(src);
            Some(q[1..q.len().saturating_sub(1)].replace("\"\"", "\""))
        }
        _ => None,
    }
}

fn classify(sql: &str) -> Result<Setup<'_>, String> {
    if crate::sql::is_query_statement(sql)
        || matches!(
            lex::first_keyword(sql).as_deref(),
            Some("DESCRIBE" | "SUMMARIZE" | "SHOW" | "VALUES")
        )
    {
        return Ok(Setup::Query);
    }
    let toks: Vec<lex::Token> = lex::tokenize(sql)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    let word = |i: usize, kw: &str| toks.get(i).is_some_and(|t| t.is_word(sql, kw));
    let refuse = || {
        format!(
            "{} statements are not allowed (only queries, CREATE TEMP TABLE/VIEW … AS <query> and SET VARIABLE)",
            lex::first_keyword(sql).unwrap_or_else(|| "this".into())
        )
    };
    if word(0, "SET") && word(1, "VARIABLE") {
        let name = toks
            .get(2)
            .and_then(|t| ident_text(sql, t))
            .ok_or_else(refuse)?;
        let eq = toks.get(3).ok_or_else(refuse)?;
        if !(eq.is_punct(sql, '=') || eq.is_word(sql, "TO")) {
            return Err(refuse());
        }
        let expr = &sql[eq.end..];
        return Ok(Setup::SetVariable { name, expr });
    }
    if word(0, "CREATE") {
        let mut i = 1;
        let mut head = String::from("CREATE");
        if word(i, "OR") && word(i + 1, "REPLACE") {
            head.push_str(" OR REPLACE");
            i += 2;
        }
        if !(word(i, "TEMP") || word(i, "TEMPORARY")) {
            return Err("only TEMP tables and views can be created".into());
        }
        i += 1;
        let kind = if word(i, "TABLE") {
            "TABLE"
        } else if word(i, "VIEW") {
            "VIEW"
        } else {
            return Err(refuse());
        };
        head.push_str(" TEMP ");
        head.push_str(kind);
        i += 1;
        if word(i, "IF") && word(i + 1, "NOT") && word(i + 2, "EXISTS") {
            head.push_str(" IF NOT EXISTS");
            i += 3;
        }
        let name = toks
            .get(i)
            .and_then(|t| ident_text(sql, t))
            .ok_or_else(refuse)?;
        let as_ = toks
            .get(i + 1)
            .filter(|t| t.is_word(sql, "AS"))
            .ok_or_else(|| {
                format!("CREATE TEMP {kind} {name}: only the form `… AS <query>` is allowed")
            })?;
        return Ok(Setup::Create {
            head,
            name,
            body: &sql[as_.end..],
        });
    }
    Err(refuse())
}

/// The query DuckDB must accept for dashboard statement `sql`: the statement
/// itself, the query of `CREATE TEMP … AS <query>`, or `SELECT <expr>` for
/// `SET VARIABLE`. Any other statement is an error.
pub fn statement_query(sql: &str) -> Result<String, String> {
    let sql = sql.trim().trim_end_matches(';');
    Ok(match classify(sql)? {
        Setup::Query => sql.to_string(),
        Setup::Create { body, .. } => body.to_string(),
        Setup::SetVariable { expr, .. } => format!("SELECT {expr}"),
    })
}

/// Is `sql` exactly one query, by DuckDB's parser (`json_serialize_sql`
/// parses without binding or running, and serialises only SELECT
/// statements: `EXPLAIN`, `PREPARE`, `COPY`, `PRAGMA`, `CALL`, … fail)?
pub fn check_query(duck: &mut dyn Duck, sql: &str) -> Result<(), String> {
    serialize(duck, sql).map(|_| ())
}

const MACROS_SQL: &str = "SELECT DISTINCT lower(function_name)::VARCHAR, internal::VARCHAR, \
     coalesce(macro_definition, '')::VARCHAR FROM duckdb_functions() \
     WHERE function_type IN ('macro', 'table_macro')";

impl Enforcer {
    /// Check `policy` against the snapshot whose default database is
    /// `default_db`: every table and column it names exists, no relation uses
    /// a reserved name, and each row filter parses, reads only claims and
    /// binds. The row filters are qualified with the snapshot's names.
    pub fn compile(
        policy: &Policy,
        duck: &mut dyn Duck,
        default_db: &str,
    ) -> Result<Enforcer, String> {
        let mut enf = Enforcer {
            policy: policy.clone(),
            default_db: default_db.to_string(),
            bodies: Vec::new(),
            denied_functions: HashSet::new(),
        };
        enf.denied_functions = denied_macros(duck)?;
        let reserved = duck.rows(&format!(
            "SELECT (database_name || '.' || schema_name || '.' || table_name)::VARCHAR FROM duckdb_tables() \
             WHERE lower(table_name) LIKE '{RESERVED_PREFIX}%' UNION ALL \
             SELECT (database_name || '.' || schema_name || '.' || view_name)::VARCHAR FROM duckdb_views() \
             WHERE NOT internal AND lower(view_name) LIKE '{RESERVED_PREFIX}%'"
        ))?;
        if let Some(r) = reserved.first().and_then(|r| r[0].clone()) {
            return Err(format!(
                "the snapshot has a table or view with a reserved name ({r}): rename it"
            ));
        }
        for t in &policy.tables {
            let cols: Vec<String> = duck
                .rows(&format!(
                    "SELECT column_name::VARCHAR FROM duckdb_columns() \
                     WHERE database_name = {} AND lower(schema_name) = lower({}) AND lower(table_name) = lower({})",
                    sql_string_literal(default_db),
                    sql_string_literal(&t.schema),
                    sql_string_literal(&t.table)
                ))?
                .into_iter()
                .filter_map(|r| r.into_iter().next().flatten())
                .collect();
            if cols.is_empty() {
                return Err(format!(
                    "policy: table '{}' does not exist in database '{default_db}'",
                    t.name()
                ));
            }
            for c in &t.columns {
                if !cols.iter().any(|x| x.eq_ignore_ascii_case(&c.column)) {
                    return Err(format!(
                        "policy: table '{}' has no column '{}'",
                        t.name(),
                        c.column
                    ));
                }
            }
            let target = format!(
                "{}.{}.{}",
                quote_ident(default_db),
                quote_ident(&t.schema),
                quote_ident(&t.table)
            );
            let body = match &t.rows {
                None => format!("SELECT * FROM {target}"),
                Some(cond) => {
                    let sql = format!("SELECT * FROM {target} WHERE ({cond})");
                    let none = HashSet::new();
                    let mut ast = serialize(duck, &sql)
                        .map_err(|e| format!("policy: rows of '{}': {e}", t.name()))?;
                    if !only_a_filter(&ast["statements"][0]["node"]) {
                        return Err(format!(
                            "policy: rows of '{}' must be one condition (it ends the WHERE clause)",
                            t.name()
                        ));
                    }
                    // The target is already qualified, so only the condition
                    // can change.
                    let mut w = Walker::new(Mode::Rows, &enf, &none);
                    w.walk(&mut ast["statements"])
                        .map_err(|e| format!("policy: rows of '{}': {e}", t.name()))?;
                    deserialize(duck, &ast)?
                }
            };
            // Binds? (Claims are unset here: getvariable() is NULL.)
            duck.rows(&format!(
                "SELECT count(*)::VARCHAR FROM ({body}) AS {} LIMIT 0",
                quote_ident(&t.table)
            ))
            .map_err(|e| format!("policy: rows of '{}': {e}", t.name()))?;
            enf.bodies.push(body);
        }
        Ok(enf)
    }

    /// Rewrite one dashboard (`(is_setup, sql)` statements, as planned).
    /// `Err((statement index, reason))` for the first statement the policy
    /// refuses.
    pub fn rewrite_script(
        &self,
        duck: &mut dyn Duck,
        stmts: &[(bool, String)],
    ) -> Result<Script, (usize, String)> {
        let mut out = Script::default();
        let mut all = BTreeSet::new();
        for (i, r) in self.rewrite_each(duck, stmts).into_iter().enumerate() {
            let (text, used, cols) = r.map_err(|e| (i, e))?;
            all.extend(used.iter().copied());
            out.stmts.push((stmts[i].0, text));
            out.uses.push((used, cols));
        }
        out.tables = all.into_iter().collect();
        Ok(out)
    }

    /// Rewrite every statement, carrying on after a refusal (for the lint):
    /// the rewritten SQL, the policy tables and the column names it uses.
    #[allow(clippy::type_complexity)]
    pub fn rewrite_each(
        &self,
        duck: &mut dyn Duck,
        stmts: &[(bool, String)],
    ) -> Vec<Result<(String, BTreeSet<usize>, BTreeSet<String>), String>> {
        let mut temps: HashSet<String> = HashSet::new();
        stmts
            .iter()
            .map(|(_, sql)| {
                let sql = sql.trim().trim_end_matches(';');
                match classify(sql)? {
                    Setup::Query => self.rewrite_query(duck, sql, &temps),
                    Setup::Create { head, name, body } => {
                        let lower = name.to_ascii_lowercase();
                        if lower.starts_with(RESERVED_PREFIX) {
                            return Err(format!("'{name}' is a reserved name"));
                        }
                        let r = self.rewrite_query(duck, body, &temps);
                        // Later statements mean the temp object, even if this
                        // one was refused.
                        temps.insert(lower);
                        let (q, used, cols) = r?;
                        Ok((format!("{head} {} AS {q}", quote_ident(&name)), used, cols))
                    }
                    Setup::SetVariable { name, expr } => {
                        let lower = name.to_ascii_lowercase();
                        if lower.starts_with(CLAIM_PREFIX) || lower.starts_with(RESERVED_PREFIX) {
                            return Err(format!("variable '{name}' is set by the server"));
                        }
                        if !valid_ident(&name) {
                            return Err(format!("invalid variable name '{name}'"));
                        }
                        let (q, used, cols) =
                            self.rewrite_query(duck, &format!("SELECT {expr}"), &temps)?;
                        Ok((format!("SET VARIABLE {name} = ({q})"), used, cols))
                    }
                }
            })
            .collect()
    }

    /// Rewrite one query, then parse the result again and verify it.
    fn rewrite_query(
        &self,
        duck: &mut dyn Duck,
        sql: &str,
        temps: &HashSet<String>,
    ) -> Result<(String, BTreeSet<usize>, BTreeSet<String>), String> {
        let mut ast = serialize(duck, sql)?;
        let mut w = Walker::new(Mode::Rewrite, self, temps);
        w.walk(&mut ast["statements"])?;
        let text = deserialize(duck, &ast)?;
        let mut again = serialize(duck, &text)
            .map_err(|e| format!("internal: the rewritten query does not parse: {e}"))?;
        Walker::new(Mode::Verify, self, temps).walk(&mut again["statements"])?;
        Ok((text, w.used, w.columns))
    }

    /// The statements that create the views of `tables` for `viewer`. Run
    /// them per request, after the claim variables are set and before the
    /// dashboard's (rewritten) statements.
    pub fn view_statements(&self, tables: &[usize], viewer: &Viewer) -> Vec<String> {
        tables
            .iter()
            .filter_map(|&i| Some((i, self.policy.tables.get(i)?, self.bodies.get(i)?)))
            .map(|(i, t, body)| {
                let (mut exclude, mut replace) = (Vec::new(), Vec::new());
                for c in &t.columns {
                    if c.allows(viewer) {
                        continue;
                    }
                    let q = quote_ident(&c.column);
                    match c.otherwise {
                        Mask::Deny => exclude.push(q),
                        Mask::Null => replace.push(format!("(CASE WHEN false THEN {q} END) AS {q}")),
                        Mask::Mask => replace.push(format!(
                            "(CASE WHEN {q} IS NULL THEN NULL ELSE '****' END) AS {q}"
                        )),
                        Mask::Hash => replace.push(format!(
                            "left(sha256({} || CAST({q} AS VARCHAR)), 16) AS {q}",
                            sql_string_literal(&self.policy.hash_key)
                        )),
                    }
                }
                let mut proj = String::from("*");
                if !exclude.is_empty() {
                    proj.push_str(&format!(" EXCLUDE ({})", exclude.join(", ")));
                }
                if !replace.is_empty() {
                    proj.push_str(&format!(" REPLACE ({})", replace.join(", ")));
                }
                format!(
                    "CREATE OR REPLACE TEMP VIEW {VIEW_PREFIX}{i} AS SELECT {proj} FROM ({body}) AS {}",
                    quote_ident(&t.table)
                )
            })
            .collect()
    }

    /// Lint: the columns statement `uses` references that `viewer`s outside
    /// an allow list can't see — `(table, rule)`.
    pub fn restricted_columns<'a>(
        &'a self,
        uses: &(BTreeSet<usize>, BTreeSet<String>),
    ) -> Vec<(&'a TablePolicy, &'a ColumnRule)> {
        let (tables, cols) = uses;
        tables
            .iter()
            .filter_map(|&i| self.policy.tables.get(i))
            .flat_map(|t| {
                t.columns
                    .iter()
                    .filter(|c| cols.contains(&c.column.to_ascii_lowercase()))
                    .map(move |c| (t, c))
            })
            .collect()
    }
}

/// Is `node` still `SELECT * FROM <target> WHERE <condition>` — nothing
/// the condition text could have added (a UNION, GROUP BY, LIMIT, …)?
fn only_a_filter(node: &Value) -> bool {
    let empty = |k: &str| match node.get(k) {
        None | Some(Value::Null) => true,
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    };
    node.get("type").and_then(Value::as_str) == Some("SELECT_NODE")
        && node.pointer("/from_table/type").and_then(Value::as_str) == Some("BASE_TABLE")
        && node
            .pointer("/cte_map/map")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        && node
            .get("select_list")
            .and_then(Value::as_array)
            .is_some_and(|l| {
                l.len() == 1 && l[0].get("class").and_then(Value::as_str) == Some("STAR")
            })
        && [
            "modifiers",
            "group_expressions",
            "group_sets",
            "having",
            "qualify",
            "sample",
        ]
        .iter()
        .all(|k| empty(k))
        && node.get("where_clause").is_some_and(|w| !w.is_null())
}

/// Macros a dashboard may not call: every user macro, and the built-in ones
/// whose body reads tables (`pg_get_viewdef`, …) or calls such a macro.
fn denied_macros(duck: &mut dyn Duck) -> Result<HashSet<String>, String> {
    let rows = duck.rows(MACROS_SQL)?;
    let mut denied = HashSet::new();
    let mut builtin = Vec::new();
    for r in rows {
        let name = r.first().cloned().flatten().unwrap_or_default();
        let internal = r.get(1).cloned().flatten().as_deref() == Some("true");
        let def = r.get(2).cloned().flatten().unwrap_or_default();
        if internal {
            builtin.push((name, def));
        } else {
            denied.insert(name);
        }
    }
    let words = |def: &str| -> HashSet<String> {
        lex::tokenize(def)
            .into_iter()
            .filter(|t| t.kind == TokKind::Word || t.kind == TokKind::QuotedIdent)
            .filter_map(|t| ident_text(def, &t))
            .map(|w| w.to_ascii_lowercase())
            .collect()
    };
    let builtin: Vec<(String, HashSet<String>)> =
        builtin.into_iter().map(|(n, d)| (n, words(&d))).collect();
    loop {
        let before = denied.len();
        for (name, w) in &builtin {
            if ["select", "from", "table", "query", "query_table"]
                .iter()
                .any(|k| w.contains(*k))
                || w.iter().any(|x| denied.contains(x))
            {
                denied.insert(name.clone());
            }
        }
        if denied.len() == before {
            return Ok(denied);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_policy() {
        let p = Policy::parse(
            r#"{"tables": {
                "sales": {"rows": "region = getvariable('__claim_region')",
                          "columns": {"margin": {"allow_groups": ["finance"], "else": "null"},
                                      "email": "hash", "ssn": "deny"}},
                "ref.regions": {"rows": true}},
              "table_functions": ["read_parquet"]}"#,
        )
        .unwrap();
        assert_eq!(p.tables.len(), 2);
        let s = &p.tables[p.find("", "SALES").unwrap()];
        assert_eq!(s.schema, "main");
        assert!(s.rows.is_some());
        let m = s.rule("Margin").unwrap();
        assert_eq!(
            (m.otherwise, m.groups.clone()),
            (Mask::Null, vec!["finance".to_string()])
        );
        assert_eq!(s.rule("email").unwrap().otherwise, Mask::Hash);
        assert_eq!(s.rule("ssn").unwrap().otherwise, Mask::Deny);
        let r = &p.tables[p.find("ref", "regions").unwrap()];
        assert_eq!(
            (r.rows.as_deref(), r.name()),
            (None, "ref.regions".to_string())
        );
        assert!(p.find("", "regions").is_none(), "schema matters");
        assert_eq!(p.table_functions, vec!["read_parquet".to_string()]);
    }

    #[test]
    fn policy_mistakes_are_errors() {
        for bad in [
            r#"[]"#,
            r#"{"tables": {"t": {}}}"#,
            r#"{"tables": {"t": {"rows": false}}}"#,
            r#"{"tables": {"t": {"rows": ""}}}"#,
            r#"{"tables": {"t": {"rows": true, "colums": {}}}}"#,
            r#"{"tables": {"t": {"rows": true, "columns": {"c": "hide"}}}}"#,
            r#"{"tables": {"t": {"rows": true, "columns": {"c": {"allow_group": ["x"]}}}}}"#,
            r#"{"tables": {"t": {"rows": true, "columns": {"c": {"allow_groups": "x"}}}}}"#,
            r#"{"tables": {"a.b.c": {"rows": true}}}"#,
            r#"{"tables": {"t; DROP": {"rows": true}}}"#,
            r#"{"tables": {"__anofox_rls_0": {"rows": true}}}"#,
            r#"{"tables": {"t": {"rows": true}, "T": {"rows": true}}}"#,
            r#"{"table_functions": ["query"]}"#,
            r#"{"table_functions": ["duckdb_tables"]}"#,
            r#"{"tabels": {}}"#,
        ] {
            assert!(Policy::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn column_rules_follow_the_viewer() {
        let rule = ColumnRule {
            column: "salary".into(),
            groups: vec!["hr".into()],
            users: vec!["ceo@x".into()],
            otherwise: Mask::Null,
        };
        let v = |u: Option<&str>, g: &[&str]| Viewer {
            user: u.map(str::to_string),
            groups: g.iter().map(|s| s.to_string()).collect(),
        };
        assert!(rule.allows(&v(Some("a"), &["hr"])));
        assert!(rule.allows(&v(Some("ceo@x"), &[])));
        assert!(!rule.allows(&v(Some("a"), &["HR"])), "groups are exact");
        assert!(!rule.allows(&v(None, &[])));
        assert_eq!(rule.audience(), "groups hr; users ceo@x");
    }

    #[test]
    fn setup_statements_are_classified() {
        assert!(matches!(classify("SELECT 1"), Ok(Setup::Query)));
        assert!(matches!(
            classify("WITH a AS (SELECT 1) FROM a"),
            Ok(Setup::Query)
        ));
        match classify("create or replace temporary table \"My T\" as select 1") {
            Ok(Setup::Create { head, name, body }) => {
                assert_eq!(head, "CREATE OR REPLACE TEMP TABLE");
                assert_eq!(name, "My T");
                assert_eq!(body.trim(), "select 1");
            }
            _ => panic!(),
        }
        match classify("SET VARIABLE lo = (SELECT min(x) FROM t)") {
            Ok(Setup::SetVariable { name, expr }) => {
                assert_eq!(name, "lo");
                assert_eq!(expr.trim(), "(SELECT min(x) FROM t)");
            }
            _ => panic!(),
        }
        for bad in [
            "CREATE TABLE t AS SELECT 1",
            "CREATE TEMP TABLE t (a INT)",
            "CREATE TEMP MACRO m() AS (SELECT 1)",
            "CREATE TEMP TABLE main.t AS SELECT 1",
            "INSERT INTO t SELECT 1",
            "PIVOT sales ON region USING sum(x)",
            "SET threads = 1",
            "RESET VARIABLE x",
            "ATTACH 'x.db'",
            "COPY t TO 'x.csv'",
        ] {
            assert!(classify(bad).is_err(), "{bad}");
        }
    }

    /// A walk over hand-written trees (the DuckDB-backed tests are in
    /// `tests/policy_duckdb.rs`).
    fn enforcer(policy: &str) -> Enforcer {
        Enforcer {
            policy: Policy {
                hash_key: "k".into(),
                ..Policy::parse(policy).unwrap()
            },
            default_db: "db".into(),
            bodies: Vec::new(),
            denied_functions: ["pg_get_viewdef".to_string()].into_iter().collect(),
        }
    }

    fn base(name: &str) -> Value {
        serde_json::json!({"type": "BASE_TABLE", "alias": "", "sample": null,
            "schema_name": "", "table_name": name, "catalog_name": "", "at_clause": null})
    }

    #[test]
    fn walk_replaces_tables_and_renames_ctes() {
        let enf = enforcer(r#"{"tables": {"sales": {"rows": true}}}"#);
        let temps = HashSet::new();
        // WITH s AS (FROM sales) FROM s, sales
        let mut ast = serde_json::json!({"type": "SELECT_NODE", "modifiers": [],
            "cte_map": {"map": [{"key": "s", "value": {"query": {"node": {
                "type": "SELECT_NODE", "modifiers": [], "cte_map": {"map": []},
                "from_table": base("sales")}}}}]},
            "from_table": {"type": "JOIN", "alias": "", "sample": null,
                "left": base("s"), "right": base("Sales")}});
        let mut w = Walker::new(Mode::Rewrite, &enf, &temps);
        w.walk(&mut ast).unwrap();
        assert_eq!(ast["cte_map"]["map"][0]["key"], "__anofox_cte_1");
        let inner = &ast["cte_map"]["map"][0]["value"]["query"]["node"]["from_table"];
        assert_eq!(
            (
                &inner["catalog_name"],
                &inner["table_name"],
                &inner["alias"]
            ),
            (&"temp".into(), &"__anofox_rls_0".into(), &"sales".into())
        );
        let l = &ast["from_table"]["left"];
        assert_eq!(
            (&l["table_name"], &l["alias"]),
            (&"__anofox_cte_1".into(), &"s".into())
        );
        assert_eq!(ast["from_table"]["right"]["table_name"], "__anofox_rls_0");
        assert_eq!(w.used.iter().copied().collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn walk_fails_closed() {
        let enf = enforcer(r#"{"tables": {"sales": {"rows": true}}}"#);
        let temps = HashSet::new();
        let run = |mut v: Value| Walker::new(Mode::Rewrite, &enf, &temps).walk(&mut v);
        assert!(run(base("payroll"))
            .unwrap_err()
            .contains("no access policy"));
        let mut q = base("sales");
        q["catalog_name"] = "db".into();
        assert!(run(q).unwrap_err().contains("qualified"));
        let mut q = base("sales");
        q["at_clause"] = serde_json::json!({"unit": "VERSION"});
        assert!(run(q).is_err());
        let tf = |f: &str| {
            serde_json::json!({"type": "TABLE_FUNCTION", "alias": "", "sample": null,
                "function": {"class": "FUNCTION", "type": "FUNCTION", "function_name": f,
                    "schema": "", "catalog": "", "children": []}})
        };
        assert!(run(tf("query")).is_err());
        assert!(run(tf("duckdb_tables")).is_err());
        assert!(
            run(tf("read_parquet")).is_err(),
            "not allowed by this policy"
        );
        assert!(run(tf("range")).is_ok());
        let call = serde_json::json!({"class": "FUNCTION", "type": "FUNCTION",
            "function_name": "PG_GET_VIEWDEF", "children": []});
        assert!(run(call).unwrap_err().contains("macro"));
        let show = serde_json::json!({"type": "SHOW_REF", "alias": "", "sample": null,
            "table_name": "\"tables\"", "query": null});
        assert!(run(show).is_err());
        let odd = serde_json::json!({"type": "COLUMN_DATA", "alias": "", "sample": null});
        assert!(run(odd).is_err());
        let node =
            serde_json::json!({"type": "STATEMENT_NODE", "modifiers": [], "cte_map": {"map": []}});
        assert!(run(node).is_err());
        // A serializer that drops default-valued keys can't hide a table or
        // a table function.
        let bare = serde_json::json!({"type": "BASE_TABLE", "table_name": "payroll"});
        assert!(run(bare).unwrap_err().contains("no access policy"));
        let bare = serde_json::json!({"type": "TABLE_FUNCTION",
            "function": {"class": "FUNCTION", "type": "FUNCTION", "function_name": "query"}});
        assert!(run(bare).is_err());
        let node = serde_json::json!({"type": "CTE_NODE"});
        assert!(run(node).is_err());
        // A recursive CTE outside its WITH can't be resolved.
        let rec = serde_json::json!({"type": "RECURSIVE_CTE_NODE", "modifiers": [],
            "cte_map": {"map": []}, "cte_name": "r"});
        assert!(run(rec).is_err());
    }

    #[test]
    fn verify_accepts_only_views_temps_and_renamed_ctes() {
        let enf = enforcer(r#"{"tables": {"sales": {"rows": true}}}"#);
        let temps: HashSet<String> = ["t1".to_string()].into_iter().collect();
        let run = |mut v: Value| Walker::new(Mode::Verify, &enf, &temps).walk(&mut v);
        let q = |c: &str, s: &str, n: &str| {
            let mut b = base(n);
            b["catalog_name"] = c.into();
            b["schema_name"] = s.into();
            b
        };
        assert!(run(q("temp", "main", "__anofox_rls_0")).is_ok());
        assert!(run(q("temp", "main", "t1")).is_ok());
        assert!(
            run(q("temp", "main", "__anofox_rls_1")).is_err(),
            "no such policy table"
        );
        assert!(run(q("db", "main", "sales")).is_err());
        assert!(run(q("", "", "sales")).is_err());
        assert!(run(q("", "", "__anofox_cte_1")).is_err(), "not in scope");
    }

    #[test]
    fn row_filters_read_only_claims() {
        let enf = enforcer(r#"{"tables": {"sales": {"rows": true}}}"#);
        let temps = HashSet::new();
        let gv = |arg: Value| {
            serde_json::json!({"class": "FUNCTION", "type": "FUNCTION", "function_name": "getvariable",
                "children": [{"class": "CONSTANT", "type": "VALUE_CONSTANT", "value": arg}]})
        };
        let lit =
            |s: &str| serde_json::json!({"type": {"id": "VARCHAR"}, "is_null": false, "value": s});
        let run = |mut v: Value| Walker::new(Mode::Rows, &enf, &temps).walk(&mut v);
        assert!(run(gv(lit("__claim_user"))).is_ok());
        assert!(run(gv(lit("__CLAIM_groups"))).is_ok());
        assert!(run(gv(lit("region"))).is_err(), "a viewer variable");
        assert!(run(gv(
            serde_json::json!({"type": {"id": "INTEGER"}, "value": 1})
        ))
        .is_err());
        // Plain table names in a row filter point at the snapshot.
        let mut t = base("user_regions");
        run(t.clone()).unwrap();
        Walker::new(Mode::Rows, &enf, &temps).walk(&mut t).unwrap();
        assert_eq!(
            (&t["catalog_name"], &t["schema_name"]),
            (&"db".into(), &"main".into())
        );
    }

    #[test]
    fn views_follow_the_viewer() {
        let mut enf = enforcer(
            r#"{"tables": {"emp": {"rows": true, "columns": {
                "salary": {"allow_groups": ["hr"], "else": "null"},
                "email": "mask", "ssn": "deny", "name": {"allow_users": ["x"], "else": "hash"}}}}}"#,
        );
        enf.bodies = vec!["SELECT * FROM \"db\".\"main\".\"emp\"".into()];
        let anon = enf.view_statements(&[0], &Viewer::default());
        assert_eq!(anon.len(), 1);
        let v = &anon[0];
        assert!(v.starts_with(
            "CREATE OR REPLACE TEMP VIEW __anofox_rls_0 AS SELECT * EXCLUDE (\"ssn\") REPLACE ("
        ));
        assert!(v.contains("(CASE WHEN false THEN \"salary\" END) AS \"salary\""));
        assert!(v.contains("ELSE '****' END) AS \"email\""));
        assert!(v.contains("left(sha256('k' || CAST(\"name\" AS VARCHAR)), 16) AS \"name\""));
        let hr = Viewer {
            user: Some("x".into()),
            groups: vec!["hr".into()],
        };
        let v = &enf.view_statements(&[0], &hr)[0];
        assert!(!v.contains("\"salary\" END"), "{v}");
        assert!(!v.contains("sha256"), "{v}");
        assert!(v.contains("EXCLUDE (\"ssn\")"));
    }
}
