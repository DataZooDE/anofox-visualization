//! Scenarios with anofox-scenario (#1243, plan: docs/plans/plan-scenarios.md),
//! slice S1: **read-only**. The locked server attaches every scenario of
//! `options.scenarios.database` to its snapshot (`scn_<id>`, read-only, before
//! the lockdown), and each request connection gets three helpers as `TEMP`
//! macros:
//!
//! * `anofox_scenarios()` — `main` and every attached scenario
//!   (`name, mode, frozen, parent, created_at, description, merged_at`);
//! * `anofox_scenario_rows(table)` — the table over `main` and every branch,
//!   with a leading `scenario` column (`UNION ALL BY NAME`);
//! * `anofox_scenario_diff(scenario, table [, against := 'main'])` —
//!   anofox-scenario's `scenario_diff`.
//!
//! The helpers read only the tables listed in `options.scenarios.tables`; any
//! other name is an error. A `::SCENARIO` picker (a dropdown) chooses the
//! branch a request reads: the server sets that request's `search_path` to
//! the scenario's catalog, so unqualified names read the branch.
//!
//! Slice S2, **writes**: with `writers` in `options.scenarios`, a `::SAVE`
//! with `::SAVE_SCENARIO` creates a scenario (`::SAVE_ACTION 'create'`) or
//! writes a `::FORM`'s row into one, on the server's own connection to the
//! live database — values bound, names checked — and logs it ([`WriteOptions`],
//! [`create_sql`], [`row_sql`]).
//!
//! Pure: the server runs the SQL this module writes.

use crate::host::serving::{quote_ident, sql_string_literal, valid_dashboard_id, valid_ident};
use crate::host::writeback::{self as wb, ColType, TableDecl};
use crate::sql::lex::{self, TokKind};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// At most this many scenarios are attached by default (each is one more
/// catalog in the snapshot).
pub const DEFAULT_MAX_SCENARIOS: usize = 50;

/// anofox-scenario's functions that change a scenario or a table. On a host
/// attached read-only they still change what the session reads
/// (DataZooDE/anofox-scenario#66), and the snapshot is one long-lived
/// session, so a dashboard may not call them.
pub const WRITE_FUNCTIONS: &[&str] = &[
    "scenario_create",
    "scenario_drop",
    "scenario_freeze",
    "scenario_unfreeze",
    "scenario_merge",
    "scenario_migrate",
    "scenario_refresh",
];

/// The schema anofox-scenario keeps its registry and deltas in.
pub const INTERNAL_SCHEMA: &str = "__anofox_scenario";

/// Parsed `options.scenarios` (S1: what the read side needs).
#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioOptions {
    /// The attached DuckDB database whose scenarios are served.
    pub database: String,
    /// The tables the helpers may read, in `main` of `database`.
    pub tables: Vec<String>,
    /// At most this many scenarios (by id) are attached, and created.
    pub max_scenarios: usize,
    /// S2: who may create scenarios and write into them (`None`: read-only).
    pub write: Option<WriteOptions>,
}

/// How `create` makes a scenario (anofox-scenario's `mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Cheap: only the changes are stored; later changes to `main` show.
    Delta,
    /// A full copy: isolated from later changes to `main`.
    Materialized,
}

impl Mode {
    pub fn sql(self) -> &'static str {
        match self {
            Mode::Delta => "delta",
            Mode::Materialized => "materialized",
        }
    }
}

/// The write side of `options.scenarios` (S2).
#[derive(Debug, Clone, PartialEq)]
pub struct WriteOptions {
    /// The change log: one `fdatasync`'d line per operation.
    pub path: PathBuf,
    /// The dashboards whose `::SAVE`s may write scenarios.
    pub dashboards: Vec<String>,
    pub writer_groups: Vec<String>,
    pub writer_users: Vec<String>,
    /// The only tables (and columns) a save may write, with their key.
    pub tables: BTreeMap<String, TableDecl>,
    pub mode: Mode,
    pub max_rows_per_save: usize,
    pub text_max_chars: usize,
    pub rate_per_minute: u32,
    pub rate_burst: u32,
    pub max_mb: u64,
}

impl WriteOptions {
    /// May this signed-in user write? (The caller has already refused API
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

    /// The start-time refusals that need the rest of the server's config
    /// (as write-back's: personal sign-ins, a framed button, the log's path).
    pub fn validate_start(
        &self,
        ctx: &wb::StartContext,
        writeback_path: Option<&std::path::Path>,
    ) -> Result<(), String> {
        if !ctx.has_auth {
            return Err(
                "options.scenarios.writers needs options.auth: every write carries a \
                        signed-in writer's identity"
                    .into(),
            );
        }
        if ctx.embed_without_frame_ancestors {
            return Err(
                "options.scenarios.writers with an embed login needs an explicit \
                        options.frame_ancestors — without it any site may frame the save button"
                    .into(),
            );
        }
        for d in &self.dashboards {
            if !ctx.known_dashboards.iter().any(|k| k == d) {
                return Err(format!(
                    "options.scenarios names dashboard '{d}', which this server doesn't serve"
                ));
            }
        }
        if ctx.state_path.is_some_and(|p| p == self.path) || writeback_path == Some(&self.path) {
            return Err(
                "options.scenarios.path must not be options.state.path or options.writeback.path"
                    .into(),
            );
        }
        crate::host::state::check_path_rules(&self.path, ctx.data_dir, ctx.dashboard_dir)
            .map_err(|e| e.replace("options.state.path", "options.scenarios.path"))
    }
}

const WRITE_KEYS: [&str; 7] = [
    "path",
    "dashboards",
    "writers",
    "mode",
    "max_rows_per_save",
    "rate",
    "max_mb",
];

impl ScenarioOptions {
    /// `{"database": "warehouse", "tables": ["plan", …] | {"plan": {…}, …},
    /// "max_scenarios": 50}`. `tables` may already be the object form the
    /// write slice will use (its keys are the table names).
    pub fn parse(v: &Value) -> Result<ScenarioOptions, String> {
        let m = v.as_object().ok_or("options.scenarios must be an object")?;
        let mut o = ScenarioOptions {
            database: String::new(),
            tables: Vec::new(),
            max_scenarios: DEFAULT_MAX_SCENARIOS,
            write: None,
        };
        let mut decls: Option<BTreeMap<String, TableDecl>> = None;
        for (k, val) in m {
            match k.as_str() {
                "database" => {
                    o.database = val
                        .as_str()
                        .map(str::trim)
                        .filter(|d| valid_ident(d))
                        .ok_or("options.scenarios.database must be the name of an attached DuckDB database")?
                        .to_string();
                }
                "tables" => {
                    let names: Vec<String> = match val {
                        Value::Array(a) => a
                            .iter()
                            .map(|t| t.as_str().map(str::to_string))
                            .collect::<Option<_>>()
                            .ok_or("options.scenarios.tables must list table names")?,
                        Value::Object(t) => {
                            let mut d = BTreeMap::new();
                            for (name, decl) in t {
                                d.insert(
                                    name.clone(),
                                    wb::parse_table("options.scenarios", name, decl)?,
                                );
                            }
                            decls = Some(d);
                            t.keys().cloned().collect()
                        }
                        _ => return Err("options.scenarios.tables must list table names".into()),
                    };
                    for t in &names {
                        if !valid_ident(t) {
                            return Err(format!(
                                "options.scenarios.tables: '{t}' is not a plain table name"
                            ));
                        }
                    }
                    o.tables = names;
                }
                "max_scenarios" => {
                    o.max_scenarios = val.as_u64().filter(|n| (1..=500).contains(n)).ok_or(
                        "options.scenarios.max_scenarios must be a whole number from 1 to 500",
                    )? as usize;
                }
                k if WRITE_KEYS.contains(&k) => {}
                // S3 (freeze, merge, drop)
                "approvers" => {
                    return Err("options.scenarios.approvers: approving scenarios (freeze, merge, drop) isn't available yet (#1243 S3)".into())
                }
                other => return Err(format!("options.scenarios: unknown key '{other}'")),
            }
        }
        if WRITE_KEYS.iter().any(|k| m.contains_key(*k)) {
            let tables = decls.take().ok_or(
                "options.scenarios.tables: to write scenarios, declare each table's key and \
                 columns: {\"plan\": {\"key\": [\"item\"], \"columns\": {\"item\": \"VARCHAR\", …}}}",
            )?;
            o.write = Some(parse_write(m, tables)?);
        }
        if o.database.is_empty() {
            return Err("options.scenarios.database is required".into());
        }
        if o.tables.is_empty() {
            return Err("options.scenarios.tables must name at least one table".into());
        }
        Ok(o)
    }
}

/// The write keys of `options.scenarios` (`m` has at least one of them).
fn parse_write(
    m: &Map<String, Value>,
    tables: BTreeMap<String, TableDecl>,
) -> Result<WriteOptions, String> {
    let str_list = |val: &Value, key: &str| -> Result<Vec<String>, String> {
        val.as_array()
            .ok_or_else(|| format!("options.scenarios.{key} must be an array of strings"))?
            .iter()
            .map(|x| {
                x.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("options.scenarios.{key}: every entry must be a string"))
            })
            .collect()
    };
    let pos = |val: &Value, key: &str| -> Result<u64, String> {
        val.as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("options.scenarios.{key} must be a positive integer"))
    };
    let mut o = WriteOptions {
        path: PathBuf::new(),
        dashboards: Vec::new(),
        writer_groups: Vec::new(),
        writer_users: Vec::new(),
        tables,
        mode: Mode::Delta,
        max_rows_per_save: wb::DEFAULT_MAX_ROWS,
        text_max_chars: crate::host::state::DEFAULT_TEXT_MAX_CHARS,
        rate_per_minute: wb::DEFAULT_RATE_PER_MINUTE,
        rate_burst: wb::DEFAULT_RATE_BURST,
        max_mb: wb::DEFAULT_MAX_MB,
    };
    if let Some(p) = m.get("path") {
        o.path = PathBuf::from(
            p.as_str()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .ok_or("options.scenarios.path must be a file path")?,
        );
    } else {
        return Err(
            "options.scenarios.path is required to write scenarios (the change log)".into(),
        );
    }
    if let Some(d) = m.get("dashboards") {
        o.dashboards = str_list(d, "dashboards")?;
    }
    if o.dashboards.is_empty() || !o.dashboards.iter().all(|d| valid_dashboard_id(d)) {
        return Err(
            "options.scenarios.dashboards must name the dashboards that write scenarios".into(),
        );
    }
    let wm = m.get("writers").and_then(Value::as_object).ok_or(
        "options.scenarios.writers must be an object like {\"groups\": […], \"users\": […]}",
    )?;
    if let Some(other) = wm
        .keys()
        .find(|k| !matches!(k.as_str(), "groups" | "users"))
    {
        return Err(format!(
            "options.scenarios.writers: unknown key '{other}' (groups, users)"
        ));
    }
    if let Some(g) = wm.get("groups") {
        o.writer_groups = str_list(g, "writers.groups")?;
    }
    if let Some(u) = wm.get("users") {
        o.writer_users = str_list(u, "writers.users")?;
    }
    if o.writer_groups.is_empty() && o.writer_users.is_empty() {
        return Err("options.scenarios.writers needs groups or users".into());
    }
    if let Some(v) = m.get("mode") {
        o.mode = match v.as_str() {
            Some("delta") => Mode::Delta,
            Some("materialized") => Mode::Materialized,
            _ => return Err("options.scenarios.mode must be 'delta' or 'materialized'".into()),
        };
    }
    if let Some(v) = m.get("max_rows_per_save") {
        o.max_rows_per_save = (pos(v, "max_rows_per_save")? as usize).min(wb::MAX_MAX_ROWS);
    }
    if let Some(v) = m.get("rate") {
        let rm = v.as_object().ok_or(
            "options.scenarios.rate must be an object like {\"per_minute\": 12, \"burst\": 5}",
        )?;
        if let Some(other) = rm
            .keys()
            .find(|k| !matches!(k.as_str(), "per_minute" | "burst"))
        {
            return Err(format!(
                "options.scenarios.rate: unknown key '{other}' (per_minute, burst)"
            ));
        }
        if let Some(v) = rm.get("per_minute") {
            o.rate_per_minute = pos(v, "rate.per_minute")?.min(10_000) as u32;
        }
        if let Some(v) = rm.get("burst") {
            o.rate_burst = pos(v, "rate.burst")?.min(10_000) as u32;
        }
    }
    if let Some(v) = m.get("max_mb") {
        o.max_mb = pos(v, "max_mb")?.min(1 << 20);
    }
    Ok(o)
}

// ──────────────────────────────────────────────────────────────── writes ──

/// A scenario name a save may create or write: what anofox-scenario accepts
/// (letters, digits, underscores; at most 64), and not `main` (the base).
pub fn valid_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !s.eq_ignore_ascii_case("main")
}

/// What a scenario `::SAVE` does (its `::SAVE_ACTION` cell).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Write a `::FORM`'s row into the picked scenario (the default).
    Write,
    /// Create a scenario from the picked one (or `main`).
    Create,
}

/// A `::SAVE_ACTION` cell: empty or `write`, `create`; S3's actions are
/// refused with the reason.
pub fn parse_action(cell: &str) -> Result<Action, String> {
    match cell.trim().to_ascii_lowercase().as_str() {
        "" | "write" => Ok(Action::Write),
        "create" => Ok(Action::Create),
        "freeze" | "unfreeze" | "merge" | "drop" => Err(format!(
            "::SAVE_ACTION '{}' (approving scenarios) isn't available yet",
            cell.trim()
        )),
        _ => Err(format!(
            "::SAVE_ACTION must be 'write' or 'create', not '{}'",
            cell.trim()
        )),
    }
}

/// One statement with its bound values, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct Bound {
    pub sql: String,
    pub params: Vec<String>,
}

/// `scenario_create(name, NULL, key_columns := …, from_scenario := parent,
/// mode := …)` with the name and parent bound. `no_pk` are the declared
/// tables without a primary key in the base: anofox-scenario takes their key
/// from `key_columns` (and refuses one for a table that has a key).
pub fn create_sql(
    name: &str,
    parent: Option<&str>,
    mode: Mode,
    tables: &BTreeMap<String, TableDecl>,
    no_pk: &[String],
) -> Bound {
    let keys: Vec<String> = tables
        .iter()
        .filter(|(t, _)| no_pk.contains(t))
        .map(|(t, d)| {
            let cols: Vec<String> = d.key.iter().map(|k| sql_string_literal(k)).collect();
            format!("{}: [{}]", sql_string_literal(t), cols.join(", "))
        })
        .collect();
    let key_arg = if keys.is_empty() {
        String::new()
    } else {
        format!(", key_columns := MAP {{{}}}", keys.join(", "))
    };
    let mut params = vec![name.to_string()];
    let from = match parent {
        Some(p) => {
            params.push(p.to_string());
            "?"
        }
        None => "NULL",
    };
    Bound {
        sql: format!(
            "SELECT * FROM scenario_create(?, NULL::VARCHAR{key_arg}, from_scenario := {from}, mode := '{}')",
            mode.sql()
        ),
        params,
    }
}

/// The row a form save writes: the form's fields, plus the table's single
/// key column from `key` (the `::SAVE_KEY` variable) when the form doesn't
/// carry it. Every field is declared and coerced; every key column has a
/// value. Declared columns the form doesn't carry keep their values.
pub fn form_row(
    opts: &WriteOptions,
    table: &str,
    form: &Map<String, Value>,
    key: Option<&str>,
) -> Result<Map<String, Value>, String> {
    let decl = opts
        .tables
        .get(table)
        .ok_or_else(|| format!("'{table}' is not a table this server writes scenarios into"))?;
    let mut fields = form.clone();
    if let ([k], Some(v)) = (decl.key.as_slice(), key) {
        if fields.get(k).is_none_or(Value::is_null) && !v.is_empty() {
            fields.insert(k.clone(), Value::String(v.to_string()));
        }
    }
    match wb::typed_row(table, decl, &fields, opts.text_max_chars)
        .map_err(|e| e.replace("(options.writeback)", "(options.scenarios)"))?
    {
        Value::Object(m) => Ok(m),
        _ => unreachable!("typed_row builds an object"),
    }
}

/// A coerced value as a bound parameter (`None`: SQL `NULL`).
fn param(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        other => Some(other.to_string()),
    }
}

fn placeholder(ty: ColType, v: &Value, params: &mut Vec<String>) -> String {
    match param(v) {
        None => format!("NULL::{}", ty.sql()),
        Some(p) => {
            params.push(p);
            format!("?::{}", ty.sql())
        }
    }
}

/// Upsert one coerced `row` into `table` of the scenario attached as
/// `alias`, values bound: [`RowSql::exists`] counts the rows with its key;
/// when there is one, [`RowSql::update`] sets the row's other columns
/// (columns it doesn't carry keep their values; `None` when it carries only
/// the key), otherwise [`RowSql::insert`] adds it.
pub fn row_sql(alias: &str, table: &str, decl: &TableDecl, row: &Map<String, Value>) -> RowSql {
    let target = format!("{}.main.{}", quote_ident(alias), quote_ident(table));
    let ty = |c: &str| decl.col(c).unwrap_or(ColType::Varchar);
    let key_cond = |params: &mut Vec<String>| -> String {
        decl.key
            .iter()
            .map(|k| {
                format!(
                    "{} = {}",
                    quote_ident(k),
                    placeholder(ty(k), &row[k.as_str()], params)
                )
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    let mut cp = Vec::new();
    let exists = Bound {
        sql: format!(
            "SELECT count(*)::VARCHAR FROM {target} WHERE {}",
            key_cond(&mut cp)
        ),
        params: cp,
    };
    let mut up = Vec::new();
    let sets: Vec<String> = decl
        .columns
        .iter()
        .filter(|(c, _)| !decl.key.contains(c))
        .filter_map(|(c, t)| {
            row.get(c)
                .map(|v| format!("{} = {}", quote_ident(c), placeholder(*t, v, &mut up)))
        })
        .collect();
    let update = (!sets.is_empty()).then(|| {
        let cond = key_cond(&mut up);
        Bound {
            sql: format!("UPDATE {target} SET {} WHERE {cond}", sets.join(", ")),
            params: up,
        }
    });
    let mut ip = Vec::new();
    let (cols, vals): (Vec<String>, Vec<String>) = decl
        .columns
        .iter()
        .filter_map(|(c, t)| {
            row.get(c)
                .map(|v| (quote_ident(c), placeholder(*t, v, &mut ip)))
        })
        .unzip();
    RowSql {
        exists,
        update,
        insert: Bound {
            sql: format!(
                "INSERT INTO {target} ({}) VALUES ({})",
                cols.join(", "),
                vals.join(", ")
            ),
            params: ip,
        },
    }
}

/// See [`row_sql`].
#[derive(Debug, Clone, PartialEq)]
pub struct RowSql {
    pub exists: Bound,
    pub update: Option<Bound>,
    pub insert: Bound,
}

/// One attached scenario: its name and the catalog it is attached as.
#[derive(Debug, Clone, PartialEq)]
pub struct Attached {
    pub name: String,
    pub alias: String,
}

/// The catalog a scenario of id `id` is attached as.
pub fn alias(id: i64) -> String {
    format!("scn_{id}")
}

/// `ATTACH '<name>' AS scn_<id> (TYPE scenario)`.
pub fn attach_sql(a: &Attached) -> String {
    format!(
        "ATTACH {} AS {} (TYPE scenario)",
        sql_string_literal(&a.name),
        quote_ident(&a.alias)
    )
}

/// The three helpers as `TEMP` macros (one statement each), for a request
/// connection whose database `db` has the scenarios `attached`.
pub fn helpers_sql(db: &str, tables: &[String], attached: &[Attached]) -> Vec<String> {
    let list = tables
        .iter()
        .map(|t| sql_string_literal(t))
        .collect::<Vec<_>>()
        .join(", ");
    // the table name, or an error naming the rule
    let checked = |f: &str| {
        format!(
            "CASE WHEN t IN ({list}) THEN t ELSE error('{f}: ' || t || ' is not one of options.scenarios.tables') END"
        )
    };
    let scenarios = "CREATE OR REPLACE TEMP MACRO anofox_scenarios() AS TABLE \
         SELECT * FROM (SELECT 'main' AS name, NULL::VARCHAR AS mode, false AS frozen, \
         NULL::VARCHAR AS parent, NULL::TIMESTAMP AS created_at, NULL::VARCHAR AS description, \
         NULL::TIMESTAMP AS merged_at \
         UNION ALL SELECT name, mode, frozen, parent, created_at, description, merged_at \
         FROM scenario_list()) ORDER BY name <> 'main', name"
        .to_string();
    let rows_check = checked("anofox_scenario_rows");
    let mut parts = vec![format!(
        "SELECT 'main' AS scenario, * FROM query_table({} || ({rows_check}))",
        sql_string_literal(&format!("{}.main.", quote_ident(db)))
    )];
    for a in attached {
        parts.push(format!(
            "SELECT {} AS scenario, * FROM query_table({} || ({rows_check}))",
            sql_string_literal(&a.name),
            sql_string_literal(&format!("{}.main.", quote_ident(&a.alias)))
        ));
    }
    let rows = format!(
        "CREATE OR REPLACE TEMP MACRO anofox_scenario_rows(t) AS TABLE {}",
        parts.join(" UNION ALL BY NAME ")
    );
    let diff = format!(
        "CREATE OR REPLACE TEMP MACRO anofox_scenario_diff(s, t, against := 'main') AS TABLE \
         FROM scenario_diff(against, s, {})",
        checked("anofox_scenario_diff")
    );
    vec![scenarios, rows, diff]
}

/// The write functions (see [`WRITE_FUNCTIONS`]) `sql` calls: a word that is
/// one of them followed by `(`, or such a call inside a string (SQL text
/// run with `query('…')`). Comments don't count. A stopgap until
/// anofox-scenario#66: text built at run time (`'scenario_' || 'merge('`)
/// isn't seen.
pub fn write_calls(sql: &str) -> Vec<String> {
    let all: Vec<_> = lex::tokenize(sql);
    let toks: Vec<_> = all.iter().filter(|t| !t.is_trivia()).collect();
    let mut out: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        if !out.iter().any(|o| o == name) {
            out.push(name.to_string());
        }
    };
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            TokKind::Word | TokKind::QuotedIdent => {
                let name = t.text(sql).trim_matches('"').to_ascii_lowercase();
                if WRITE_FUNCTIONS.contains(&name.as_str())
                    && toks.get(i + 1).is_some_and(|n| n.is_punct(sql, '('))
                {
                    add(&name);
                }
            }
            TokKind::String | TokKind::DollarString => {
                let text = t.text(sql).to_ascii_lowercase();
                for f in WRITE_FUNCTIONS {
                    let mut rest = text.as_str();
                    while let Some(k) = rest.find(f) {
                        let before = rest[..k].chars().next_back();
                        let after = rest[k + f.len()..].trim_start();
                        if !before.is_some_and(|c| c.is_alphanumeric() || c == '_')
                            && after.starts_with('(')
                        {
                            add(f);
                            break;
                        }
                        rest = &rest[k + f.len()..];
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The variable a dashboard's `::SCENARIO` picker sets: the column name
/// right before `::SCENARIO` (`name AS scenario ::SCENARIO` → `scenario`),
/// in the first statement that has one.
pub fn picker_var(stmts: &[(bool, String)]) -> Option<String> {
    for (_, sql) in stmts {
        let toks: Vec<_> = lex::tokenize(sql)
            .into_iter()
            .filter(|t| !t.is_trivia())
            .collect();
        for w in toks.windows(4) {
            let [name, c1, c2, role] = w else { continue };
            if c1.is_punct(sql, ':') && c2.is_punct(sql, ':') && role.is_word(sql, "SCENARIO") {
                let n = match name.kind {
                    TokKind::Word => name.text(sql).to_string(),
                    TokKind::QuotedIdent => name.text(sql).trim_matches('"').replace("\"\"", "\""),
                    _ => continue,
                };
                return Some(n);
            }
        }
    }
    None
}

/// The statement that makes a request read scenario `value` (the picker's
/// value, as text): `SET search_path` to its catalog when it is exactly one
/// of the `attached` names; `None` (the base) for `main`, nothing, or any
/// other value. `rest` are the operator's further search path entries
/// (`options.search_path` after its first, the scenarios' database).
pub fn search_path_sql(
    value: Option<&str>,
    attached: &[Attached],
    rest: &[String],
) -> Option<String> {
    let v = value?;
    let a = attached.iter().find(|a| a.name == v)?;
    let mut path = vec![format!("{}.main", quote_ident(&a.alias))];
    path.extend(rest.iter().cloned());
    Some(format!(
        "SET search_path = {}",
        sql_string_literal(&path.join(","))
    ))
}

/// Whether a statement works with scenarios explicitly (the helpers or
/// anofox-scenario's own functions). Such a statement keeps the base search
/// path: anofox-scenario finds its registry through the search path's first
/// catalog, so on a branch's `scenario_list()` is empty and `scenario_diff`
/// fails — and it names its branches itself anyway.
pub fn names_scenarios(sql: &str) -> bool {
    lex::tokenize(sql).iter().any(|t| {
        matches!(t.kind, TokKind::Word | TokKind::QuotedIdent) && {
            let w = t.text(sql).trim_matches('"').to_ascii_lowercase();
            w.starts_with("anofox_scenario") || w.starts_with("scenario_")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_picker_and_what_it_switches() {
        let stmts = vec![
            (false, "SELECT 1 AS n ::METRIC".to_string()),
            (
                false,
                "SELECT name AS scenario ::SCENARIO FROM anofox_scenarios()".to_string(),
            ),
        ];
        assert_eq!(picker_var(&stmts).as_deref(), Some("scenario"));
        let quoted = vec![(
            false,
            "SELECT name AS \"Plan\"::scenario FROM t".to_string(),
        )];
        assert_eq!(picker_var(&quoted).as_deref(), Some("Plan"));
        assert_eq!(
            picker_var(&[(false, "SELECT '::SCENARIO' AS x ::TEXT".into())]),
            None
        );
        let a = [Attached {
            name: "demand_up".into(),
            alias: alias(4),
        }];
        assert_eq!(
            search_path_sql(Some("demand_up"), &a, &[]).as_deref(),
            Some("SET search_path = '\"scn_4\".main'")
        );
        assert_eq!(
            search_path_sql(Some("demand_up"), &a, &["ref.lookup".into()]).as_deref(),
            Some("SET search_path = '\"scn_4\".main,ref.lookup'")
        );
        for v in [Some("main"), Some("Demand_up"), Some("x'; DROP"), None] {
            assert_eq!(search_path_sql(v, &a, &[]), None, "{v:?}");
        }
    }

    #[test]
    fn statements_that_name_scenarios_keep_the_base() {
        assert!(names_scenarios(
            "SELECT name AS s ::SCENARIO FROM anofox_scenarios()"
        ));
        assert!(names_scenarios(
            "SELECT * FROM anofox_scenario_diff('a', 'plan')"
        ));
        assert!(names_scenarios("SELECT * FROM scenario_list()"));
        assert!(!names_scenarios("SELECT sum(qty) ::METRIC FROM plan"));
        assert!(!names_scenarios(
            "SELECT 'anofox_scenarios()' AS t ::TEXT FROM plan"
        ));
    }

    #[test]
    fn options_parse_and_refuse() {
        let o = ScenarioOptions::parse(
            &json!({"database": "warehouse", "tables": ["plan", "capacity"]}),
        )
        .unwrap();
        assert_eq!(o.tables, ["plan", "capacity"]);
        assert_eq!(o.max_scenarios, DEFAULT_MAX_SCENARIOS);
        // the write slice's object form, read-only
        let o = ScenarioOptions::parse(&json!({"database": "w", "tables": {
            "plan": {"key": ["item"], "columns": {"item": "VARCHAR", "qty": "DOUBLE"}}}}))
        .unwrap();
        assert_eq!(o.tables, ["plan"]);
        assert!(o.write.is_none());
        for (v, msg) in [
            (json!({"tables": ["plan"]}), "database is required"),
            (json!({"database": "w"}), "at least one table"),
            (
                json!({"database": "w; DROP", "tables": ["plan"]}),
                "attached DuckDB database",
            ),
            (
                json!({"database": "w", "tables": ["plan; x"]}),
                "plain table name",
            ),
            (
                json!({"database": "w", "tables": ["p"], "max_scenarios": 0}),
                "1 to 500",
            ),
            (
                json!({"database": "w", "tables": ["p"], "nope": 1}),
                "unknown key 'nope'",
            ),
        ] {
            let e = ScenarioOptions::parse(&v).unwrap_err();
            assert!(e.contains(msg), "{v}: {e}");
        }
    }

    fn write_opts() -> WriteOptions {
        ScenarioOptions::parse(&json!({
            "database": "w",
            "path": "/srv/scn.jsonl",
            "dashboards": ["sop"],
            "writers": {"groups": ["planners"]},
            "mode": "materialized",
            "tables": {
                "plan": {"key": ["item", "week"], "columns": {"item": "VARCHAR", "week": "DATE", "qty": "DOUBLE"}},
                "capacity": {"key": ["line"], "columns": {"line": "VARCHAR", "per_shift": "DOUBLE", "on": "BOOLEAN"}}
            }
        }))
        .unwrap()
        .write
        .unwrap()
    }

    #[test]
    fn write_options_parse_and_refuse() {
        let w = write_opts();
        assert_eq!(w.mode, Mode::Materialized);
        assert!(w.is_writer("x", &["planners".into()]));
        assert!(!w.is_writer("x", &["viewers".into()]));
        assert!(w.enabled_for("sop") && !w.enabled_for("other"));
        let base = json!({"database": "w", "path": "/p", "dashboards": ["sop"],
            "writers": {"users": ["a"]},
            "tables": {"plan": {"key": ["item"], "columns": {"item": "VARCHAR"}}}});
        let with = |k: &str, v: Value| {
            let mut b = base.clone();
            b[k] = v;
            ScenarioOptions::parse(&b)
        };
        assert!(ScenarioOptions::parse(&base).unwrap().write.is_some());
        for (k, v, msg) in [
            ("tables", json!(["plan"]), "declare each table's key"),
            ("writers", json!({}), "groups or users"),
            ("writers", json!({"roles": []}), "unknown key 'roles'"),
            ("dashboards", json!([]), "name the dashboards"),
            ("dashboards", json!(["../x"]), "name the dashboards"),
            ("mode", json!("copy"), "'delta' or 'materialized'"),
            ("approvers", json!({"users": ["a"]}), "S3"),
            (
                "tables",
                json!({"plan": {"key": ["nope"], "columns": {"item": "VARCHAR"}}}),
                "options.scenarios.tables.plan.key",
            ),
        ] {
            let e = with(k, v.clone()).unwrap_err();
            assert!(e.contains(msg), "{k}={v}: {e}");
        }
        let mut no_path = base.clone();
        no_path.as_object_mut().unwrap().remove("path");
        assert!(ScenarioOptions::parse(&no_path)
            .unwrap_err()
            .contains("path is required"));
    }

    #[test]
    fn names_and_actions() {
        for ok in ["demand_up", "S2", "a_1"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            "main",
            "MAIN",
            "Demand +10 %",
            "a'b",
            "a.b",
            &"x".repeat(65),
        ] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert_eq!(parse_action(""), Ok(Action::Write));
        assert_eq!(parse_action(" Create "), Ok(Action::Create));
        assert!(parse_action("merge")
            .unwrap_err()
            .contains("isn't available yet"));
        assert!(parse_action("delete")
            .unwrap_err()
            .contains("'write' or 'create'"));
    }

    #[test]
    fn create_binds_the_names_and_keys_tables_without_a_primary_key() {
        let w = write_opts();
        let b = create_sql(
            "up",
            Some("base_1"),
            Mode::Delta,
            &w.tables,
            &["plan".into()],
        );
        assert_eq!(
            b.sql,
            "SELECT * FROM scenario_create(?, NULL::VARCHAR, key_columns := MAP {'plan': ['item', 'week']}, from_scenario := ?, mode := 'delta')"
        );
        assert_eq!(b.params, ["up", "base_1"]);
        let b = create_sql("up", None, Mode::Materialized, &w.tables, &[]);
        assert_eq!(
            b.sql,
            "SELECT * FROM scenario_create(?, NULL::VARCHAR, from_scenario := NULL, mode := 'materialized')"
        );
        assert_eq!(b.params, ["up"]);
    }

    #[test]
    fn a_form_row_is_coerced_keyed_and_bound() {
        let w = write_opts();
        let form = json!({"per_shift": "120", "on": true})
            .as_object()
            .unwrap()
            .clone();
        let row = form_row(&w, "capacity", &form, Some("L1'; DROP")).unwrap();
        assert_eq!(
            Value::Object(row.clone()),
            json!({"line": "L1'; DROP", "per_shift": 120.0, "on": true})
        );
        let r = row_sql("__w", "capacity", &w.tables["capacity"], &row);
        assert_eq!(
            r.exists.sql,
            "SELECT count(*)::VARCHAR FROM \"__w\".main.\"capacity\" WHERE \"line\" = ?::VARCHAR"
        );
        assert_eq!(r.exists.params, ["L1'; DROP"]);
        let u = r.update.unwrap();
        assert_eq!(
            u.sql,
            "UPDATE \"__w\".main.\"capacity\" SET \"per_shift\" = ?::DOUBLE, \"on\" = ?::BOOLEAN WHERE \"line\" = ?::VARCHAR"
        );
        assert_eq!(u.params, ["120.0", "true", "L1'; DROP"]);
        assert_eq!(
            r.insert.sql,
            "INSERT INTO \"__w\".main.\"capacity\" (\"line\", \"per_shift\", \"on\") VALUES (?::VARCHAR, ?::DOUBLE, ?::BOOLEAN)"
        );
        assert_eq!(r.insert.params, ["L1'; DROP", "120.0", "true"]);
        // a null value stays a typed NULL, never a bound string
        let row = form_row(
            &w,
            "capacity",
            &json!({"line": "L2", "per_shift": null})
                .as_object()
                .unwrap()
                .clone(),
            None,
        )
        .unwrap();
        let r = row_sql("__w", "capacity", &w.tables["capacity"], &row);
        assert!(r
            .update
            .unwrap()
            .sql
            .contains("SET \"per_shift\" = NULL::DOUBLE WHERE"));
        assert!(
            r.insert.sql.ends_with("VALUES (?::VARCHAR, NULL::DOUBLE)"),
            "{}",
            r.insert.sql
        );
        assert_eq!(r.insert.params, ["L2"]);
        // only the key: nothing to update
        let row = form_row(&w, "capacity", &Map::new(), Some("L3")).unwrap();
        assert!(row_sql("__w", "capacity", &w.tables["capacity"], &row)
            .update
            .is_none());
        // undeclared fields, a missing key, an undeclared table
        for (t, f, key, msg) in [
            (
                "capacity",
                json!({"line": "L1", "x": 1}),
                None,
                "'x' is not a column of 'capacity' (options.scenarios)",
            ),
            (
                "capacity",
                json!({"per_shift": 1}),
                None,
                "the key must have a value",
            ),
            (
                "plan",
                json!({"item": "a", "qty": 1}),
                Some("2026-01-05"),
                "plan.week: the key must have a value",
            ),
            (
                "capacity",
                json!({"line": "L1", "per_shift": "lots"}),
                None,
                "capacity.per_shift",
            ),
            (
                "secrets",
                json!({}),
                None,
                "not a table this server writes scenarios into",
            ),
        ] {
            let e = form_row(&w, t, f.as_object().unwrap(), key).unwrap_err();
            assert!(e.contains(msg), "{f}: {e}");
        }
    }

    #[test]
    fn helpers_read_only_the_declared_tables_of_main_and_each_branch() {
        let a = [Attached {
            name: "Demand +10 %".into(),
            alias: alias(3),
        }];
        let sql = helpers_sql("warehouse", &["plan".into()], &a);
        assert_eq!(sql.len(), 3);
        assert!(
            sql[1].contains("query_table('\"warehouse\".main.' || (CASE WHEN t IN ('plan') THEN t"),
            "{}",
            sql[1]
        );
        assert!(
            sql[1].contains(
                "SELECT 'Demand +10 %' AS scenario, * FROM query_table('\"scn_3\".main.'"
            ),
            "{}",
            sql[1]
        );
        assert!(
            sql[2].contains("scenario_diff(against, s, CASE WHEN t IN ('plan')"),
            "{}",
            sql[2]
        );
        assert_eq!(
            attach_sql(&a[0]),
            "ATTACH 'Demand +10 %' AS \"scn_3\" (TYPE scenario)"
        );
    }

    #[test]
    fn a_name_with_a_quote_stays_a_literal() {
        let a = [Attached {
            name: "it's".into(),
            alias: alias(1),
        }];
        assert!(helpers_sql("w", &["plan".into()], &a)[1].contains("SELECT 'it''s' AS scenario"));
        assert_eq!(
            attach_sql(&a[0]),
            "ATTACH 'it''s' AS \"scn_1\" (TYPE scenario)"
        );
    }

    #[test]
    fn write_calls_are_found_outside_strings_and_comments() {
        assert_eq!(
            write_calls("SELECT * FROM scenario_merge('x', on_conflict := 'ours')"),
            ["scenario_merge"]
        );
        assert_eq!(
            write_calls("CALL Scenario_Create('x'); SELECT \"scenario_drop\"('y')"),
            ["scenario_create", "scenario_drop"]
        );
        // as SQL text for query(), too; a comment or a longer name is not a call
        assert_eq!(
            write_calls("SELECT * FROM query('SELECT * FROM scenario_merge (''x'')')"),
            ["scenario_merge"]
        );
        assert!(
            write_calls("SELECT 'see scenario_merge_preview(' AS t -- scenario_freeze(\n")
                .is_empty()
        );
        assert!(
            write_calls("SELECT * FROM scenario_diff('a', 'plan'), scenario_list()").is_empty()
        );
        assert!(write_calls("SELECT scenario_merge_preview AS c FROM t").is_empty());
    }
}
