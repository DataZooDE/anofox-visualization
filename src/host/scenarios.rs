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
//! other name is an error. Writes (S2) aren't here.
//!
//! Pure: the server runs the SQL this module writes.

use crate::host::serving::{quote_ident, sql_string_literal, valid_ident};
use crate::sql::lex::{self, TokKind};
use serde_json::Value;

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
    /// At most this many scenarios (by id) are attached.
    pub max_scenarios: usize,
}

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
        };
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
                        Value::Object(t) => t.keys().cloned().collect(),
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
                // the write slice's keys (S2): known, not used yet
                "path" | "dashboards" | "writers" | "approvers" | "mode" | "max_rows_per_save"
                | "rate" => {}
                other => return Err(format!("options.scenarios: unknown key '{other}'")),
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn options_parse_and_refuse() {
        let o = ScenarioOptions::parse(
            &json!({"database": "warehouse", "tables": ["plan", "capacity"]}),
        )
        .unwrap();
        assert_eq!(o.tables, ["plan", "capacity"]);
        assert_eq!(o.max_scenarios, DEFAULT_MAX_SCENARIOS);
        // the write slice's object form
        let o = ScenarioOptions::parse(
            &json!({"database": "w", "tables": {"plan": {"key": ["item"]}}}),
        )
        .unwrap();
        assert_eq!(o.tables, ["plan"]);
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
