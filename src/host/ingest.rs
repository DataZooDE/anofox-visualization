//! The ingest API (#769, docs/plans/plan-p3-gaps.md §1, D3/D4):
//! `POST /api/data/<table>` with a JSON object or an array of objects, from an
//! API key whose scopes name the table (`ingest:<table>`). The locked server
//! writes the rows into one operator-attached database
//! (`options.ingest.database`), never into the snapshot; they show at the next
//! refresh.
//!
//! This module is the part without a database: the options, the request body
//! as rows, and the schema rules. The first batch creates the table with the
//! types DuckDB infers (plus `_id`, a UUID, and `_ts`, the server time); a
//! later batch with a new key adds a column; a type that doesn't fit the
//! existing column is a `400`, never a silent cast. A table with a fixed
//! schema in the options takes only its own columns. The JSON never becomes
//! SQL text: the server writes the rows to a temp file that `read_json` reads.

use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The columns the server adds to every row.
pub const SYSTEM_COLUMNS: [&str; 2] = ["_id", "_ts"];

/// `options.ingest`.
#[derive(Debug, Clone, PartialEq)]
pub struct IngestOptions {
    /// `run`: this replica takes the writes (one writer; the others answer
    /// `503`). Default `false`.
    pub run: bool,
    /// `database`: the attached, read-write database the tables go into.
    pub database: String,
    /// `refresh`: rebuild the snapshot after a write (at most every
    /// [`REFRESH_EVERY_S`] seconds), so the rows show without waiting for
    /// `refresh_s`. Default `false`.
    pub refresh: bool,
    /// `max_body_bytes` (default 1 MiB, at most 16 MiB).
    pub max_body_bytes: usize,
    /// `max_rows` per request (default 10 000, at most 1 000 000).
    pub max_rows: usize,
    /// `rate_per_minute` per API key (default 60).
    pub rate_per_minute: u32,
    /// `tables`: a fixed schema per table (`{"events": {"kind": "VARCHAR",
    /// "n": "BIGINT"}}`); a key outside it is a `400`. Other tables infer.
    pub tables: BTreeMap<String, Vec<(String, String)>>,
}

/// The shortest time between two refreshes that writes asked for.
pub const REFRESH_EVERY_S: u64 = 30;

/// A plain identifier for a table or column: `[A-Za-z_][A-Za-z0-9_]{0,63}`.
pub fn valid_name(s: &str) -> bool {
    crate::host::serving::valid_ident(s)
}

/// A column type for a fixed schema: a DuckDB type name, optionally with a
/// precision (`DECIMAL(18,2)`, `VARCHAR`), optionally a list (`BIGINT[]`).
/// It becomes SQL text in the `CREATE TABLE`, so only this shape passes.
pub fn valid_type(t: &str) -> bool {
    let t = t.trim();
    let t = t.strip_suffix("[]").unwrap_or(t);
    let (name, args) = match t.split_once('(') {
        Some((n, rest)) => match rest.strip_suffix(')') {
            Some(a) => (n, Some(a)),
            None => return false,
        },
        None => (t, None),
    };
    !name.is_empty()
        && name.len() <= 32
        && name.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
        && args.is_none_or(|a| {
            let parts: Vec<&str> = a.split(',').map(str::trim).collect();
            parts.len() <= 2
                && parts
                    .iter()
                    .all(|p| !p.is_empty() && p.len() <= 3 && p.chars().all(|c| c.is_ascii_digit()))
        })
}

impl IngestOptions {
    pub fn parse(v: &Value) -> Result<IngestOptions, String> {
        let o = v.as_object().ok_or(
            "options.ingest must be an object like {\"run\": true, \"database\": \"inbox\"}",
        )?;
        let mut out = IngestOptions {
            run: false,
            database: String::new(),
            refresh: false,
            max_body_bytes: 1 << 20,
            max_rows: 10_000,
            rate_per_minute: 60,
            tables: BTreeMap::new(),
        };
        let num = |k: &str, v: &Value| {
            v.as_u64()
                .filter(|n| *n > 0)
                .ok_or(format!("options.ingest.{k} must be a positive integer"))
        };
        for (k, v) in o {
            match k.as_str() {
                "run" => out.run = v.as_bool().ok_or("options.ingest.run must be true or false")?,
                "refresh" => {
                    out.refresh = v
                        .as_bool()
                        .ok_or("options.ingest.refresh must be true or false")?
                }
                "database" => {
                    out.database = v
                        .as_str()
                        .filter(|d| valid_name(d))
                        .ok_or("options.ingest.database must be the name of an attached database")?
                        .to_string()
                }
                "max_body_bytes" => out.max_body_bytes = num(k, v)?.min(16 << 20) as usize,
                "max_rows" => out.max_rows = num(k, v)?.min(1_000_000) as usize,
                "rate_per_minute" => out.rate_per_minute = num(k, v)?.min(100_000) as u32,
                "tables" => {
                    let t = v
                        .as_object()
                        .ok_or("options.ingest.tables must be an object of tables")?;
                    for (name, cols) in t {
                        if !valid_name(name) {
                            return Err(format!("options.ingest.tables: '{name}' is not a plain table name"));
                        }
                        let cols = cols.as_object().ok_or(format!(
                            "options.ingest.tables.{name} must be an object of column types, like {{\"kind\": \"VARCHAR\"}}"
                        ))?;
                        let mut list = Vec::new();
                        for (c, ty) in cols {
                            let ty = ty.as_str().unwrap_or("");
                            if !valid_name(c) || SYSTEM_COLUMNS.contains(&c.to_ascii_lowercase().as_str()) {
                                return Err(format!("options.ingest.tables.{name}: '{c}' is not a column name (plain, not _id or _ts)"));
                            }
                            if !valid_type(ty) {
                                return Err(format!("options.ingest.tables.{name}.{c}: '{ty}' is not a column type"));
                            }
                            list.push((c.clone(), ty.trim().to_ascii_uppercase()));
                        }
                        if list.is_empty() {
                            return Err(format!("options.ingest.tables.{name} names no columns"));
                        }
                        out.tables.insert(name.to_ascii_lowercase(), list);
                    }
                }
                other => {
                    return Err(format!(
                        "options.ingest: unknown key '{other}' (run, database, refresh, max_body_bytes, max_rows, rate_per_minute, tables)"
                    ))
                }
            }
        }
        if out.database.is_empty() {
            return Err(
                "options.ingest.database is required (the attached database the rows go into)"
                    .into(),
            );
        }
        Ok(out)
    }

    /// The fixed schema of `table`, if the options give one.
    pub fn fixed(&self, table: &str) -> Option<&[(String, String)]> {
        self.tables
            .get(&table.to_ascii_lowercase())
            .map(Vec::as_slice)
    }
}

/// One request's rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    pub rows: Vec<Map<String, Value>>,
    /// Every key, in first-seen order.
    pub columns: Vec<String>,
    /// Keys whose every value in this batch is `null` (or missing).
    pub all_null: BTreeSet<String>,
}

/// The request body as rows: a JSON object, or an array of 1..=`max_rows`
/// objects. Keys are plain identifiers, not `_id` / `_ts`, unique ignoring
/// case; with a fixed schema, only its columns.
pub fn parse_batch(
    body: &str,
    max_rows: usize,
    fixed: Option<&[(String, String)]>,
) -> Result<Batch, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("the body is not JSON: {e}"))?;
    let items = match v {
        Value::Object(o) => vec![Value::Object(o)],
        Value::Array(a) => a,
        _ => return Err("the body must be a JSON object or an array of objects".into()),
    };
    if items.is_empty() {
        return Err("no rows".into());
    }
    if items.len() > max_rows {
        return Err(format!(
            "{} rows; at most {max_rows} per request",
            items.len()
        ));
    }
    let mut columns: Vec<String> = Vec::new();
    let mut non_null: BTreeSet<String> = BTreeSet::new();
    let mut rows = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        let Value::Object(o) = item else {
            return Err(format!("row {i} is not an object"));
        };
        for (k, v) in &o {
            if !valid_name(k) {
                return Err(format!("row {i}: '{k}' is not a column name (letters, digits and _, not starting with a digit)"));
            }
            if SYSTEM_COLUMNS.contains(&k.to_ascii_lowercase().as_str()) {
                return Err(format!("row {i}: '{k}' is set by the server"));
            }
            if let Some(f) = fixed {
                if !f.iter().any(|(c, _)| c.eq_ignore_ascii_case(k)) {
                    return Err(format!(
                        "row {i}: '{k}' is not a column of this table's schema"
                    ));
                }
            }
            match columns.iter().find(|c| c.eq_ignore_ascii_case(k)) {
                Some(c) if c != k => {
                    return Err(format!("row {i}: '{k}' and '{c}' differ only in case"));
                }
                Some(_) => {}
                None => columns.push(k.clone()),
            }
            nested_keys_ok(v, 0).map_err(|e| format!("row {i}, '{k}': {e}"))?;
            if !v.is_null() {
                non_null.insert(k.clone());
            }
        }
        rows.push(o);
    }
    let all_null = columns
        .iter()
        .filter(|c| !non_null.contains(*c))
        .cloned()
        .collect();
    Ok(Batch {
        rows,
        columns,
        all_null,
    })
}

/// Nested objects (a `STRUCT` column) use plain key names too, at most
/// [`MAX_DEPTH`] levels deep: their names end up in the column's type, which
/// the server writes into an `ALTER TABLE … ADD COLUMN` (DuckDB quotes them,
/// this is the second line).
fn nested_keys_ok(v: &Value, depth: usize) -> Result<(), String> {
    match v {
        Value::Object(o) => {
            if depth >= MAX_DEPTH {
                return Err(format!("nested deeper than {MAX_DEPTH} levels"));
            }
            for (k, x) in o {
                if !valid_name(k) {
                    return Err(format!("the nested key '{k}' is not a plain name"));
                }
                nested_keys_ok(x, depth + 1)?;
            }
            Ok(())
        }
        Value::Array(a) => {
            if depth >= MAX_DEPTH {
                return Err(format!("nested deeper than {MAX_DEPTH} levels"));
            }
            a.iter().try_for_each(|x| nested_keys_ok(x, depth + 1))
        }
        _ => Ok(()),
    }
}

/// How deep a value's objects and lists may nest.
pub const MAX_DEPTH: usize = 8;

/// The rows as newline-delimited JSON, for `read_json`.
pub fn ndjson(b: &Batch) -> String {
    let mut s = String::new();
    for r in &b.rows {
        s.push_str(&Value::Object(r.clone()).to_string());
        s.push('\n');
    }
    s
}

fn is_int(t: &str) -> bool {
    matches!(
        t,
        "TINYINT"
            | "SMALLINT"
            | "INTEGER"
            | "BIGINT"
            | "HUGEINT"
            | "UTINYINT"
            | "USMALLINT"
            | "UINTEGER"
            | "UBIGINT"
            | "UHUGEINT"
    )
}

/// May a batch column of type `incoming` (as DuckDB inferred it) go into an
/// existing column of type `existing` without a silent change? The same
/// type; a whole number into any number column (an overflow is DuckDB's
/// own error); a `DATE` into a `TIMESTAMP`.
pub fn compatible(incoming: &str, existing: &str) -> bool {
    let (i, e) = (
        incoming.trim().to_ascii_uppercase(),
        existing.trim().to_ascii_uppercase(),
    );
    i == e
        || (is_int(&i) && (is_int(&e) || e == "DOUBLE" || e.starts_with("DECIMAL")))
        || (i == "DATE" && (e == "TIMESTAMP" || e == "TIMESTAMP WITH TIME ZONE"))
}

/// The columns to add (name, type), and the batch columns to leave out.
pub type ColumnPlan = (Vec<(String, String)>, Vec<String>);

/// What a batch does to an existing table: the columns to add (name, type)
/// and the batch columns to leave out (all `null` and not in the table yet:
/// no type to give them). A type that doesn't fit is an error naming the
/// column.
pub fn plan_columns(
    existing: &[(String, String)],
    incoming: &[(String, String)],
    all_null: &BTreeSet<String>,
) -> Result<ColumnPlan, String> {
    let mut add = Vec::new();
    let mut leave_out = Vec::new();
    for (c, t) in incoming {
        match existing.iter().find(|(e, _)| e.eq_ignore_ascii_case(c)) {
            Some(_) if all_null.contains(c) => {}
            Some((e, et)) => {
                if !compatible(t, et) {
                    return Err(format!(
                        "column '{e}' is {et}; this batch has {t} — send {et} values, or use a new column"
                    ));
                }
            }
            None if all_null.contains(c) => leave_out.push(c.clone()),
            None => add.push((c.clone(), t.clone())),
        }
    }
    Ok((add, leave_out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn options_parse_and_refuse() {
        let o = IngestOptions::parse(&json!({"run": true, "database": "inbox", "refresh": true,
            "tables": {"Events": {"kind": "varchar", "n": "BIGINT", "amount": "DECIMAL(18, 2)", "tags": "VARCHAR[]"}}}))
        .unwrap();
        assert!(o.run && o.refresh && o.database == "inbox");
        assert_eq!(o.max_body_bytes, 1 << 20);
        assert_eq!(
            o.fixed("events").unwrap()[0],
            ("kind".to_string(), "VARCHAR".to_string())
        );
        assert!(o.fixed("other").is_none());
        for bad in [
            json!({"run": true}),
            json!({"database": "x; DROP"}),
            json!({"database": "i", "nope": 1}),
            json!({"database": "i", "max_rows": 0}),
            json!({"database": "i", "tables": {"t": {"_id": "UUID"}}}),
            json!({"database": "i", "tables": {"t": {"c": "VARCHAR); DROP TABLE x; --"}}}),
            json!({"database": "i", "tables": {"t": {"c": "DECIMAL(1,2,3)"}}}),
            json!({"database": "i", "tables": {"a.b": {"c": "VARCHAR"}}}),
            json!({"database": "i", "tables": {"t": {}}}),
            json!("inbox"),
        ] {
            assert!(IngestOptions::parse(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_body_becomes_rows() {
        let b = parse_batch(r#"{"kind": "click", "n": 1}"#, 10, None).unwrap();
        assert_eq!(b.rows.len(), 1);
        assert_eq!(b.columns, ["kind", "n"]);
        let b = parse_batch(r#"[{"a": 1, "b": null}, {"a": 2, "c": "x"}]"#, 10, None).unwrap();
        assert_eq!(b.columns, ["a", "b", "c"]);
        assert_eq!(b.all_null.iter().collect::<Vec<_>>(), ["b"]);
        assert_eq!(ndjson(&b), "{\"a\":1,\"b\":null}\n{\"a\":2,\"c\":\"x\"}\n");
        for (bad, why) in [
            ("[]", "no rows"),
            ("42", "object or an array"),
            ("[1]", "not an object"),
            ("{\"a b\": 1}", "not a column name"),
            ("{\"x; DROP\": 1}", "not a column name"),
            ("{\"_id\": 1}", "set by the server"),
            ("{\"_TS\": 1}", "set by the server"),
            ("[{\"a\": 1}, {\"A\": 2}]", "differ only in case"),
            ("[{},{},{}]", "at most 2"),
            ("{not json", "not JSON"),
            ("{\"o\": {\"a\\\"); DROP TABLE x; --\": 1}}", "nested key"),
            ("{\"o\": [{\"b c\": 1}]}", "nested key"),
            ("{\"o\": [[[[[[[[[1]]]]]]]]]}", "deeper than 8"),
        ] {
            let e = parse_batch(bad, 2, None).unwrap_err();
            assert!(e.contains(why), "{bad}: {e}");
        }
        assert!(parse_batch(r#"{"o": {"k": [1, {"z": 2}]}}"#, 10, None).is_ok());
        let fixed = vec![("kind".to_string(), "VARCHAR".to_string())];
        assert!(parse_batch(r#"{"kind": "x"}"#, 10, Some(&fixed)).is_ok());
        let e = parse_batch(r#"{"kind": "x", "extra": 1}"#, 10, Some(&fixed)).unwrap_err();
        assert!(
            e.contains("'extra' is not a column of this table's schema"),
            "{e}"
        );
    }

    #[test]
    fn types_widen_but_never_silently_change() {
        assert!(compatible("BIGINT", "BIGINT"));
        assert!(compatible("bigint", "DOUBLE"));
        assert!(compatible("BIGINT", "DECIMAL(18,2)"));
        assert!(compatible("BIGINT", "INTEGER"));
        assert!(compatible("DATE", "TIMESTAMP"));
        assert!(!compatible("DOUBLE", "BIGINT"));
        assert!(!compatible("VARCHAR", "DATE"));
        assert!(!compatible("BIGINT", "VARCHAR"));
        assert!(!compatible("DOUBLE", "DECIMAL(18,2)"));
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        let existing = [
            s("_id", "UUID"),
            s("_ts", "TIMESTAMP WITH TIME ZONE"),
            s("kind", "VARCHAR"),
            s("n", "DOUBLE"),
        ];
        let nulls: BTreeSet<String> = ["gone".to_string(), "kind".to_string()].into();
        let (add, out) = plan_columns(
            &existing,
            &[
                s("kind", "JSON"),
                s("N", "BIGINT"),
                s("city", "VARCHAR"),
                s("gone", "JSON"),
            ],
            &nulls,
        )
        .unwrap();
        assert_eq!(add, [s("city", "VARCHAR")]);
        assert_eq!(out, ["gone"]);
        let e = plan_columns(&existing, &[s("n", "VARCHAR")], &BTreeSet::new()).unwrap_err();
        assert!(
            e.contains("column 'n' is DOUBLE; this batch has VARCHAR"),
            "{e}"
        );
    }

    #[test]
    fn fixed_types_are_a_narrow_shape() {
        for ok in [
            "VARCHAR",
            "decimal(18,2)",
            "DECIMAL(18, 2)",
            "BIGINT[]",
            "TIMESTAMP_TZ",
        ] {
            assert!(valid_type(ok), "{ok}");
        }
        for bad in [
            "",
            "VARCHAR)",
            "INT; DROP",
            "DECIMAL(a)",
            "STRUCT(a INT)",
            "VARCHAR -- x",
            "DECIMAL(1000,2)",
        ] {
            assert!(!valid_type(bad), "{bad}");
        }
    }
}
