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
        let mut comma_end = 0;
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
                comma_end = t.end;
                continue;
            }
            // a comment right after a comma, on the comma's line, belongs to
            // the item before it; one on a line of its own to the next (#984)
            if t.kind == TokKind::LineComment
                && items.last().is_some_and(|i| i.is_empty())
                && !stmt[comma_end..t.start].contains('\n')
            {
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

/// The statements that set up a build instance on a fresh DuckDB:
/// the customer's database attached as `source`, read-only, then external
/// access off and the configuration locked (H1).
/// `memory_limit` (a DuckDB size, `4GB`) and `threads` bound what a
/// customer's mapping can take (#980) — set before the lock, so nothing in
/// the mapping can raise them.
pub fn instance_setup(source_db: &str, memory_limit: &str, threads: u32) -> Vec<String> {
    vec![
        format!(
            "ATTACH {} AS {} (READ_ONLY)",
            lit(source_db),
            policy::MAPPING_SOURCE
        ),
        format!("SET memory_limit = {}", lit(memory_limit)),
        format!("SET threads = {}", threads.max(1)),
        "SET enable_external_access = false".into(),
        "SET autoinstall_known_extensions = false".into(),
        "SET autoload_known_extensions = false".into(),
        "SET lock_configuration = true".into(),
    ]
}

// ---- the scope-column check (R2): does a view's column come from source? ----
//
// Fail closed (#978): a column counts as from source only when it is a plain
// column reference that resolves — through CTEs, subqueries, joins, a plain
// `*` and set operations matched by position — to a real column of a
// `source.*` table (`columns`, from the build instance's catalog). Anything
// this resolver doesn't fully understand (a `*` with REPLACE / RENAME /
// COLUMNS, a scalar subquery, a lambda, a 3+-part name, a positional
// rename, a table function, an unknown node) is a constant.

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

/// The deepest nesting the resolver follows before it gives up (fail closed).
const MAX_DEPTH: usize = 64;

/// The most steps one column's resolution takes (#986: a CTE body is walked
/// again for every reference, so a chain of CTEs that each read the one
/// before several times grows exponentially): past it, fail closed.
const MAX_VISITS: usize = 100_000;

/// CTE frames, innermost last: (name, its query node).
type Frames<'a> = Vec<Vec<(String, &'a Value)>>;

struct Resolver<'a> {
    ctes: Frames<'a>,
    /// `source` table as `schema.table` (lower case) → its columns.
    columns: &'a BTreeMap<String, BTreeSet<String>>,
    depth: usize,
    visits: usize,
}

fn st<'v>(v: &'v Value, k: &str) -> &'v str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// A select-list entry's output name: its alias, else a bare column's name.
fn out_name(e: &Value) -> Option<String> {
    let a = st(e, "alias");
    if !a.is_empty() {
        return Some(a.to_ascii_lowercase());
    }
    if st(e, "type") == "COLUMN_REF" {
        return e
            .get("column_names")
            .and_then(Value::as_array)
            .and_then(|a| a.last())
            .and_then(Value::as_str)
            .map(str::to_ascii_lowercase);
    }
    None
}

fn is_star(e: &Value) -> bool {
    st(e, "class") == "STAR"
}

/// A `*` that only passes columns through (no REPLACE / RENAME / COLUMNS(…)
/// / unpacking — an EXCLUDE only drops columns, see [`star_excludes`]).
fn plain_star(e: &Value) -> bool {
    let empty = |k: &str| match e.get(k) {
        None | Some(Value::Null) => true,
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
        Some(Value::Bool(b)) => !b,
        _ => false,
    };
    is_star(e)
        && empty("replace_list")
        && empty("rename_list")
        && empty("columns")
        && empty("expr")
        && empty("unpacked")
}

/// What `*`'s EXCLUDE does to `col` (#984): `Some(true)` the star doesn't
/// produce it, `Some(false)` an EXCLUDE qualified with a relation names it
/// (which relation's copy is left is not followed: fail closed), `None`
/// not excluded.
fn star_excludes(e: &Value, col: &str) -> Option<bool> {
    let plain = e
        .get("exclude_list")
        .and_then(Value::as_array)
        .is_some_and(|a| {
            a.iter()
                .any(|x| x.as_str().is_some_and(|x| x.eq_ignore_ascii_case(col)))
        });
    if plain {
        return Some(true);
    }
    let qualified = e
        .get("qualified_exclude_list")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|x| st(x, "column").eq_ignore_ascii_case(col)));
    qualified.then_some(false)
}

impl<'a> Resolver<'a> {
    /// The CTE `name` visible here, and the scope its own body sees: the
    /// frames outside it and the CTEs written before it in its WITH (a
    /// non-recursive CTE doesn't see itself — DuckDB binds the outer one).
    fn cte(&self, name: &str) -> Option<(&'a Value, Frames<'a>)> {
        for (k, frame) in self.ctes.iter().enumerate().rev() {
            if let Some(pos) = frame
                .iter()
                .rposition(|(n, _)| n.eq_ignore_ascii_case(name))
            {
                let mut scope: Frames<'a> = self.ctes[..k].to_vec();
                scope.push(frame[..pos].to_vec());
                return Some((frame[pos].1, scope));
            }
        }
        None
    }

    fn deeper<T>(&mut self, fallback: T, f: impl FnOnce(&mut Self) -> T) -> T {
        if self.depth >= MAX_DEPTH || self.visits >= MAX_VISITS {
            return fallback;
        }
        self.visits += 1;
        self.depth += 1;
        let r = f(self);
        self.depth -= 1;
        r
    }

    /// Column `col` (lower case) of query node `node`, by name.
    fn node_column(&mut self, node: &'a Value, col: &str) -> Origin {
        self.deeper(Origin::Constant, |me| match st(node, "type") {
            "SELECT_NODE" => {
                let pushed = me.push_ctes(node);
                let out = me.select_column(node, col);
                if pushed {
                    me.ctes.pop();
                }
                out
            }
            "SET_OPERATION_NODE" => {
                let by_name = st(node, "setop_type").contains("BY_NAME");
                let (Some(l), Some(r)) = (node.get("left"), node.get("right")) else {
                    return Origin::Constant;
                };
                let rs = if by_name {
                    vec![me.node_column(l, col), me.node_column(r, col)]
                } else {
                    // by position: the left branch names the output column
                    let Some(i) = me.position_of(l, col) else {
                        return Origin::Constant;
                    };
                    vec![me.column_at(l, i), me.column_at(r, i)]
                };
                if rs.iter().all(|r| *r == Origin::Source) {
                    Origin::Source
                } else if rs.contains(&Origin::Absent) && !rs.contains(&Origin::Constant) {
                    Origin::Absent
                } else {
                    Origin::Constant
                }
            }
            // a shape this check doesn't know: fail closed
            _ => Origin::Constant,
        })
    }

    /// The output position of `col` in `node` (no `*` before it allowed).
    fn position_of(&mut self, node: &'a Value, col: &str) -> Option<usize> {
        match st(node, "type") {
            "SELECT_NODE" => {
                let list = node.get("select_list")?.as_array()?;
                for (i, e) in list.iter().enumerate() {
                    if is_star(e) {
                        return None;
                    }
                    if out_name(e).as_deref() == Some(col) {
                        return Some(i);
                    }
                }
                None
            }
            "SET_OPERATION_NODE" if !st(node, "setop_type").contains("BY_NAME") => {
                self.deeper(None, |me| me.position_of(node.get("left")?, col))
            }
            _ => None,
        }
    }

    /// The output column at position `i` of `node`.
    fn column_at(&mut self, node: &'a Value, i: usize) -> Origin {
        self.deeper(Origin::Constant, |me| match st(node, "type") {
            "SELECT_NODE" => {
                let list = node
                    .get("select_list")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                if list.iter().any(is_star) {
                    return Origin::Constant;
                }
                let Some(e) = list.get(i) else {
                    return Origin::Constant;
                };
                let pushed = me.push_ctes(node);
                let out = me.expr(e, node.get("from_table"));
                if pushed {
                    me.ctes.pop();
                }
                out
            }
            "SET_OPERATION_NODE" if !st(node, "setop_type").contains("BY_NAME") => {
                let (Some(l), Some(r)) = (node.get("left"), node.get("right")) else {
                    return Origin::Constant;
                };
                if me.column_at(l, i) == Origin::Source && me.column_at(r, i) == Origin::Source {
                    Origin::Source
                } else {
                    Origin::Constant
                }
            }
            _ => Origin::Constant,
        })
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
        // DuckDB names output columns left to right; a `*` before a named
        // entry may already produce `col` — the first match wins
        for e in list {
            if is_star(e) {
                if !plain_star(e) {
                    return Origin::Constant;
                }
                match star_excludes(e, col) {
                    Some(true) => continue,
                    Some(false) => return Origin::Constant,
                    None => {}
                }
                let qual = st(e, "relation_name");
                let r = match from {
                    Some(f) => self.of_column(f, (!qual.is_empty()).then_some(qual), col),
                    None => Origin::Absent,
                };
                if r != Origin::Absent {
                    return r;
                }
            } else if out_name(e).as_deref() == Some(col) {
                return self.expr(e, from);
            }
        }
        Origin::Absent
    }

    /// An expression: from source only when it is one plain column
    /// reference that resolves to a source column (a CAST of one too).
    fn expr(&mut self, e: &'a Value, from: Option<&'a Value>) -> Origin {
        let mut e = e;
        // `col::TYPE` keeps the column's values
        while st(e, "class") == "CAST" {
            match e.get("child") {
                Some(c) => e = c,
                None => return Origin::Constant,
            }
        }
        if st(e, "type") != "COLUMN_REF" {
            return Origin::Constant;
        }
        let parts: Vec<String> = e
            .get("column_names")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_ascii_lowercase)
                    .collect()
            })
            .unwrap_or_default();
        let (qual, name) = match parts.as_slice() {
            [c] => (None, c.as_str()),
            [q, c] => (Some(q.as_str()), c.as_str()),
            // a 3+-part name (or none): fail closed
            _ => return Origin::Constant,
        };
        match from {
            Some(f) => match self.of_column(f, qual, name) {
                Origin::Source => Origin::Source,
                _ => Origin::Constant,
            },
            None => Origin::Constant,
        }
    }

    /// Column `col` (of relation `qual`, if named) in a FROM item.
    fn of_column(&mut self, f: &'a Value, qual: Option<&str>, col: &str) -> Origin {
        if col.starts_with('#') {
            return Origin::Constant;
        }
        self.deeper(Origin::Constant, |me| match st(f, "type") {
            "JOIN" => {
                let rs: Vec<Origin> = [f.get("left"), f.get("right")]
                    .into_iter()
                    .flatten()
                    .map(|x| me.of_column(x, qual, col))
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
                // a column list (`AS t(a, b)`) renames by position: fail closed
                if f.get("column_name_alias")
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty())
                {
                    return Origin::Constant;
                }
                if cat.is_empty() && sch.is_empty() {
                    if let Some((n, scope)) = me.cte(name) {
                        let saved = std::mem::replace(&mut me.ctes, scope);
                        let out = me.node_column(n, col);
                        me.ctes = saved;
                        return out;
                    }
                }
                let source = cat.eq_ignore_ascii_case(policy::MAPPING_SOURCE)
                    || (cat.is_empty() && sch.eq_ignore_ascii_case(policy::MAPPING_SOURCE));
                if !source {
                    return Origin::Constant;
                }
                // a real column of that source table (by schema, #984), or
                // not there at all
                let schema = if cat.is_empty() || sch.is_empty() {
                    "main"
                } else {
                    sch
                };
                let key = format!("{schema}.{name}").to_ascii_lowercase();
                match me.columns.get(&key) {
                    Some(cols) if cols.contains(col) => Origin::Source,
                    Some(_) => Origin::Absent,
                    None => Origin::Constant,
                }
            }
            "SUBQUERY" => {
                if qual.is_some_and(|q| !q.eq_ignore_ascii_case(st(f, "alias"))) {
                    return Origin::Absent;
                }
                if f.get("column_name_alias")
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty())
                {
                    return Origin::Constant;
                }
                match f.get("subquery").and_then(|s| s.get("node")) {
                    Some(n) => me.node_column(n, col),
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
        })
    }
}

/// Whether `col` of the mapping query (its parse tree) comes from a column
/// of a `source.*` table (`columns`: source table → its columns, lower case).
fn scope_from_source(
    tree: &Value,
    col: &str,
    columns: &BTreeMap<String, BTreeSet<String>>,
) -> bool {
    let Some(node) = tree
        .get("statements")
        .and_then(|s| s.get(0))
        .and_then(|s| s.get("node"))
    else {
        return false;
    };
    Resolver {
        ctes: Vec::new(),
        columns,
        depth: 0,
        visits: 0,
    }
    .node_column(node, &col.to_ascii_lowercase())
        == Origin::Source
}

/// The source database's tables (`schema.table`) and their columns (lower
/// case).
fn source_columns(duck: &mut dyn Duck) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let mut m: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in rows(
        duck,
        &format!(
            "SELECT lower(schema_name || '.' || table_name), lower(column_name) FROM duckdb_columns() WHERE database_name = {}",
            lit(policy::MAPPING_SOURCE)
        ),
    )? {
        if let [Some(t), Some(c)] = r.as_slice() {
            m.entry(t.clone()).or_default().insert(c.clone());
        }
    }
    Ok(m)
}

// ---- the report ----

/// One scope's allowed values as an operator wrote them (`--scope
/// name=a,b`, `agent.mapping.scope`): the name trimmed and lower-cased, the
/// values trimmed with blanks dropped; an empty name or no value at all is
/// an error (#984, #988). The CLI and the server parse alike.
pub fn scope_entry(name: &str, values: &[&str]) -> Result<(String, Vec<String>), String> {
    let name = name.trim().to_ascii_lowercase();
    if name.is_empty() {
        return Err("a scope needs a name".into());
    }
    let vals: Vec<String> = values
        .iter()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect();
    if vals.is_empty() {
        return Err(format!("scope '{name}' needs at least one value"));
    }
    Ok((name, vals))
}

/// The scope names `scope_values` gives that `contract` doesn't declare
/// (#988: a misspelt one would otherwise switch its check off unnoticed).
pub fn unknown_scopes(
    contract: &Contract,
    scope_values: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    scope_values
        .keys()
        .filter(|k| {
            !contract.tables.iter().flat_map(|t| &t.columns).any(|c| {
                c.scope
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(k))
            })
        })
        .cloned()
        .collect()
}

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

    pub fn push(
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
    // a list or array of anything is no scalar family (#984)
    if t.contains('[') {
        return "other";
    }
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
    // #979: types outside the known families (lists, BLOB, UUID, STRUCT,
    // MAP, …) only match exactly
    if c == "other" || v == "other" {
        let norm = |t: &str| {
            t.split_whitespace()
                .collect::<String>()
                .to_ascii_uppercase()
        };
        return norm(contract) == norm(view);
    }
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
             WHERE database_name = 'memory' AND schema_name = {} AND lower(table_name) = {}",
            lit(CONTRACT_SCHEMA),
            lit(&t.name)
        ),
    )?
    .into_iter()
    .filter_map(|r| Some((r[0].clone()?, (r[1].clone()?, r[2].as_deref() == Some("true")))))
    .collect();
    // #979: a declared table whose columns can't be read checks nothing —
    // never a silent pass
    if want.is_empty() {
        r.push(
            true,
            &t.name,
            None,
            "conform/error",
            "the contract table's columns could not be read".into(),
        );
        return Ok(());
    }
    let have: BTreeMap<String, String> = rows(
        duck,
        &format!(
            "SELECT lower(column_name), data_type FROM duckdb_columns() \
             WHERE database_name = 'memory' AND schema_name = 'main' AND lower(table_name) = {}",
            lit(&t.name)
        ),
    )?
    .into_iter()
    .filter_map(|r| Some((r[0].clone()?, r[1].clone()?)))
    .collect();
    let tree = match policy::parse_tree(duck, &v.query) {
        Ok(t) => t,
        Err(e) => {
            r.push(true, &t.name, None, "conform/error", e);
            return Ok(());
        }
    };
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
            let n = match one(
                duck,
                &format!(
                    "SELECT count(*)::VARCHAR FROM main.{} WHERE {} IS NULL",
                    qi(&t.name),
                    qi(&c.name)
                ),
            ) {
                Ok(n) => n,
                // #979: a view that fails when read is a finding, not a usage error
                Err(e) => {
                    r.push(true, &t.name, Some(&c.name), "conform/error", e);
                    continue;
                }
            };
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
    } else if !scope_from_source(tree, &c.name, &source_columns(duck)?) {
        r.push(true, &t.name, Some(&c.name), "conform/scope-not-source", format!(
            "scope column '{scope}' must come from a source.* column, not a literal or an expression without one — or mark it '-- @scope {scope} constant' when it is one value for this customer"
        ));
        return Ok(());
    }
    // #979: counted in SQL, not over a capped list of values
    let count = |duck: &mut dyn Duck, extra: &str| -> Result<usize, String> {
        Ok(one(
            duck,
            &format!(
                "SELECT count(DISTINCT {col})::VARCHAR FROM main.{} WHERE {col} IS NOT NULL{extra}",
                qi(&t.name)
            ),
        )?
        .and_then(|n| n.parse().ok())
        .unwrap_or(0))
    };
    let n_distinct = match count(duck, "") {
        Ok(n) => n,
        Err(e) => {
            r.push(true, &t.name, Some(&c.name), "conform/error", e);
            return Ok(());
        }
    };
    let Some(allowed) = opts.scope_values.get(scope) else {
        if !constant {
            r.push(false, &t.name, Some(&c.name), "conform/scope-unchecked", format!("{n_distinct} distinct value(s); give --scope {scope}=<values> to check them against the operator's list"));
        }
        return Ok(());
    };
    // #984: an empty list allows nothing (`NOT IN (NULL)` would allow all)
    let not_in = if allowed.is_empty() {
        " AND true".to_string()
    } else {
        format!(
            " AND {col}::VARCHAR NOT IN ({})",
            allowed
                .iter()
                .map(|a| lit(a))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let n_outside = match count(duck, &not_in) {
        Ok(n) => n,
        Err(e) => {
            r.push(true, &t.name, Some(&c.name), "conform/error", e);
            return Ok(());
        }
    };
    if n_outside > 0 {
        let shown: Vec<String> = rows(duck, &format!("SELECT DISTINCT {col}::VARCHAR FROM main.{} WHERE {col} IS NOT NULL{not_in} ORDER BY 1 LIMIT 5", qi(&t.name)))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| r.into_iter().next().flatten())
            .map(|s| crate::host::access::diag(&s))
            .collect();
        r.push(
            true,
            &t.name,
            Some(&c.name),
            "conform/scope-values",
            format!(
                "{n_outside} value(s) outside the operator's list for '{scope}', e.g. {}",
                shown.join(", ")
            ),
        );
    }
    if !constant && allowed.len() > 1 && n_distinct < 2 {
        r.push(true, &t.name, Some(&c.name), "conform/scope-one-value", format!(
            "scope '{scope}' has {} distinct value(s) although the operator lists {}: a mapping that fills it with one value would make the policy on it meaningless (mark it constant if that is right)",
            n_distinct, allowed.len()
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
    for p in &plan {
        if p.setup {
            if let Err(e) = rows(duck, &p.sql) {
                setup_err.get_or_insert(first_line(&e));
            }
        } else if let Some(kind) = crate::inputs::input_kind(&p.roles) {
            // #985: an input's default is set before the panels bind, as
            // `dashboard --check` and the agent do — a panel like
            // `COLUMNS(getvariable('dim'))` doesn't bind with it NULL
            for set in input_defaults(duck, kind, p) {
                let _ = rows(duck, &set);
            }
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

/// The `SET VARIABLE`s an input's default makes, from its option query's
/// rows (named and typed by `DESCRIBE`; at most 1000 rows).
fn input_defaults(
    duck: &mut dyn Duck,
    kind: crate::InputKind,
    p: &crate::sql::Panel,
) -> Vec<String> {
    let q = p.sql.trim().trim_end_matches(';');
    let Ok(cols) = rows(duck, &format!("DESCRIBE {q}")) else {
        return Vec::new();
    };
    let Ok(data) = rows(duck, &format!("SELECT * FROM ({q}) LIMIT 1000")) else {
        return Vec::new();
    };
    let cols: Vec<(String, &'static str)> = cols
        .iter()
        .map(|c| {
            let name = c.first().cloned().flatten().unwrap_or_default();
            let ty = c.get(1).cloned().flatten().unwrap_or_default();
            (
                name,
                if ty == "BOOLEAN" {
                    "boolean"
                } else {
                    family(&ty)
                },
            )
        })
        .collect();
    let maps: Vec<serde_json::Map<String, Value>> = data
        .into_iter()
        .map(|r| {
            cols.iter()
                .zip(r)
                .map(|((n, fam), v)| {
                    let v = match (v, *fam) {
                        (None, _) => Value::Null,
                        (Some(v), "integer" | "number") => v
                            .parse::<serde_json::Number>()
                            .map(Value::Number)
                            .unwrap_or(Value::String(v)),
                        (Some(v), "boolean") => Value::Bool(v == "true"),
                        (Some(v), _) => Value::String(v),
                    };
                    (n.clone(), v)
                })
                .collect()
        })
        .collect();
    crate::inputs::input_defaults_with(
        kind,
        crate::inputs::input_index(&p.roles),
        crate::inputs::default_index(&p.roles),
        &maps,
    )
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
        // #979: outside the known families, only the exact type
        assert!(!compatible("INTEGER[]", "VARCHAR[]"));
        assert!(!compatible("BLOB", "STRUCT(a INTEGER)"));
        assert!(!compatible("UUID", "VARCHAR"));
        assert!(compatible("INTEGER[]", "INTEGER[]"));
        // #984: a list of a scalar family is no scalar
        assert!(!compatible("DECIMAL(18,3)", "DECIMAL(18,3)[]"));
        assert!(!compatible("TIMESTAMP", "TIMESTAMP[]"));
        assert!(!compatible("TIME", "TIME[3]"));
        assert!(compatible("DECIMAL(18,3)[]", "DECIMAL(18,3)[]"));
    }

    #[test]
    fn a_marker_on_its_own_line_belongs_to_the_column_below() {
        // #984: a comment after a comma on the same line is the previous
        // column's; on a line of its own, the next one's
        let c = parse_contract(
            "CREATE TABLE t (\n    a INTEGER, -- the a\n    -- @scope tenant\n    tenant VARCHAR NOT NULL,\n    -- @optional\n    b DOUBLE\n);",
        )
        .unwrap();
        let cols = &c.tables[0].columns;
        assert_eq!(cols[0].doc, "the a");
        assert_eq!(cols[0].scope, None);
        assert_eq!(cols[1].scope.as_deref(), Some("tenant"));
        assert!(!cols[0].optional && !cols[1].optional && cols[2].optional);
    }
}
