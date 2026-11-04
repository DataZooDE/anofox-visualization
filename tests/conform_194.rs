//! `dashboard --conform` (#194 A3, plan-dashboard-to-agent §4): a customer's
//! mapping onto a canonical contract. The proof the plan asks for: a renamed
//! synthetic source, mapped, gives the dashboard's panels the same numbers as
//! a pass-through mapping of the same data; and a mapping with a macro, a
//! `query_table` call, a constant scope column without its marker, a foreign
//! database or a statement other than CREATE VIEW is refused. Needs the
//! `duckdb` CLI (skips without it locally).
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn have_duckdb(test: &str) -> bool {
    let ok = Command::new("duckdb").arg("-version").output().is_ok();
    if !ok {
        common::skip_without("duckdb", test);
    }
    ok
}

/// A scratch directory under `std::env::temp_dir()` (TMPDIR), removed on drop.
struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_194_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Scratch(d)
    }
    fn write(&self, name: &str, text: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const CONTRACT: &str = "\
-- Suppliers and their deliveries.
CREATE TABLE suppliers (
    supplier_id VARCHAR PRIMARY KEY, -- the supplier's code
    name VARCHAR NOT NULL,
    region VARCHAR NOT NULL, -- @scope region
    rating DOUBLE -- @optional the buyer's rating
);
CREATE TABLE deliveries (
    supplier_id VARCHAR NOT NULL REFERENCES suppliers (supplier_id),
    due DATE NOT NULL,
    days_late INTEGER,
    region VARCHAR NOT NULL -- @scope region
);
";

const DATA: &str = "\
('S1', 'Alpha', 'north', 4.5), ('S2', 'Beta', 'south', 3.0), ('S3', 'Gamma', 'north', NULL)";
const DELIVERIES: &str = "\
('S1', DATE '2026-01-05', 2, 'north'), ('S2', DATE '2026-01-07', 0, 'south'),
('S1', DATE '2026-02-01', 5, 'north'), ('S3', DATE '2026-02-03', 1, 'north'),
('S2', DATE '2026-02-10', 4, 'south')";

const DASHBOARD: &str = "\
-- @title Supplier reliability
-- @question How late are our suppliers?
SELECT s.name ::XAXIS, round(avg(d.days_late), 2) AS late ::BARCHART FROM deliveries d JOIN suppliers s USING (supplier_id) GROUP BY 1 ORDER BY 1;
-- @question Which suppliers have the best rating?
SELECT name ::XAXIS, rating ::BARCHART FROM suppliers ORDER BY 1;
";

/// The two panels' SQL (role casts gone), as a host runs them.
const PANELS: [&str; 2] = [
    "SELECT s.name, round(avg(d.days_late), 2) AS late FROM deliveries d JOIN suppliers s USING (supplier_id) GROUP BY 1 ORDER BY 1",
    "SELECT name, rating FROM suppliers ORDER BY 1",
];

fn duckdb(db: &Path, sql: &str) {
    let out = Command::new("duckdb")
        .arg(db)
        .arg("-c")
        .arg(sql)
        .env("DATAZOO_DISABLE_TELEMETRY", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The source under the contract's own names, and one renamed (German
/// columns, other table names, the rating in another table) — same rows.
fn sources(s: &Scratch) -> (PathBuf, PathBuf) {
    let plain = s.0.join("plain.duckdb");
    duckdb(
        &plain,
        &format!(
            "CREATE TABLE suppliers AS SELECT supplier_id, name, region, rating::DOUBLE AS rating FROM (VALUES {DATA}) t(supplier_id, name, region, rating);
             CREATE TABLE deliveries AS SELECT * FROM (VALUES {DELIVERIES}) t(supplier_id, due, days_late, region);"
        ),
    );
    let renamed = s.0.join("renamed.duckdb");
    duckdb(
        &renamed,
        &format!(
            "CREATE TABLE lieferanten AS SELECT code, firma, gebiet FROM (VALUES {DATA}) t(code, firma, gebiet, note);
             CREATE TABLE bewertung AS SELECT code, note FROM (VALUES {DATA}) t(code, firma, gebiet, note);
             CREATE TABLE lieferungen AS SELECT * FROM (VALUES {DELIVERIES}) t(code, termin, verzug, gebiet);"
        ),
    );
    (plain, renamed)
}

const PASS_THROUGH: &str = "\
CREATE VIEW suppliers AS SELECT supplier_id, name, region, rating FROM source.suppliers;
CREATE VIEW deliveries AS SELECT supplier_id, due, days_late, region FROM source.deliveries;
";

const RENAMED: &str = "\
-- the integration engineer's mapping for this customer
CREATE VIEW suppliers AS
    SELECT l.code AS supplier_id, l.firma AS name, l.gebiet AS region, b.note::DOUBLE AS rating
    FROM source.lieferanten l LEFT JOIN source.bewertung b USING (code);
CREATE VIEW deliveries AS
    SELECT code AS supplier_id, termin AS due, verzug::INTEGER AS days_late, gebiet AS region
    FROM source.lieferungen;
";

fn conform(s: &Scratch, views: &str, db: &Path, extra: &[&str]) -> (i32, serde_json::Value) {
    let contract = s.write("contract.sql", CONTRACT);
    let views = s.write("views.sql", views);
    let dash = s.write("dash.sql", DASHBOARD);
    let out = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .arg("--conform")
        .arg(&contract)
        .arg("--views")
        .arg(&views)
        .arg("--db")
        .arg(db)
        .arg("--dashboard")
        .arg(&dash)
        .args(extra)
        .arg("--json")
        .env("DATAZOO_DISABLE_TELEMETRY", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(text.trim()).unwrap_or_else(|e| {
        panic!(
            "--conform --json: {e}\nstdout: {text}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code().unwrap_or(-1), v)
}

/// A panel's rows over a mapping, built as the build instance builds it.
fn panel_rows(db: &Path, views: &str, panel: &str) -> String {
    let sql = format!(
        "ATTACH '{}' AS source (READ_ONLY); SET enable_external_access = false; \
         SET lock_configuration = true; {views} {panel};",
        db.display()
    );
    let out = Command::new("duckdb")
        .arg(":memory:")
        .arg("-json")
        .arg("-c")
        .arg(&sql)
        .env("DATAZOO_DISABLE_TELEMETRY", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn codes(v: &serde_json::Value) -> Vec<String> {
    v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "error")
        .map(|f| f["code"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_renamed_source_gives_the_same_numbers_as_a_pass_through() {
    if !have_duckdb("a_renamed_source_gives_the_same_numbers_as_a_pass_through") {
        return;
    }
    let s = Scratch::new("same");
    let (plain, renamed) = sources(&s);
    let scope = ["--scope", "region=north,south"];
    for (views, db) in [(PASS_THROUGH, &plain), (RENAMED, &renamed)] {
        let (code, v) = conform(&s, views, db, &scope);
        assert_eq!(code, 0, "{v}");
        assert_eq!(v["ok"], true, "{v}");
        let qs = v["questions"].as_array().unwrap();
        assert_eq!(qs.len(), 2, "{v}");
        assert!(qs.iter().all(|q| q["available"] == true), "{v}");
    }
    for panel in PANELS {
        let a = panel_rows(&plain, PASS_THROUGH, panel);
        let b = panel_rows(&renamed, RENAMED, panel);
        assert!(a.len() > 10, "{a}");
        assert_eq!(a, b, "{panel}");
    }
}

#[test]
fn a_missing_optional_column_turns_its_question_off() {
    if !have_duckdb("a_missing_optional_column_turns_its_question_off") {
        return;
    }
    let s = Scratch::new("optional");
    let (_, renamed) = sources(&s);
    let no_rating = "\
CREATE VIEW suppliers AS SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten;
CREATE VIEW deliveries AS SELECT code AS supplier_id, termin AS due, verzug::INTEGER AS days_late, gebiet AS region FROM source.lieferungen;
";
    let (code, v) = conform(&s, no_rating, &renamed, &["--scope", "region=north,south"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["missing_optional"]["suppliers"][0], "rating", "{v}");
    let qs = v["questions"].as_array().unwrap();
    assert_eq!(qs[0]["available"], true, "{v}");
    assert_eq!(qs[1]["available"], false, "{v}");
    assert!(qs[1]["reason"].as_str().unwrap().contains("rating"), "{v}");
}

#[test]
fn inadmissible_mappings_are_refused() {
    if !have_duckdb("inadmissible_mappings_are_refused") {
        return;
    }
    let s = Scratch::new("refused");
    let (_, renamed) = sources(&s);
    let deliveries = "CREATE VIEW deliveries AS SELECT code AS supplier_id, termin AS due, verzug::INTEGER AS days_late, gebiet AS region FROM source.lieferungen;\n";
    let suppliers = "CREATE VIEW suppliers AS SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten;\n";
    let scope = ["--scope", "region=north,south"];
    // a macro (a built-in table macro), a query_table call, a constant
    // scope column without its marker
    for (views, want) in [
        (
            format!("CREATE VIEW suppliers AS SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten, duckdb_types() LIMIT 0;\n{deliveries}"),
            "mapping/refused",
        ),
        (
            format!("{suppliers}CREATE VIEW deliveries AS SELECT * FROM query_table('source.lieferungen');\n"),
            "mapping/refused",
        ),
        (
            format!("CREATE VIEW suppliers AS SELECT code AS supplier_id, firma AS name, 'north' AS region FROM source.lieferanten;\n{deliveries}"),
            "conform/scope-not-source",
        ),
        (
            format!("CREATE VIEW suppliers AS SELECT code AS supplier_id, firma AS name, gebiet AS region FROM memory.main.x;\n{deliveries}"),
            "mapping/refused",
        ),
    ] {
        let (code, v) = conform(&s, &views, &renamed, &scope);
        assert_eq!(code, 1, "{views}\n{v}");
        assert!(codes(&v).contains(&want.to_string()), "{views}\n{v}");
    }
    // a statement other than CREATE VIEW over a contract table
    for views in [
        format!("CREATE MACRO m() AS 1;\n{suppliers}{deliveries}"),
        format!("ATTACH 'other.duckdb' AS other;\n{suppliers}{deliveries}"),
        format!("COPY (SELECT 1) TO 'x.csv';\n{suppliers}{deliveries}"),
        format!("{suppliers}{deliveries}CREATE VIEW helper AS SELECT 1;\n"),
    ] {
        let (code, v) = conform(&s, &views, &renamed, &scope);
        assert_eq!(code, 1, "{views}\n{v}");
        assert_eq!(codes(&v), ["mapping/parse"], "{views}\n{v}");
    }
    // a view ending in a line comment is still one statement
    let (code, v) = conform(
        &s,
        &format!(
            "{}-- the last line is a comment\n;\n{deliveries}",
            suppliers.trim_end().trim_end_matches(';')
        ),
        &renamed,
        &scope,
    );
    assert_eq!(code, 0, "{v}");
    // the constant, marked: a note, not a refusal
    let marked = format!("-- @scope region constant\nCREATE VIEW suppliers AS SELECT code AS supplier_id, firma AS name, 'north' AS region FROM source.lieferanten;\n{deliveries}");
    let (code, v) = conform(&s, &marked, &renamed, &scope);
    assert_eq!(code, 0, "{v}");
    assert!(
        !codes(&v).contains(&"conform/scope-not-source".to_string()),
        "{v}"
    );
    // a scope value the operator doesn't allow
    let (code, v) = conform(
        &s,
        &format!("{suppliers}{deliveries}"),
        &renamed,
        &["--scope", "region=north"],
    );
    assert_eq!(code, 1, "{v}");
}
