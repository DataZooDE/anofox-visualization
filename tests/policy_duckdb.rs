//! Access policies (issue #49) on a real DuckDB: the rewrite filters rows and
//! masks columns per viewer, and every bypass we know of is refused. Each
//! statement is rewritten exactly as the locked server does it, then run in a
//! read-only session with the viewer's claims set. Skips without a `duckdb`
//! CLI (fails without one on CI, see `common::skip_without`).
mod common;

use anofox_visualization::host::policy::{Enforcer, Policy, Script, Viewer};
use anofox_visualization::host::serving::{page_sql_inline, parse_page, sql_literal};
use std::path::PathBuf;
use std::process::Command;

fn have_duckdb() -> bool {
    let ok = Command::new("duckdb").arg("-version").output().is_ok();
    if !ok {
        common::skip_without("duckdb", "policy_duckdb");
    }
    ok
}

struct Db {
    dir: PathBuf,
    file: String,
}

impl Drop for Db {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const FIXTURE: &str = "
CREATE TABLE sales AS SELECT * FROM (VALUES
  (1, 'EU', 100.0), (2, 'EU', 200.0), (3, 'US', 300.0), (4, 'APAC', 400.0), (5, 'US', 500.0))
  t(id, region, amount);
CREATE TABLE user_regions AS SELECT * FROM (VALUES
  ('alice', 'EU'), ('bob', 'US'), ('bob', 'APAC')) t(\"user\", region);
CREATE TABLE emp AS SELECT * FROM (VALUES
  (1, 'Ann', 'ann@x.com', 9000, '111-11'), (2, 'Ben', 'ben@x.com', 7000, '222-22'),
  (3, 'Cid', 'ann@x.com', 8000, NULL)) t(id, name, email, salary, ssn);
CREATE TABLE payroll AS SELECT * FROM (VALUES (1, 'secret', 123456)) t(id, note, bonus);
CREATE SCHEMA ref;
CREATE TABLE ref.regions AS SELECT * FROM (VALUES ('EU', 'Europe'), ('US', 'Americas')) t(region, label);
CREATE VIEW v_payroll AS SELECT * FROM payroll;
CREATE MACRO leak() AS (SELECT max(bonus) FROM payroll);
CREATE MACRO leak2() AS leak() + 0;
CREATE MACRO t_leak() AS TABLE SELECT * FROM payroll;
CREATE MACRO fmt(x) AS x * 2;
";

const POLICY: &str = r#"{"tables": {
  "sales": {"rows": "region IN (SELECT region FROM user_regions WHERE \"user\" = getvariable('__claim_user')) OR list_contains(getvariable('__claim_groups'), 'admins')"},
  "emp": {"rows": true, "columns": {
      "salary": {"allow_groups": ["hr"], "else": "null"},
      "email": "mask",
      "name": {"allow_groups": ["hr"], "else": "hash"},
      "ssn": {"allow_users": ["carol"], "else": "deny"}}},
  "ref.regions": {"rows": true}
}}"#;

fn db() -> Db {
    let dir = std::env::temp_dir().join(format!(
        "anofox-policy-{}",
        anofox_visualization::host::serving::random_token()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("snap.duckdb").to_string_lossy().into_owned();
    let out = Command::new("duckdb")
        .arg(&file)
        .arg("-c")
        .arg(FIXTURE)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Db { dir, file }
}

/// Run SQL read-only; rows as JSON objects (or DuckDB's error).
fn json(file: &str, sql: &str) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, String> {
    let out = Command::new("duckdb")
        .args(["-readonly", "-json", file, "-c", sql])
        .output()
        .map_err(|e| e.to_string())?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() || stderr.contains("Error") {
        return Err(stderr.into_owned());
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    // Several result sets: keep the last one.
    let last = s.rfind("\n[").map_or(s, |i| &s[i + 1..]);
    anofox_visualization::sql::parse_rows_json(last).map_err(|e| format!("{e}: {s}"))
}

fn rows_as_text(file: &str, sql: &str) -> Result<Vec<Vec<Option<String>>>, String> {
    Ok(json(file, sql)?
        .into_iter()
        .map(|r| {
            r.values()
                .map(|v| match v {
                    serde_json::Value::Null => None,
                    serde_json::Value::String(s) => Some(s.clone()),
                    other => Some(other.to_string()),
                })
                .collect()
        })
        .collect())
}

struct Fx {
    db: Db,
    enf: Enforcer,
}

fn fx() -> Option<Fx> {
    fx_with(POLICY)
}

fn fx_with(policy: &str) -> Option<Fx> {
    if !have_duckdb() {
        return None;
    }
    let db = db();
    let p = Policy::parse(policy).unwrap();
    let file = db.file.clone();
    let enf = Enforcer::compile(&p, &mut |sql: &str| rows_as_text(&file, sql), "snap").unwrap();
    Some(Fx { db, enf })
}

fn viewer(user: &str, groups: &[&str]) -> Viewer {
    Viewer {
        user: Some(user.into()),
        groups: groups.iter().map(|g| g.to_string()).collect(),
    }
}

impl Fx {
    /// Rewrite a script (every statement but the last is setup).
    fn rewrite(&self, stmts: &[&str]) -> Result<Script, String> {
        let file = self.db.file.clone();
        let planned: Vec<(bool, String)> = stmts
            .iter()
            .enumerate()
            .map(|(i, s)| (i + 1 < stmts.len(), s.to_string()))
            .collect();
        self.enf
            .rewrite_script(&mut |sql: &str| rows_as_text(&file, sql), &planned)
            .map_err(|(i, e)| format!("statement {i}: {e}"))
    }

    /// Run a rewritten script as `v` would: claims, views, setup, panel —
    /// `wrap` turns the panel into the SQL actually run (`::PAGED`).
    fn run_wrapped(
        &self,
        s: &Script,
        v: &Viewer,
        wrap: impl Fn(&str) -> String,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, String> {
        // The server registers the keyed hash in Rust; here a stand-in.
        let mut sql = String::from(
            "CREATE TEMP MACRO __anofox_policy_hash(d, v) AS left(sha256('test-key:' || d || ':' || v), 16);\n",
        );
        if let Some(u) = &v.user {
            sql.push_str(&format!(
                "SET VARIABLE __claim_user = {};\n",
                sql_literal(&serde_json::json!(u)).unwrap()
            ));
        }
        sql.push_str(&format!(
            "SET VARIABLE __claim_groups = {};\n",
            sql_literal(&serde_json::json!(v.groups)).unwrap()
        ));
        for st in self.enf.view_statements(&s.tables, v) {
            sql.push_str(&st);
            sql.push_str(";\n");
        }
        let (panel, setup) = s.stmts.split_last().unwrap();
        for (_, st) in setup {
            sql.push_str(st);
            sql.push_str(";\n");
        }
        sql.push_str(&wrap(&panel.1));
        json(&self.db.file, &sql)
    }

    fn run(
        &self,
        s: &Script,
        v: &Viewer,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, String> {
        self.run_wrapped(s, v, |p| p.to_string())
    }

    /// The rewrite must refuse `stmts`, with a message containing `why`.
    fn refused(&self, stmts: &[&str], why: &str) {
        match self.rewrite(stmts) {
            Ok(s) => panic!("not refused: {stmts:?}\n→ {:?}", s.stmts),
            Err(e) => assert!(e.contains(why), "{stmts:?}: {e} (expected '{why}')"),
        }
    }
}

fn col(rows: &[serde_json::Map<String, serde_json::Value>], c: &str) -> Vec<serde_json::Value> {
    rows.iter().map(|r| r[c].clone()).collect()
}

fn ids(rows: &[serde_json::Map<String, serde_json::Value>]) -> Vec<i64> {
    let mut v: Vec<i64> = rows.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    v.sort();
    v
}

#[test]
fn rows_follow_the_viewer() {
    let Some(f) = fx() else { return };
    let s = f
        .rewrite(&["SELECT id, region, amount FROM sales"])
        .unwrap();
    assert!(!s.stmts[0].1.contains("snap"), "{}", s.stmts[0].1);
    assert_eq!(ids(&f.run(&s, &viewer("alice", &[])).unwrap()), vec![1, 2]);
    assert_eq!(ids(&f.run(&s, &viewer("bob", &[])).unwrap()), vec![3, 4, 5]);
    assert_eq!(
        ids(&f.run(&s, &viewer("root", &["admins"])).unwrap()),
        vec![1, 2, 3, 4, 5]
    );
    assert!(f.run(&s, &viewer("mallory", &[])).unwrap().is_empty());
    // Anonymous (no login): no claims, no rows.
    assert!(f.run(&s, &Viewer::default()).unwrap().is_empty());
}

#[test]
fn columns_are_masked_per_viewer() {
    let Some(f) = fx() else { return };
    let s = f.rewrite(&["SELECT * FROM emp ORDER BY id"]).unwrap();
    let anon = f.run(&s, &viewer("alice", &[])).unwrap();
    assert_eq!(anon.len(), 3);
    assert!(!anon[0].contains_key("ssn"), "denied columns are dropped");
    assert!(col(&anon, "salary").iter().all(|v| v.is_null()));
    assert_eq!(col(&anon, "email")[0], "****");
    let names = col(&anon, "name");
    assert_eq!(names[0].as_str().unwrap().len(), 16);
    assert_ne!(names[0], "Ann");
    assert_ne!(names[0], names[1]);
    // hr sees salary and names; carol sees ssn.
    let hr = f.run(&s, &viewer("x", &["hr"])).unwrap();
    assert_eq!(col(&hr, "salary")[0], 9000);
    assert_eq!(col(&hr, "name")[0], "Ann");
    assert!(!hr[0].contains_key("ssn"));
    let carol = f.run(&s, &viewer("carol", &[])).unwrap();
    assert_eq!(col(&carol, "ssn")[0], "111-11");
    // Equal values hash equally (grouping still works), NULL stays NULL.
    let s = f
        .rewrite(&["SELECT count(DISTINCT email) AS n, count(salary) AS k FROM emp"])
        .unwrap();
    let r = f.run(&s, &viewer("alice", &[])).unwrap();
    assert_eq!((r[0]["n"].clone(), r[0]["k"].clone()), (1.into(), 0.into()));
    // A panel naming a denied column fails for that viewer only.
    let s = f.rewrite(&["SELECT id, ssn FROM emp"]).unwrap();
    assert!(f.run(&s, &viewer("alice", &[])).is_err());
    assert_eq!(f.run(&s, &viewer("carol", &[])).unwrap().len(), 3);
    // The lint sees the restricted columns of a statement.
    let s = f.rewrite(&["SELECT e.ssn, salary FROM emp e"]).unwrap();
    let mut hits: Vec<String> = f
        .enf
        .restricted_columns(&s.uses[0])
        .iter()
        .map(|(_, c)| format!("{}:{}", c.column, c.otherwise.as_str()))
        .collect();
    hits.sort();
    assert_eq!(hits, vec!["salary:null", "ssn:deny"]);
}

#[test]
fn hash_is_keyed() {
    let Some(f) = fx() else { return };
    let s = f.rewrite(&["SELECT name FROM emp WHERE id = 1"]).unwrap();
    let h = f.run(&s, &viewer("alice", &[])).unwrap()[0]["name"].clone();
    for plain in ["md5('Ann')", "sha256('Ann')", "left(sha256('Ann'), 16)"] {
        let r = json(&f.db.file, &format!("SELECT {plain} AS h")).unwrap();
        assert_ne!(r[0]["h"], h, "{plain}");
    }
}

#[test]
fn queries_through_ctes_subqueries_joins_and_unions_stay_filtered() {
    let Some(f) = fx() else { return };
    let alice = viewer("alice", &[]);
    for q in [
        "WITH x AS (SELECT * FROM sales) SELECT id FROM x",
        "WITH sales AS (SELECT * FROM sales) SELECT id FROM sales",
        "WITH payroll AS (SELECT * FROM sales) SELECT id FROM payroll",
        "WITH a AS (SELECT * FROM sales), b AS (SELECT * FROM a) SELECT id FROM b",
        "SELECT id FROM (SELECT * FROM sales) s",
        "SELECT id FROM (FROM sales)",
        "SELECT id FROM sales WHERE id IN (SELECT id FROM sales)",
        "SELECT DISTINCT s1.id FROM sales s1 JOIN sales s2 USING (region)",
        "SELECT DISTINCT id FROM sales, LATERAL (SELECT amount FROM sales s2 WHERE s2.id = sales.id)",
        "SELECT id FROM sales UNION SELECT id FROM sales",
        "SELECT id FROM sales EXCEPT SELECT 99",
        "SELECT sales.id FROM sales POSITIONAL JOIN (SELECT 1)",
        "SELECT s1.id FROM sales s1 ASOF JOIN sales s2 ON s1.id >= s2.id AND s1.region = s2.region",
        "SELECT id FROM sales UNION ALL BY NAME SELECT id FROM sales WHERE false",
        "SELECT id FROM sales QUALIFY row_number() OVER (PARTITION BY region) >= 1",
        "FROM sales SELECT id",
        "SELECT id FROM \"SALES\"",
        "SELECT id FROM main.sales",
        "SELECT id FROM sales USING SAMPLE 100%",
        "SELECT unnest(ids) AS id FROM (SELECT list(id) AS ids FROM sales)",
        "SELECT * FROM unnest((SELECT list(id) FROM sales)) t(id)",
        "WITH RECURSIVE r(id) AS (SELECT min(id) FROM sales UNION ALL SELECT r.id + 1 FROM r WHERE r.id < 2) SELECT id FROM r",
        "SELECT (SELECT max(id) FROM sales) AS id",
        "SELECT id FROM sales WHERE EXISTS (SELECT 1 FROM sales s2 WHERE s2.id = sales.id)",
        "SELECT id FROM (FROM sales PIVOT (sum(amount) FOR region IN ('EU', 'US', 'APAC')))",
    ] {
        let s = f
            .rewrite(&[q])
            .unwrap_or_else(|e| panic!("{q}: {e}"));
        let got = ids(&f.run(&s, &alice).unwrap_or_else(|e| panic!("{q}: {e}\n{:?}", s.stmts)));
        assert!(
            got.iter().all(|i| [1, 2].contains(i)) && !got.is_empty(),
            "{q} → {got:?}"
        );
    }
}

#[test]
fn setup_statements_are_rewritten_too() {
    let Some(f) = fx() else { return };
    let s = f
        .rewrite(&[
            "CREATE TEMP TABLE mine AS SELECT * FROM sales",
            "CREATE OR REPLACE TEMP VIEW big AS SELECT * FROM mine WHERE amount > 150",
            "SET VARIABLE top = (SELECT max(amount) FROM sales)",
            "SET VARIABLE pay = (SELECT max(salary) FROM emp)",
            "SELECT id, getvariable('top') AS top, getvariable('pay') AS pay FROM big",
        ])
        .unwrap();
    let r = f.run(&s, &viewer("alice", &[])).unwrap();
    assert_eq!(ids(&r), vec![2]);
    assert!(r[0]["top"].to_string().contains("200"), "{r:?}");
    assert!(r[0]["pay"].is_null(), "masked inside SET VARIABLE, too");
    // A temp table shadowing a policy table is the dashboard's own, filtered copy.
    let s = f
        .rewrite(&[
            "CREATE TEMP TABLE sales AS SELECT * FROM sales WHERE amount < 1000",
            "SELECT id FROM sales",
        ])
        .unwrap();
    assert_eq!(ids(&f.run(&s, &viewer("alice", &[])).unwrap()), vec![1, 2]);
}

#[test]
fn paged_requests_inherit_the_rewrite() {
    let Some(f) = fx() else { return };
    let s = f
        .rewrite(&["SELECT id, region, amount FROM sales"])
        .unwrap();
    let alice = viewer("alice", &[]);
    let page = |p: &str| {
        let p = parse_page(&serde_json::from_str(p).unwrap()).unwrap();
        move |base: &str| page_sql_inline(base, &p)
    };
    let r = f
        .run_wrapped(&s, &alice, page(r#"{"count":true}"#))
        .unwrap();
    assert_eq!(r[0]["n"], 2);
    let r = f
        .run_wrapped(&s, &alice, page(r#"{"distinct":"region"}"#))
        .unwrap();
    assert_eq!(col(&r, "v"), vec![serde_json::json!("EU")]);
    let r = f
        .run_wrapped(&s, &alice, page(r#"{"stats":["amount"]}"#))
        .unwrap();
    assert_eq!(
        (r[0]["n"].clone(), r[0]["s0"].clone()),
        (2.into(), 300.0.into())
    );
    let r = f
        .run_wrapped(&s, &alice, page(r#"{"profile":["amount"]}"#))
        .unwrap();
    assert!(!r.is_empty());
    let r = f
        .run_wrapped(
            &s,
            &alice,
            page(r#"{"limit":10,"sort":"amount","desc":true}"#),
        )
        .unwrap();
    assert_eq!(ids(&r), vec![1, 2]);
    // Filtering on a value the viewer can't see finds nothing.
    let r = f
        .run_wrapped(
            &s,
            &alice,
            page(r#"{"count":true,"filters":[{"col":"region","op":"eq","value":"US"}]}"#),
        )
        .unwrap();
    assert_eq!(r[0]["n"], 0);
}

#[test]
fn tables_without_a_policy_are_refused() {
    let Some(f) = fx() else { return };
    for q in [
        "SELECT * FROM payroll",
        "SELECT * FROM user_regions",
        "SELECT * FROM v_payroll",
        "FROM payroll",
        "SELECT * FROM sales WHERE id IN (SELECT id FROM payroll)",
        "SELECT (SELECT max(bonus) FROM payroll)",
        "SELECT * FROM sales, LATERAL (SELECT * FROM payroll p WHERE p.id = sales.id)",
        "SELECT region FROM sales UNION ALL SELECT note FROM payroll",
        "SELECT * FROM sales WHERE EXISTS (SELECT 1 FROM payroll)",
        "WITH x AS (SELECT * FROM payroll) SELECT * FROM x",
        "WITH RECURSIVE r AS (SELECT * FROM payroll UNION ALL SELECT * FROM r) SELECT * FROM r",
        "SELECT * FROM unnest((SELECT list(bonus) FROM payroll))",
        "SELECT * FROM range((SELECT max(bonus) FROM payroll))",
        "SELECT * FROM __anofox_rls_0",
        "SELECT * FROM regions",
        "SELECT * FROM 'data.csv'",
        "SELECT * FROM 'payroll.parquet'",
        "FROM payroll PIVOT (sum(bonus) FOR note IN ('secret'))",
        "SELECT list_transform([1], x -> (SELECT max(bonus) FROM payroll))",
        "VALUES ((SELECT max(bonus) FROM payroll))",
        "SELECT * FROM (VALUES (1)) v(x) WHERE x IN (FROM payroll SELECT id)",
        "SELECT id FROM sales UNION ALL BY NAME FROM payroll",
        "SELECT max(amount) FILTER (WHERE id IN (SELECT id FROM payroll)) FROM sales",
        "SELECT id FROM sales ORDER BY (SELECT max(bonus) FROM payroll)",
        "SELECT id FROM sales GROUP BY GROUPING SETS ((id), ()) HAVING count(*) < (SELECT max(bonus) FROM payroll)",
        "SELECT sum(amount) OVER (ORDER BY (SELECT bonus FROM payroll)) FROM sales",
        "SELECT * FROM sales s1 ASOF JOIN payroll p ON s1.id >= p.id",
        "WITH a AS (SELECT * FROM sales) SELECT * FROM (SELECT * FROM __anofox_cte_1)",
    ] {
        f.refused(&[q], "no access policy");
    }
}

#[test]
fn qualified_names_are_refused() {
    let Some(f) = fx() else { return };
    for q in [
        "SELECT * FROM snap.main.sales",
        "SELECT * FROM snap.main.payroll",
        "SELECT * FROM temp.main.__anofox_rls_0",
        "SELECT * FROM \"temp\".\"main\".\"__anofox_rls_0\"",
        "SELECT * FROM system.main.duckdb_tables",
    ] {
        f.refused(&[q], "qualified with a database");
    }
    for q in [
        "SELECT * FROM snap.sales",
        "SELECT * FROM snap.payroll",
        "SELECT * FROM information_schema.tables",
        "SELECT * FROM information_schema.columns",
        "SELECT * FROM pg_catalog.pg_tables",
        "SELECT * FROM temp.__anofox_rls_0",
    ] {
        f.refused(&[q], "no access policy");
    }
    // A schema-qualified policy table is fine.
    let s = f
        .rewrite(&["SELECT label FROM ref.regions ORDER BY 1"])
        .unwrap();
    assert_eq!(f.run(&s, &viewer("alice", &[])).unwrap().len(), 2);
}

#[test]
fn table_functions_and_catalog_functions_are_refused() {
    let Some(f) = fx() else { return };
    for q in [
        "SELECT * FROM query('SELECT * FROM payroll')",
        "SELECT * FROM query('SELECT * FROM ' || getvariable('t'))",
        "SELECT * FROM query_table('payroll')",
        "SELECT * FROM query_table(['payroll', 'sales'])",
        "SELECT * FROM query_table(getvariable('t'))",
        "SELECT * FROM json_execute_serialized_sql(json_serialize_sql('SELECT * FROM payroll'))",
        "SELECT * FROM read_csv('/etc/passwd')",
        "SELECT * FROM read_text('/etc/passwd')",
        "SELECT * FROM read_parquet('x.parquet')",
        "SELECT * FROM glob('*')",
        "SELECT * FROM duckdb_tables()",
        "SELECT * FROM duckdb_columns()",
        "SELECT * FROM duckdb_views()",
        "SELECT * FROM duckdb_functions()",
        "SELECT * FROM duckdb_settings()",
        "SELECT * FROM duckdb_secrets()",
        "SELECT * FROM pragma_table_info('payroll')",
        "SELECT * FROM pragma_storage_info('payroll')",
        "SELECT * FROM sql_auto_complete('SELECT * FROM pay')",
        "SELECT * FROM t_leak()",
        "SELECT * FROM histogram_values(sales, amount)",
        "SELECT * FROM main.range(3)",
        "SELECT * FROM sales, query('SELECT 1')",
        "SELECT (SELECT count(*) FROM duckdb_tables())",
    ] {
        f.refused(&[q], "not allowed under the access policy");
    }
    for q in [
        "SELECT leak()",
        "SELECT leak2()",
        "SELECT pg_get_viewdef(1)",
        "SELECT format_type(1, 1)",
        "SELECT id FROM sales WHERE amount < leak()",
    ] {
        f.refused(&[q], "macro");
    }
    // Harmless generators still work.
    let s = f
        .rewrite(&["SELECT count(*) AS n FROM range(3), generate_series(1, 2), unnest([1])"])
        .unwrap();
    assert_eq!(f.run(&s, &viewer("alice", &[])).unwrap()[0]["n"], 6);
}

#[test]
fn file_readers_need_the_policy() {
    let Some(f) =
        fx_with(r#"{"tables": {"sales": {"rows": true}}, "table_functions": ["read_csv"]}"#)
    else {
        return;
    };
    let csv = f.db.dir.join("x.csv");
    std::fs::write(&csv, "a\n1\n2\n").unwrap();
    let q = format!("SELECT count(*) AS n FROM read_csv('{}')", csv.display());
    let s = f.rewrite(&[q.as_str()]).unwrap();
    assert_eq!(f.run(&s, &Viewer::default()).unwrap()[0]["n"], 2);
    f.refused(&["SELECT * FROM read_parquet('x')"], "not allowed");
}

#[test]
fn ctes_cannot_smuggle_a_table() {
    let Some(f) = fx() else { return };
    // `payroll` inside its own non-recursive CTE would be the table: renamed,
    // it can only be the CTE, so DuckDB refuses the statement.
    for q in [
        "WITH payroll AS (SELECT * FROM payroll) SELECT * FROM payroll",
        "WITH sales AS (SELECT * FROM payroll) SELECT * FROM sales",
        "WITH s AS (SELECT * FROM sales), payroll AS (SELECT * FROM payroll) SELECT * FROM s",
    ] {
        match f.rewrite(&[q]) {
            Err(_) => {}
            Ok(s) => {
                let r = f.run(&s, &viewer("root", &["admins"]));
                assert!(
                    r.as_ref()
                        .map_or(true, |rows| rows.iter().all(|r| !r.contains_key("bonus"))),
                    "{q} → {r:?}"
                );
            }
        }
    }
    // A CTE named like a view cannot reach the view.
    f.refused(
        &["WITH __anofox_rls_0 AS (SELECT 1) SELECT * FROM temp.main.__anofox_rls_0"],
        "qualified",
    );
    // An inner CTE named like an outer table, used outside its scope.
    let s = f
        .rewrite(&["SELECT id FROM (WITH payroll AS (SELECT * FROM sales) SELECT * FROM payroll)"])
        .unwrap();
    assert_eq!(ids(&f.run(&s, &viewer("alice", &[])).unwrap()), vec![1, 2]);
    f.refused(
        &["SELECT * FROM (WITH payroll AS (SELECT 1) SELECT * FROM payroll), payroll"],
        "no access policy",
    );
}

#[test]
fn statements_other_than_queries_are_refused() {
    let Some(f) = fx() else { return };
    for (q, why) in [
        ("PIVOT sales ON region USING sum(amount)", "only queries"),
        ("EXPLAIN SELECT * FROM payroll", "not allowed"),
        ("DESCRIBE payroll", "no access policy"),
        ("SUMMARIZE payroll", "no access policy"),
        ("SHOW TABLES", "bare name"),
        ("SHOW ALL TABLES", "bare name"),
        ("SELECT 1; SELECT * FROM payroll", "exactly one"),
        ("SELECT * FROM sales AT (VERSION => 1)", "time travel"),
    ] {
        f.refused(&[q, "SELECT 1"], why);
    }
    for (q, why) in [
        (
            "SET VARIABLE __claim_groups = ['admins']",
            "set by the server",
        ),
        ("SET VARIABLE __CLAIM_user = 'root'", "set by the server"),
        ("SET VARIABLE __anofox_x = 1", "set by the server"),
        ("CREATE TEMP MACRO m() AS (SELECT 1)", "not allowed"),
        (
            "CREATE TEMP TABLE __anofox_rls_0 AS SELECT * FROM sales",
            "reserved",
        ),
        (
            "CREATE OR REPLACE TEMP VIEW __anofox_rls_0 AS SELECT 1",
            "reserved",
        ),
        ("CREATE TABLE t AS SELECT * FROM sales", "TEMP"),
        (
            "CREATE TEMP TABLE t AS SELECT * FROM payroll",
            "no access policy",
        ),
        ("ATTACH 'x.duckdb' AS x", "not allowed"),
        ("SET threads = 1", "not allowed"),
        ("RESET VARIABLE __claim_user", "not allowed"),
        ("INSERT INTO sales VALUES (9, 'EU', 1)", "not allowed"),
        ("USE system", "not allowed"),
    ] {
        f.refused(&[q, "SELECT 1"], why);
    }
    // DESCRIBE of a query on a policy table shows the viewer's columns only.
    let s = f.rewrite(&["DESCRIBE SELECT * FROM emp"]).unwrap();
    let names = col(&f.run(&s, &viewer("alice", &[])).unwrap(), "column_name");
    assert!(!names.contains(&"ssn".into()), "{names:?}");
}

#[test]
fn a_row_filter_reads_only_claims() {
    if !have_duckdb() {
        return;
    }
    let db = db();
    let file = db.file.clone();
    let compile = |p: &str| {
        Enforcer::compile(
            &Policy::parse(p).unwrap(),
            &mut |sql: &str| rows_as_text(&file, sql),
            "snap",
        )
    };
    for (rows, why) in [
        ("region = getvariable('region')", "only read claims"),
        (
            "region = getvariable(lower('__claim_x'))",
            "only read claims",
        ),
        ("region IN (SELECT * FROM query('SELECT 1'))", "not allowed"),
        ("region = 'EU' OR leak() > 0", "macro"),
        ("nope = 1", "nope"),
        ("region = 'EU'; DROP TABLE x", "parse"),
        (
            "true) UNION SELECT * FROM sales WHERE (true",
            "one condition",
        ),
        ("true) GROUP BY ALL HAVING (true", "one condition"),
        ("true) LIMIT (1", "one condition"),
        ("true) QUALIFY (true", "one condition"),
    ] {
        let p = format!(
            r#"{{"tables": {{"sales": {{"rows": {}}}}}}}"#,
            serde_json::json!(rows)
        );
        match compile(&p) {
            Ok(_) => panic!("accepted: {rows}"),
            Err(e) => assert!(
                e.to_lowercase().contains(why) || why == "parse",
                "{rows}: {e}"
            ),
        }
    }
    for (p, why) in [
        (r#"{"tables": {"nope": {"rows": true}}}"#, "does not exist"),
        (
            r#"{"tables": {"emp": {"rows": true, "columns": {"wage": "null"}}}}"#,
            "no column",
        ),
    ] {
        assert!(compile(p).unwrap_err().contains(why), "{p}");
    }
    // A wrong `rows` is reported at startup, never served unfiltered.
    assert!(compile(r#"{"tables": {"sales": {"rows": "true OR 1/0"}}}"#).is_ok());
}

#[test]
fn reserved_names_in_the_snapshot_stop_the_start() {
    if !have_duckdb() {
        return;
    }
    let db = db();
    let out = Command::new("duckdb")
        .arg(&db.file)
        .arg("-c")
        .arg("CREATE TABLE __anofox_cte_1 AS SELECT * FROM payroll")
        .output()
        .unwrap();
    assert!(out.status.success());
    let file = db.file.clone();
    let e = Enforcer::compile(
        &Policy::parse(r#"{"tables": {"sales": {"rows": true}}}"#).unwrap(),
        &mut |sql: &str| rows_as_text(&file, sql),
        "snap",
    )
    .unwrap_err();
    assert!(e.contains("reserved"), "{e}");
}

#[test]
fn macros_are_refused_but_columns_named_like_them_are_fine() {
    let Some(f) = fx() else { return };
    // A user macro without a table is still refused: its body is not checked.
    f.refused(&["SELECT fmt(id) FROM sales"], "macro");
    let s = f.rewrite(&["SELECT id AS leak FROM sales"]).unwrap();
    assert_eq!(f.run(&s, &viewer("alice", &[])).unwrap().len(), 2);
}

#[test]
fn the_lint_reports_refusals_and_hidden_columns() {
    let Some(f) = fx() else { return };
    let file = f.db.file.clone();
    let script = "SELECT region::XAXIS, sum(amount)::BARCHART FROM sales GROUP BY ALL;\n\
                  SELECT id::TABLE, salary, ssn FROM emp;\n\
                  SELECT note::TABLE FROM payroll;\n\
                  SELECT id::TABLE FROM emp;";
    let d = anofox_visualization::lint::check_policy(script, &f.enf, &mut |sql: &str| {
        rows_as_text(&file, sql)
    });
    let got: Vec<(usize, &str)> = d.iter().map(|d| (d.stmt, d.code)).collect();
    assert_eq!(
        got,
        vec![
            (2, "policy/masked-column"),
            (2, "policy/denied-column"),
            (3, "policy/refused")
        ],
        "{d:#?}"
    );
}

#[test]
fn every_duckdb_function_is_classified() {
    let Some(f) = fx() else { return };
    let list = anofox_visualization::host::policy::functions();
    let rows = json(
        &f.db.file,
        "LOAD json; SELECT DISTINCT function_type AS t, function_name AS f FROM duckdb_functions() \
         WHERE function_type <> 'pragma' AND internal",
    )
    .unwrap();
    let missing: Vec<String> = rows
        .iter()
        .filter_map(|r| {
            let (t, name) = (r["t"].as_str()?, r["f"].as_str()?);
            let table = matches!(t, "table" | "table_macro");
            // ICU's per-locale collations differ between builds (harmless).
            (list.get(table, name).is_none() && !name.starts_with("icu_collate_"))
                .then(|| format!("{t} {name}"))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "functions not classified for the access policy (src/host/policy_functions.txt): {missing:?}"
    );
    // The functions #105 found stay denied, and the generators allowed.
    for name in [
        "json_serialize_plan",
        "stats",
        "current_setting",
        "currval",
        "getenv",
    ] {
        assert_eq!(list.get(false, name).map(|x| x.0), Some(false), "{name}");
    }
    for name in [
        "query",
        "query_table",
        "duckdb_tables",
        "read_csv",
        "json_execute_serialized_sql",
    ] {
        assert_eq!(list.get(true, name).map(|x| x.0), Some(false), "{name}");
    }
    for name in ["range", "generate_series", "unnest"] {
        assert_eq!(list.get(true, name).map(|x| x.0), Some(true), "{name}");
    }
}

#[test]
fn unknown_functions_need_the_policy() {
    let Some(f) = fx() else { return };
    f.refused(
        &["SELECT my_ext_fn(id) FROM sales"],
        "reviewed function list",
    );
    f.refused(
        &["SELECT __anofox_policy_hash('x', 'y')"],
        "reviewed function list",
    );
    let Some(g) = fx_with(r#"{"tables": {"sales": {"rows": true}}, "functions": ["my_ext_fn"]}"#)
    else {
        return;
    };
    assert!(g.rewrite(&["SELECT my_ext_fn(id) FROM sales"]).is_ok());
    for bad in [
        r#"{"functions": ["json_serialize_plan"]}"#,
        r#"{"functions": ["__anofox_policy_hash"]}"#,
    ] {
        assert!(Policy::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn plan_stats_and_settings_functions_are_refused() {
    let Some(f) = fx() else { return };
    for q in [
        "SELECT json_serialize_plan('SELECT * FROM payroll WHERE bonus > 42', optimize := true)",
        "SELECT json_serialize_plan('SELECT name FROM temp.main.__anofox_rls_1')",
        "SELECT stats(region) FROM sales",
        "SELECT json_serialize_sql('SELECT 1')",
        "SELECT json_deserialize_sql(json_serialize_sql('SELECT 1'))",
        "SELECT current_setting('allowed_directories')",
        "SELECT current_query()",
        "SELECT nextval('s')",
        "SELECT currval('s')",
        "SELECT getenv('HOME')",
        "SELECT id FROM sales WHERE stats(amount) LIKE '%Max%'",
        "SET VARIABLE x = json_serialize_plan('FROM payroll')",
        "SELECT JSON_SERIALIZE_PLAN('FROM payroll')",
    ] {
        f.refused(&[q], "not allowed under the access policy");
    }
    // …in a row filter, too.
    let file = f.db.file.clone();
    let e = Enforcer::compile(
        &Policy::parse(r#"{"tables": {"sales": {"rows": "stats(region) IS NOT NULL"}}}"#).unwrap(),
        &mut |sql: &str| rows_as_text(&file, sql),
        "snap",
    )
    .unwrap_err();
    assert!(e.contains("not allowed"), "{e}");
}
