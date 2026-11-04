//! Canonical schemas and customer mappings (#194 A3, plan-dashboard-to-agent
//! §4): a dashboard meant for many customers reads a **contract**
//! (`canonical/<id>.sql`, `CREATE TABLE` statements) and each customer maps
//! its own source onto it (`customer-mapping/<customer>/views.sql`, only
//! `CREATE [OR REPLACE] VIEW <contract table> AS <query over source.*>`).
//! [`conform`] checks one mapping against one contract on the customer's
//! data, in a DuckDB instance set up by [`instance_setup`]: only `source`
//! attached `READ_ONLY`, then external access off and the configuration
//! locked (H1) — never the operator's serving session.
//!
//! Markers in the contract (line comments after a column): `-- @optional`
//! (the column may be missing; questions that read it are then off) and
//! `-- @scope <name>` (a row-policy column the mapping must fill from a
//! source column, M9). In the mapping, `-- @scope <name> constant` says a
//! scope column is legitimately one value for this customer (R2).

use crate::host::policy::{self, Duck};
use crate::sql::lex::{self, TokKind};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The schema the contract's tables are created in, inside the build
/// instance (the views themselves live in `main`).
pub const CONTRACT_SCHEMA: &str = "anofox_contract";

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub name: String,
    /// The column's comment, markers removed.
    pub doc: String,
    pub optional: bool,
    /// `-- @scope <name>`: the policy dimension this column carries.
    pub scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Contract {
    pub tables: Vec<Table>,
    /// The `CREATE TABLE` statements, as written.
    pub statements: Vec<String>,
}

impl Contract {
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }
}

/// One mapping view.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub name: String,
    /// The query after `AS`.
    pub query: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mapping {
    pub views: Vec<View>,
    /// Scope names the mapping marks `-- @scope <name> constant`.
    pub constant_scopes: BTreeSet<String>,
}

fn ident(src: &str, t: &lex::Token) -> Option<String> {
    match t.kind {
        TokKind::Word => Some(t.text(src).to_ascii_lowercase()),
        TokKind::QuotedIdent => {
            let s = t.text(src);
            Some(
                s[1..s.len().saturating_sub(1)]
                    .replace("\"\"", "\"")
                    .to_ascii_lowercase(),
            )
        }
        _ => None,
    }
}

/// The marker words of a line comment (`-- @scope tenant` → `["@scope",
/// "tenant"]`) and the rest of its text.
fn comment_parts(c: &str) -> (Vec<String>, String) {
    let body = c.trim_start_matches('-').trim();
    let mut markers = Vec::new();
    let mut doc = Vec::new();
    let mut it = body.split_whitespace().peekable();
    while let Some(w) = it.next() {
        if w == "@optional" {
            markers.push(w.to_string());
        } else if w == "@scope" {
            markers.push(w.to_string());
            if let Some(n) = it.next() {
                markers.push(n.to_ascii_lowercase());
            }
            if it.peek() == Some(&"constant") {
                markers.push("constant".into());
                it.next();
            }
        } else {
            doc.push(w);
        }
    }
    (markers, doc.join(" "))
}

/// The meaningful tokens of `src` (no whitespace), comments kept.
fn tokens(src: &str) -> Vec<lex::Token> {
    lex::tokenize(src)
        .into_iter()
        .filter(|t| t.kind != TokKind::Whitespace)
        .collect()
}

const TABLE_CONSTRAINTS: [&str; 5] = ["primary", "foreign", "unique", "check", "constraint"];

/// Parse `canonical/<id>.sql`: only `CREATE TABLE` statements.
pub fn parse_contract(text: &str) -> Result<Contract, String> {
    let mut c = Contract {
        tables: Vec::new(),
        statements: Vec::new(),
    };
    for stmt in lex::split_statements(text) {
        let toks = tokens(&stmt);
        let code: Vec<&lex::Token> = toks.iter().filter(|t| !t.is_trivia()).collect();
        if code.is_empty() {
            continue;
        }
        let word = |i: usize, w: &str| code.get(i).is_some_and(|t| t.is_word(&stmt, w));
        if !(word(0, "create") && word(1, "table")) {
            return Err(format!(
                "a contract holds only CREATE TABLE statements: '{}'",
                stmt.trim().chars().take(60).collect::<String>()
            ));
        }
        let name = code
            .get(2)
            .and_then(|t| ident(&stmt, t))
            .filter(|_| code.get(3).is_some_and(|t| t.is_punct(&stmt, '(')))
            .ok_or("a contract table is CREATE TABLE <name> (…), unqualified")?;
        if c.table(&name).is_some() {
            return Err(format!("contract table '{name}' is declared twice"));
        }
        // the items inside the parentheses, split on depth-1 commas; each
        // item's comments run up to the next item's first token
        let open = toks
            .iter()
            .position(|t| t.is_punct(&stmt, '('))
            .unwrap_or(0);
        let mut depth = 0;
        let mut items: Vec<Vec<&lex::Token>> = vec![Vec::new()];
        for t in &toks[open..] {
            if t.is_punct(&stmt, '(') {
                depth += 1;
                if depth == 1 {
                    continue;
                }
            } else if t.is_punct(&stmt, ')') {
                depth -= 1;
                if depth == 0 {
                    continue;
                }
            }
            if depth == 0 {
                // a trailing comment after the closing parenthesis
                if t.kind == TokKind::LineComment {
                    items.last_mut().unwrap().push(t);
                }
                continue;
            }
            if depth == 1 && t.is_punct(&stmt, ',') {
                items.push(Vec::new());
                continue;
            }
            // a comment right after a comma belongs to the item before it
            if t.kind == TokKind::LineComment && items.last().is_some_and(|i| i.is_empty()) {
                let n = items.len();
                if n >= 2 {
                    items[n - 2].push(t);
                    continue;
                }
            }
            items.last_mut().unwrap().push(t);
        }
        let mut columns = Vec::new();
        for item in items {
            let Some(first) = item.iter().find(|t| !t.is_trivia()) else {
                continue;
            };
            let Some(col) = ident(&stmt, first) else {
                continue;
            };
            if first.kind == TokKind::Word && TABLE_CONSTRAINTS.contains(&col.as_str()) {
                continue;
            }
            let (mut markers, mut doc) = (Vec::new(), Vec::new());
            for t in item.iter().filter(|t| t.kind == TokKind::LineComment) {
                let (m, d) = comment_parts(t.text(&stmt));
                markers.extend(m);
                if !d.is_empty() {
                    doc.push(d);
                }
            }
            if markers.iter().any(|m| m == "constant") {
                return Err(format!(
                    "{name}.{col}: 'constant' belongs in a customer's mapping, not the contract"
                ));
            }
            let scope = markers
                .iter()
                .position(|m| m == "@scope")
                .map(|i| markers.get(i + 1).cloned().unwrap_or_else(|| col.clone()));
            columns.push(Column {
                name: col,
                doc: doc.join(" "),
                optional: markers.iter().any(|m| m == "@optional"),
                scope,
            });
        }
        if columns.is_empty() {
            return Err(format!("contract table '{name}' has no columns"));
        }
        c.tables.push(Table { name, columns });
        c.statements.push(stmt.trim().to_string());
    }
    if c.tables.is_empty() {
        return Err("the contract declares no table".into());
    }
    Ok(c)
}

/// Parse `customer-mapping/<customer>/views.sql`: one `CREATE [OR REPLACE]
/// VIEW <contract table> AS <query>` per contract table, nothing else (H1).
pub fn parse_mapping(text: &str, contract: &Contract) -> Result<Mapping, Vec<String>> {
    let mut errs = Vec::new();
    let mut m = Mapping {
        views: Vec::new(),
        constant_scopes: BTreeSet::new(),
    };
    for t in lex::tokenize(text) {
        if t.kind == TokKind::LineComment {
            let (markers, _) = comment_parts(t.text(text));
            if let [a, n, c] = markers.as_slice() {
                if a == "@scope" && c == "constant" {
                    m.constant_scopes.insert(n.clone());
                }
            }
        }
    }
    for stmt in lex::split_statements(text) {
        let toks = tokens(&stmt);
        let code: Vec<&lex::Token> = toks.iter().filter(|t| !t.is_trivia()).collect();
        if code.is_empty() {
            continue;
        }
        let shown: String = stmt.split_whitespace().collect::<Vec<_>>().join(" ");
        let shown: String = shown.chars().take(70).collect();
        let w = |i: usize, kw: &str| code.get(i).is_some_and(|t| t.is_word(&stmt, kw));
        let mut i = 1;
        if w(1, "or") && w(2, "replace") {
            i = 3;
        }
        if !(w(0, "create") && w(i, "view")) {
            errs.push(format!(
                "a mapping holds only CREATE [OR REPLACE] VIEW <table> AS <query>: '{shown}'"
            ));
            continue;
        }
        let name = code.get(i + 1).and_then(|t| ident(&stmt, t));
        let as_at = i + 2;
        let (Some(name), true) = (name, w(as_at, "as")) else {
            errs.push(format!(
                "a mapping view is CREATE VIEW <table> AS <query> (unqualified, no column list): '{shown}'"
            ));
            continue;
        };
        if contract.table(&name).is_none() {
            errs.push(format!(
                "view '{name}' is not a contract table (a mapping makes no helper views or tables)"
            ));
            continue;
        }
        if m.views.iter().any(|v| v.name == name) {
            errs.push(format!("view '{name}' is mapped twice"));
            continue;
        }
        let query = stmt[code[as_at].end..].trim().to_string();
        m.views.push(View { name, query });
    }
    for t in &contract.tables {
        if !m.views.iter().any(|v| v.name == t.name) {
            errs.push(format!("contract table '{}' is not mapped", t.name));
        }
    }
    for s in &m.constant_scopes {
        if !contract
            .tables
            .iter()
            .flat_map(|t| &t.columns)
            .any(|c| c.scope.as_deref() == Some(s.as_str()))
        {
            errs.push(format!(
                "'-- @scope {s} constant': the contract has no scope column '{s}'"
            ));
        }
    }
    if errs.is_empty() {
        Ok(m)
    } else {
        Err(errs)
    }
}

/// A SQL string literal.
fn lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// A quoted identifier.
fn qi(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// The statements that set up a build instance on a fresh `:memory:` DuckDB:
/// the customer's database attached as `source`, read-only, then external
/// access off and the configuration locked (H1).
pub fn instance_setup(source_db: &str) -> Vec<String> {
    vec![
        format!(
            "ATTACH {} AS {} (READ_ONLY)",
            lit(source_db),
            policy::MAPPING_SOURCE
        ),
        "SET enable_external_access = false".into(),
        "SET autoinstall_known_extensions = false".into(),
        "SET autoload_known_extensions = false".into(),
        "SET lock_configuration = true".into(),
    ]
}

// ---- the scope-column check (R2): does a view's column come from source? ----

/// What a column resolves to in a query.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Origin {
    /// a `source.*` column
    Source,
    /// a literal, or an expression that reads no source column
    Constant,
    /// this relation doesn't have the column
    Absent,
}

struct Resolver<'a> {
    /// CTE name → its query node, innermost scope last.
    ctes: Vec<Vec<(String, &'a Value)>>,
}

fn st<'v>(v: &'v Value, k: &str) -> &'v str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

impl<'a> Resolver<'a> {
    fn cte(&self, name: &str) -> Option<&'a Value> {
        self.ctes
            .iter()
            .rev()
            .flat_map(|s| s.iter().rev())
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
    }

    /// Column `col` of query node `node`.
    fn node_column(&mut self, node: &'a Value, col: &str) -> Origin {
        match st(node, "type") {
            "SELECT_NODE" => {
                let pushed = self.push_ctes(node);
                let out = self.select_column(node, col);
                if pushed {
                    self.ctes.pop();
                }
                out
            }
            "SET_OPERATION_NODE" => {
                let parts = [node.get("left"), node.get("right")];
                let rs: Vec<Origin> = parts
                    .into_iter()
                    .flatten()
                    .map(|n| self.node_column(n, col))
                    .collect();
                if rs.contains(&Origin::Constant) || rs.is_empty() {
                    Origin::Constant
                } else if rs.iter().all(|r| *r == Origin::Source) {
                    Origin::Source
                } else {
                    Origin::Absent
                }
            }
            // anything else (a shape this check doesn't know): fail closed
            _ => Origin::Constant,
        }
    }

    fn push_ctes(&mut self, node: &'a Value) -> bool {
        let Some(map) = node
            .get("cte_map")
            .and_then(|c| c.get("map"))
            .and_then(Value::as_array)
        else {
            return false;
        };
        let scope: Vec<(String, &'a Value)> = map
            .iter()
            .filter_map(|e| {
                let q = e.get("value")?;
                let n = q
                    .get("query")
                    .and_then(|q| q.get("node"))
                    .or_else(|| q.get("query_node"))?;
                Some((st(e, "key").to_string(), n))
            })
            .collect();
        self.ctes.push(scope);
        true
    }

    fn select_column(&mut self, node: &'a Value, col: &str) -> Origin {
        let list = node
            .get("select_list")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let from = node.get("from_table");
        for e in list {
            let named = |e: &Value| {
                let a = st(e, "alias");
                if !a.is_empty() {
                    return a.eq_ignore_ascii_case(col);
                }
                st(e, "type") == "COLUMN_REF"
                    && e.get("column_names")
                        .and_then(Value::as_array)
                        .and_then(|a| a.last())
                        .and_then(Value::as_str)
                        .is_some_and(|c| c.eq_ignore_ascii_case(col))
            };
            if named(e) {
                return self.expr(e, from);
            }
        }
        // `*` passes the column through from the FROM clause
        if list
            .iter()
            .any(|e| st(e, "class") == "STAR" && st(e, "relation_name").is_empty())
        {
            return match from {
                Some(f) => self.of_column(f, None, col),
                None => Origin::Absent,
            };
        }
        for e in list.iter().filter(|e| st(e, "class") == "STAR") {
            if let Some(f) = from {
                let r = self.of_column(f, Some(st(e, "relation_name")), col);
                if r != Origin::Absent {
                    return r;
                }
            }
        }
        Origin::Absent
    }

    /// An expression: from source only if every column it reads is, and it
    /// reads one at all.
    fn expr(&mut self, e: &'a Value, from: Option<&'a Value>) -> Origin {
        let mut refs = Vec::new();
        collect_refs(e, &mut refs);
        if refs.is_empty() {
            return Origin::Constant;
        }
        for r in refs {
            let (qual, name) = match r.as_slice() {
                [c] => (None, c.as_str()),
                [.., q, c] => (Some(q.as_str()), c.as_str()),
                [] => continue,
            };
            let got = match from {
                Some(f) => self.of_column(f, qual, name),
                None => Origin::Constant,
            };
            if got != Origin::Source {
                return Origin::Constant;
            }
        }
        Origin::Source
    }

    /// Column `col` (of relation `qual`, if named) in a FROM item.
    fn of_column(&mut self, f: &'a Value, qual: Option<&str>, col: &str) -> Origin {
        match st(f, "type") {
            "JOIN" => {
                let rs: Vec<Origin> = [f.get("left"), f.get("right")]
                    .into_iter()
                    .flatten()
                    .map(|x| self.of_column(x, qual, col))
                    .collect();
                if rs.contains(&Origin::Constant) {
                    Origin::Constant
                } else if rs.contains(&Origin::Source) {
                    Origin::Source
                } else {
                    Origin::Absent
                }
            }
            "BASE_TABLE" => {
                let (cat, sch, name, alias) = (
                    st(f, "catalog_name"),
                    st(f, "schema_name"),
                    st(f, "table_name"),
                    st(f, "alias"),
                );
                let rel = if alias.is_empty() { name } else { alias };
                if qual.is_some_and(|q| !q.eq_ignore_ascii_case(rel)) {
                    return Origin::Absent;
                }
                let renamed = renamed(f, col);
                if cat.is_empty() && sch.is_empty() {
                    if let Some(n) = self.cte(name) {
                        return self.node_column(n, &renamed.unwrap_or_else(|| col.to_string()));
                    }
                }
                let source = cat.eq_ignore_ascii_case(policy::MAPPING_SOURCE)
                    || (cat.is_empty() && sch.eq_ignore_ascii_case(policy::MAPPING_SOURCE));
                if source {
                    Origin::Source
                } else {
                    Origin::Constant
                }
            }
            "SUBQUERY" => {
                if qual.is_some_and(|q| !q.eq_ignore_ascii_case(st(f, "alias"))) {
                    return Origin::Absent;
                }
                match f.get("subquery").and_then(|s| s.get("node")) {
                    Some(n) => {
                        let c = renamed(f, col).unwrap_or_else(|| col.to_string());
                        self.node_column(n, &c)
                    }
                    None => Origin::Constant,
                }
            }
            "EMPTY" => Origin::Absent,
            // a table function (range, unnest, …) or anything else: not source
            _ => {
                if qual.is_some_and(|q| !q.eq_ignore_ascii_case(st(f, "alias"))) {
                    Origin::Absent
                } else {
                    Origin::Constant
                }
            }
        }
    }
}

/// `FROM x AS a(c1, c2)`: the inner name of output column `col`, by position.
/// `None` without a column list. A column past the list keeps its name.
fn renamed(f: &Value, col: &str) -> Option<String> {
    let names = f.get("column_name_alias").and_then(Value::as_array)?;
    if names.is_empty() {
        return None;
    }
    names
        .iter()
        .position(|n| n.as_str().is_some_and(|n| n.eq_ignore_ascii_case(col)))
        .map(|i| format!("#{i}"))
        .or_else(|| Some(col.to_string()))
}

/// Every column reference in an expression (not inside a subquery).
fn collect_refs(v: &Value, out: &mut Vec<Vec<String>>) {
    match v {
        Value::Object(o) => {
            if o.get("type").and_then(Value::as_str) == Some("COLUMN_REF") {
                if let Some(a) = o.get("column_names").and_then(Value::as_array) {
                    out.push(
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect(),
                    );
                }
                return;
            }
            if o.contains_key("subquery") || o.contains_key("node") {
                // a scalar subquery: its value isn't a column of this row —
                // treated as no source column (fail closed)
                out.push(vec!["#subquery".into()]);
                return;
            }
            o.values().for_each(|x| collect_refs(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| collect_refs(x, out)),
        _ => {}
    }
}

/// Whether `col` of the mapping query (its parse tree) comes from a
/// `source.*` column.
fn scope_from_source(tree: &Value, col: &str) -> bool {
    let Some(node) = tree
        .get("statements")
        .and_then(|s| s.get(0))
        .and_then(|s| s.get("node"))
    else {
        return false;
    };
    // a renamed column by position (`#i`) only arises inside FROM items; a
    // name starting with '#' never matches a real column, so it fails closed
    Resolver { ctes: Vec::new() }.node_column(node, col) == Origin::Source
}

// ---- the report ----

/// What `--conform` is told besides the files.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Per scope name, the values the operator allows (`--scope tenant=a,b`).
    pub scope_values: BTreeMap<String, Vec<String>>,
}

/// One finding: an error refuses the mapping; a note is information.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub error: bool,
    pub table: String,
    pub column: Option<String>,
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Per contract table, its optional columns the mapping doesn't have.
    pub missing_optional: BTreeMap<String, Vec<String>>,
}

impl Report {
    pub fn ok(&self) -> bool {
        !self.findings.iter().any(|f| f.error)
    }

    fn push(
        &mut self,
        error: bool,
        table: &str,
        column: Option<&str>,
        code: &'static str,
        m: String,
    ) {
        self.findings.push(Finding {
            error,
            table: table.to_string(),
            column: column.map(str::to_string),
            code,
            message: m,
        });
    }

    pub fn to_json(&self) -> Value {
        json!({
            "ok": self.ok(),
            "findings": self.findings.iter().map(|f| json!({
                "severity": if f.error { "error" } else { "note" },
                "table": f.table, "column": f.column, "code": f.code, "message": f.message,
            })).collect::<Vec<_>>(),
            "missing_optional": self.missing_optional,
        })
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        for f in &self.findings {
            let at = match &f.column {
                Some(c) => format!("{}.{c}", f.table),
                None => f.table.clone(),
            };
            s.push_str(&format!(
                "{} [{}] {at}: {}\n",
                if f.error { "error" } else { "note " },
                f.code,
                f.message
            ));
        }
        s.push_str(if self.ok() {
            "the mapping conforms\n"
        } else {
            "the mapping does not conform\n"
        });
        s
    }
}

fn rows(duck: &mut dyn Duck, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
    duck.rows(sql)
}

fn one(duck: &mut dyn Duck, sql: &str) -> Result<Option<String>, String> {
    Ok(rows(duck, sql)?
        .into_iter()
        .next()
        .and_then(|r| r.into_iter().next().flatten()))
}

/// The type family a contract column and a view column are compared by.
fn family(t: &str) -> &'static str {
    let t = t.to_ascii_uppercase();
    let base = t.split('(').next().unwrap_or("").trim();
    match base {
        "TINYINT" | "SMALLINT" | "INTEGER" | "BIGINT" | "HUGEINT" | "UTINYINT" | "USMALLINT"
        | "UINTEGER" | "UBIGINT" | "UHUGEINT" => "integer",
        "FLOAT" | "DOUBLE" | "DECIMAL" => "number",
        "VARCHAR" => "text",
        "BOOLEAN" => "boolean",
        "DATE" => "date",
        b if b.starts_with("TIMESTAMP") => "timestamp",
        b if b.starts_with("TIME") => "time",
        _ => "other",
    }
}

fn compatible(contract: &str, view: &str) -> bool {
    let (c, v) = (family(contract), family(view));
    c == v || (c == "number" && v == "integer") || (c == "timestamp" && v == "date")
}

/// Check `mapping` against `contract` in a build instance (`duck`, already
/// set up with [`instance_setup`]): every statement admissible to the
/// classifier, every table and column present with a compatible type,
/// `NOT NULL` columns without NULLs, scope columns from source (or marked
/// constant) and within the operator's values, keys unique and references
/// resolving. Creates the contract tables (in [`CONTRACT_SCHEMA`]) and the
/// views (in `main`) in that instance.
pub fn conform(
    duck: &mut dyn Duck,
    contract: &Contract,
    mapping: &Mapping,
    opts: &Options,
) -> Result<Report, String> {
    let mut r = Report::default();
    rows(
        duck,
        &format!("CREATE SCHEMA IF NOT EXISTS {CONTRACT_SCHEMA}"),
    )?;
    rows(duck, &format!("USE memory.{CONTRACT_SCHEMA}"))?;
    for s in &contract.statements {
        rows(duck, s).map_err(|e| format!("the contract doesn't parse: {e}"))?;
    }
    rows(duck, "USE memory.main")?;
    // the classifier first: nothing is created from a refused statement
    let mut admitted = Vec::new();
    for v in &mapping.views {
        match policy::check_mapping(duck, &v.query) {
            Ok(()) => admitted.push(v),
            Err(e) => r.push(true, &v.name, None, "mapping/refused", e),
        }
    }
    for v in admitted {
        if let Err(e) = rows(
            duck,
            &format!("CREATE VIEW main.{} AS {}", qi(&v.name), v.query),
        ) {
            r.push(true, &v.name, None, "mapping/error", e);
            continue;
        }
        check_table(duck, contract, mapping, v, opts, &mut r)?;
    }
    check_keys(duck, contract, &mut r)?;
    Ok(r)
}

fn check_table(
    duck: &mut dyn Duck,
    contract: &Contract,
    mapping: &Mapping,
    v: &View,
    opts: &Options,
    r: &mut Report,
) -> Result<(), String> {
    let Some(t) = contract.table(&v.name) else {
        return Ok(());
    };
    let want: BTreeMap<String, (String, bool)> = rows(
        duck,
        &format!(
            "SELECT lower(column_name), data_type, (NOT is_nullable)::VARCHAR FROM duckdb_columns() \
             WHERE database_name = 'memory' AND schema_name = {} AND table_name = {}",
            lit(CONTRACT_SCHEMA),
            lit(&t.name)
        ),
    )?
    .into_iter()
    .filter_map(|r| Some((r[0].clone()?, (r[1].clone()?, r[2].as_deref() == Some("true")))))
    .collect();
    let have: BTreeMap<String, String> = rows(
        duck,
        &format!(
            "SELECT lower(column_name), data_type FROM duckdb_columns() \
             WHERE database_name = 'memory' AND schema_name = 'main' AND table_name = {}",
            lit(&t.name)
        ),
    )?
    .into_iter()
    .filter_map(|r| Some((r[0].clone()?, r[1].clone()?)))
    .collect();
    let tree = policy::parse_tree(duck, &v.query)?;
    for c in &t.columns {
        let Some((ctype, not_null)) = want.get(&c.name) else {
            continue;
        };
        let Some(vtype) = have.get(&c.name) else {
            if c.optional {
                r.missing_optional
                    .entry(t.name.clone())
                    .or_default()
                    .push(c.name.clone());
                r.push(
                    false,
                    &t.name,
                    Some(&c.name),
                    "conform/optional-missing",
                    "optional column not mapped: questions that read it are off".into(),
                );
            } else {
                r.push(
                    true,
                    &t.name,
                    Some(&c.name),
                    "conform/missing-column",
                    format!("required column missing (the contract has {ctype})"),
                );
            }
            continue;
        };
        if !compatible(ctype, vtype) {
            r.push(
                true,
                &t.name,
                Some(&c.name),
                "conform/type",
                format!("the view gives {vtype}, the contract wants {ctype}"),
            );
            continue;
        }
        let col = format!("{}.{}", qi(&t.name), qi(&c.name));
        if *not_null {
            let n = one(
                duck,
                &format!(
                    "SELECT count(*)::VARCHAR FROM main.{} WHERE {} IS NULL",
                    qi(&t.name),
                    qi(&c.name)
                ),
            )?;
            if n.as_deref().is_some_and(|n| n != "0") {
                r.push(
                    true,
                    &t.name,
                    Some(&c.name),
                    "conform/null",
                    format!(
                        "{} rows are NULL in a NOT NULL column",
                        n.unwrap_or_default()
                    ),
                );
            }
        }
        if let Some(scope) = &c.scope {
            check_scope(duck, mapping, opts, &tree, t, c, scope, &col, r)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn check_scope(
    duck: &mut dyn Duck,
    mapping: &Mapping,
    opts: &Options,
    tree: &Value,
    t: &Table,
    c: &Column,
    scope: &str,
    col: &str,
    r: &mut Report,
) -> Result<(), String> {
    let constant = mapping.constant_scopes.contains(scope);
    if constant {
        r.push(
            false,
            &t.name,
            Some(&c.name),
            "conform/scope-constant",
            format!(
                "scope '{scope}' is marked constant for this customer (-- @scope {scope} constant)"
            ),
        );
    } else if !scope_from_source(tree, &c.name) {
        r.push(true, &t.name, Some(&c.name), "conform/scope-not-source", format!(
            "scope column '{scope}' must come from a source.* column, not a literal or an expression without one — or mark it '-- @scope {scope} constant' when it is one value for this customer"
        ));
        return Ok(());
    }
    let distinct: Vec<String> = rows(duck, &format!("SELECT DISTINCT {col}::VARCHAR FROM main.{} WHERE {col} IS NOT NULL ORDER BY 1 LIMIT 1001", qi(&t.name)))?
        .into_iter()
        .filter_map(|r| r.into_iter().next().flatten())
        .collect();
    let Some(allowed) = opts.scope_values.get(scope) else {
        if !constant {
            r.push(false, &t.name, Some(&c.name), "conform/scope-unchecked", format!("{} distinct value(s); give --scope {scope}=<values> to check them against the operator's list", distinct.len()));
        }
        return Ok(());
    };
    let outside: Vec<&String> = distinct.iter().filter(|d| !allowed.contains(d)).collect();
    if !outside.is_empty() {
        let shown: Vec<String> = outside
            .iter()
            .take(5)
            .map(|s| crate::host::access::diag(s))
            .collect();
        r.push(
            true,
            &t.name,
            Some(&c.name),
            "conform/scope-values",
            format!(
                "{} value(s) outside the operator's list for '{scope}', e.g. {}",
                outside.len(),
                shown.join(", ")
            ),
        );
    }
    if !constant && allowed.len() > 1 && distinct.len() < 2 {
        r.push(true, &t.name, Some(&c.name), "conform/scope-one-value", format!(
            "scope '{scope}' has {} distinct value(s) although the operator lists {}: a mapping that fills it with one value would make the policy on it meaningless (mark it constant if that is right)",
            distinct.len(), allowed.len()
        ));
    }
    Ok(())
}

/// Primary keys unique and references resolving, as counts (fast on big data).
fn check_keys(duck: &mut dyn Duck, contract: &Contract, r: &mut Report) -> Result<(), String> {
    let cons = rows(
        duck,
        &format!(
            "SELECT lower(table_name), constraint_type, lower(constraint_column_names::VARCHAR), \
             lower(coalesce(referenced_table, '')), lower(coalesce(referenced_column_names, [])::VARCHAR) \
             FROM duckdb_constraints() WHERE database_name = 'memory' AND schema_name = {} \
             AND constraint_type IN ('PRIMARY KEY', 'FOREIGN KEY')",
            lit(CONTRACT_SCHEMA)
        ),
    )?;
    let list = |s: &Option<String>| -> Vec<String> {
        s.as_deref()
            .unwrap_or("")
            .trim_matches(['[', ']'])
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect()
    };
    let present = |duck: &mut dyn Duck, t: &str| -> bool {
        one(duck, &format!("SELECT count(*)::VARCHAR FROM duckdb_views() WHERE database_name = 'memory' AND schema_name = 'main' AND view_name = {}", lit(t)))
            .ok()
            .flatten()
            .as_deref()
            != Some("0")
    };
    for c in cons {
        let (Some(table), Some(kind)) = (c[0].clone(), c[1].clone()) else {
            continue;
        };
        if contract.table(&table).is_none() || !present(duck, &table) {
            continue;
        }
        let cols: Vec<String> = list(&c[2]).iter().map(|x| qi(x)).collect();
        if cols.is_empty() {
            continue;
        }
        let k = cols.join(", ");
        if kind == "PRIMARY KEY" {
            let dup = one(duck, &format!("SELECT count(*)::VARCHAR FROM (SELECT {k} FROM main.{} GROUP BY ALL HAVING count(*) > 1)", qi(&table)));
            match dup {
                Ok(Some(n)) if n != "0" => r.push(
                    true,
                    &table,
                    None,
                    "conform/key-duplicate",
                    format!(
                        "{n} duplicated key value(s) of ({})",
                        list(&c[2]).join(", ")
                    ),
                ),
                Ok(_) => {}
                Err(e) => r.push(true, &table, None, "conform/error", e),
            }
        } else {
            let (Some(to), refs) = (c[3].clone(), list(&c[4])) else {
                continue;
            };
            if to.is_empty() || refs.len() != cols.len() || !present(duck, &to) {
                continue;
            }
            let on: Vec<String> = cols
                .iter()
                .zip(&refs)
                .map(|(a, b)| format!("f.{a} = t.{}", qi(b)))
                .collect();
            let not_null: Vec<String> = cols.iter().map(|a| format!("f.{a} IS NOT NULL")).collect();
            let q = format!(
                "SELECT count(*)::VARCHAR FROM main.{} f WHERE {} AND NOT EXISTS (SELECT 1 FROM main.{} t WHERE {})",
                qi(&table), not_null.join(" AND "), qi(&to), on.join(" AND ")
            );
            match one(duck, &q) {
                Ok(Some(n)) if n != "0" => r.push(
                    true,
                    &table,
                    None,
                    "conform/reference",
                    format!("{n} row(s) reference a missing {to} ({})", refs.join(", ")),
                ),
                Ok(_) => {}
                Err(e) => r.push(true, &table, None, "conform/error", e),
            }
        }
    }
    Ok(())
}

/// One declared question (`-- @question`) and whether this customer's
/// mapping can answer it (plan §4, L6).
#[derive(Debug, Clone, PartialEq)]
pub struct Availability {
    pub id: String,
    pub text: String,
    /// `None`: answerable; `Some(why)`: switched off for this customer.
    pub off: Option<String>,
}

/// Which declared questions of `script` this customer can answer, in the
/// build instance after [`conform`] created the views: the dashboard's
/// setup statements run there, then each question's panel is bound
/// (`DESCRIBE`, nothing runs) — a panel that reads a column or table the
/// mapping doesn't have fails to bind and its question is off.
pub fn questions(duck: &mut dyn Duck, script: &str) -> Vec<Availability> {
    let plan = crate::sql::plan(script);
    let mut setup_err: Option<String> = None;
    for p in plan.iter().filter(|p| p.setup) {
        if let Err(e) = rows(duck, &p.sql) {
            setup_err.get_or_insert(first_line(&e));
        }
    }
    crate::sql::question::declared(script)
        .into_iter()
        .map(|q| {
            let off = match plan.get(q.stmt) {
                None => Some("no panel answers it".to_string()),
                Some(p) => rows(
                    duck,
                    &format!("DESCRIBE {}", p.sql.trim().trim_end_matches(';')),
                )
                .err()
                .map(|e| {
                    let e = first_line(&e);
                    match &setup_err {
                        Some(s) => format!("{e} (a setup statement failed first: {s})"),
                        None => e,
                    }
                }),
            };
            Availability {
                id: q.id,
                text: q.text,
                off,
            }
        })
        .collect()
}

fn first_line(e: &str) -> String {
    e.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(e)
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTRACT: &str = "
-- Suppliers and their deliveries.
CREATE TABLE suppliers (
    supplier_id VARCHAR PRIMARY KEY, -- the supplier's code
    name VARCHAR NOT NULL,
    tenant VARCHAR NOT NULL, -- @scope tenant
    rating DOUBLE -- @optional the buyer's rating
);
CREATE TABLE deliveries (
    supplier_id VARCHAR NOT NULL REFERENCES suppliers (supplier_id),
    due DATE NOT NULL,
    days_late INTEGER,
    tenant VARCHAR NOT NULL -- @scope tenant
);";

    #[test]
    fn a_contract_parses_with_its_markers() {
        let c = parse_contract(CONTRACT).unwrap();
        assert_eq!(c.tables.len(), 2);
        let s = &c.tables[0];
        assert_eq!(s.name, "suppliers");
        let names: Vec<_> = s.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["supplier_id", "name", "tenant", "rating"]);
        assert_eq!(s.columns[0].doc, "the supplier's code");
        assert_eq!(s.columns[2].scope.as_deref(), Some("tenant"));
        assert!(s.columns[3].optional && s.columns[3].doc == "the buyer's rating");
        assert_eq!(c.tables[1].columns[3].scope.as_deref(), Some("tenant"));
        for bad in [
            "CREATE VIEW v AS SELECT 1",
            "CREATE TABLE a (x INT); CREATE TABLE a (y INT)",
            "INSERT INTO t VALUES (1)",
            "CREATE TABLE a (x INT -- @scope t constant\n)",
            "",
        ] {
            assert!(parse_contract(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_mapping_holds_one_view_per_contract_table_only() {
        let c = parse_contract(CONTRACT).unwrap();
        let ok = "-- @scope tenant constant\nCREATE VIEW suppliers AS SELECT * FROM source.s;\nCREATE OR REPLACE VIEW deliveries AS SELECT * FROM source.d;";
        let m = parse_mapping(ok, &c).unwrap();
        assert_eq!(m.views.len(), 2);
        assert_eq!(m.views[1].query, "SELECT * FROM source.d");
        assert!(m.constant_scopes.contains("tenant"));
        let errs = |t: &str| parse_mapping(t, &c).unwrap_err().join("\n");
        assert!(errs("CREATE VIEW suppliers AS SELECT 1;").contains("'deliveries' is not mapped"));
        assert!(errs("CREATE VIEW helper AS SELECT 1; CREATE VIEW suppliers AS FROM helper; CREATE VIEW deliveries AS SELECT 1;").contains("not a contract table"));
        assert!(
            errs("CREATE TABLE suppliers AS SELECT 1; CREATE VIEW deliveries AS SELECT 1;")
                .contains("only CREATE")
        );
        assert!(errs(
            "COPY x TO 'y'; CREATE VIEW suppliers AS SELECT 1; CREATE VIEW deliveries AS SELECT 1;"
        )
        .contains("only CREATE"));
        assert!(
            errs("CREATE VIEW suppliers(a) AS SELECT 1; CREATE VIEW deliveries AS SELECT 1;")
                .contains("no column list")
        );
        assert!(errs("CREATE VIEW suppliers AS SELECT 1; CREATE VIEW suppliers AS SELECT 2; CREATE VIEW deliveries AS SELECT 1;").contains("twice"));
        assert!(errs("-- @scope region constant\nCREATE VIEW suppliers AS SELECT 1; CREATE VIEW deliveries AS SELECT 1;").contains("no scope column 'region'"));
    }

    #[test]
    fn types_compare_by_family() {
        assert!(compatible("DOUBLE", "BIGINT"));
        assert!(compatible("DECIMAL(18,3)", "DOUBLE"));
        assert!(compatible("VARCHAR", "VARCHAR"));
        assert!(compatible("TIMESTAMP WITH TIME ZONE", "TIMESTAMP"));
        assert!(!compatible("INTEGER", "VARCHAR"));
        assert!(!compatible("DATE", "TIMESTAMP"));
        assert!(!compatible("INTEGER", "DOUBLE"));
    }
}
