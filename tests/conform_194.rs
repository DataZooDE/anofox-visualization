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
    conform_with(s, views, db, extra, DASHBOARD)
}

fn conform_with(
    s: &Scratch,
    views: &str,
    db: &Path,
    extra: &[&str],
    dashboard: &str,
) -> (i32, serde_json::Value) {
    let contract = s.write("contract.sql", CONTRACT);
    let views = s.write("views.sql", views);
    let dash = s.write("dash.sql", dashboard);
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

/// #978: every way the review found to fill a scope column with a constant
/// is refused (by the classifier or the scope check), and a shadowing CTE
/// ends in a report, not a stack overflow.
#[test]
fn scope_bypasses_are_refused() {
    if !have_duckdb("scope_bypasses_are_refused") {
        return;
    }
    let s = Scratch::new("bypass");
    let (_, renamed) = sources(&s);
    let deliveries = "CREATE VIEW deliveries AS SELECT code AS supplier_id, termin AS due, verzug::INTEGER AS days_late, gebiet AS region FROM source.lieferungen;\n";
    let scope = ["--scope", "region=north,south"];
    let refused = |c: &[String]| {
        c.iter()
            .any(|x| x == "conform/scope-not-source" || x == "mapping/refused")
    };
    for body in [
        // 1. a * with REPLACE
        "SELECT code AS supplier_id, firma AS name, * EXCLUDE (code, firma) REPLACE ('north' AS gebiet) RENAME (gebiet AS region) FROM source.lieferanten",
        "SELECT * REPLACE ('north' AS region) FROM (SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten)",
        // 2. a scalar subquery; a positional rename over a constant
        "SELECT code AS supplier_id, firma AS name, (SELECT 'north') AS region FROM source.lieferanten",
        "WITH c AS (SELECT 'north', code, firma FROM source.lieferanten) SELECT code AS supplier_id, firma AS name, region FROM c AS c(region, code, firma)",
        // 3. a lambda parameter; a 3-part name
        "SELECT code AS supplier_id, firma AS name, list_transform(['north'], lambda gebiet: gebiet)[1] AS region FROM source.lieferanten",
        // 4. a later UNION branch puts a constant in the scope column
        "SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten UNION ALL SELECT code, gebiet, 'north' FROM source.lieferanten",
    ] {
        let views = format!("CREATE VIEW suppliers AS {body};\n{deliveries}");
        let (code, v) = conform(&s, &views, &renamed, &scope);
        assert_eq!(code, 1, "{body}\n{v}");
        assert!(refused(&codes(&v)), "{body}\n{v}");
    }
    // 5. a nested CTE shadowing the outer one: a report, not a crash
    let views = format!(
        "CREATE VIEW suppliers AS WITH x AS (SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten) SELECT * FROM (WITH x AS (FROM x) SELECT * FROM x) t;\n{deliveries}"
    );
    let (code, v) = conform(&s, &views, &renamed, &scope);
    assert!(code == 0 || code == 1, "{v}");
    // the plain forms still conform: a CAST of a source column, a plain *,
    // a positional UNION of source columns
    for body in [
        "SELECT code AS supplier_id, firma AS name, gebiet::VARCHAR AS region FROM source.lieferanten",
        "SELECT * FROM (SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten)",
        "SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten WHERE gebiet = 'north' UNION ALL SELECT code, firma, gebiet FROM source.lieferanten WHERE gebiet <> 'north'",
    ] {
        let views = format!("CREATE VIEW suppliers AS {body};\n{deliveries}");
        let (code, v) = conform(&s, &views, &renamed, &scope);
        assert_eq!(code, 0, "{body}\n{v}");
    }
}

/// #979: no silent "conforms" — a mixed-case contract table is still
/// checked, a scope value outside the list is found, and a view that fails
/// when read is a finding (exit 1), not a usage error.
#[test]
fn no_false_conforms() {
    if !have_duckdb("no_false_conforms") {
        return;
    }
    let s = Scratch::new("false");
    let (_, renamed) = sources(&s);
    let deliveries = "CREATE VIEW deliveries AS SELECT code AS supplier_id, termin AS due, verzug::INTEGER AS days_late, gebiet AS region FROM source.lieferungen;\n";
    let suppliers = "CREATE VIEW suppliers AS SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten;\n";
    // a contract table written in mixed case, a view missing its required column
    let contract = s.write(
        "mixed.sql",
        &CONTRACT.replace("CREATE TABLE suppliers", "CREATE TABLE Suppliers"),
    );
    let views = s.write("v.sql", &format!("CREATE VIEW suppliers AS SELECT code AS supplier_id, gebiet AS region FROM source.lieferanten;\n{deliveries}"));
    let out = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .args([
            "--conform",
            contract.to_str().unwrap(),
            "--views",
            views.to_str().unwrap(),
            "--db",
            renamed.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1), "{v}");
    assert!(
        codes(&v).contains(&"conform/missing-column".to_string()),
        "{v}"
    );
    // a value outside the operator's list
    let (code, v) = conform(
        &s,
        &format!("{suppliers}{deliveries}"),
        &renamed,
        &["--scope", "region=north"],
    );
    assert_eq!(code, 1, "{v}");
    assert!(
        codes(&v).contains(&"conform/scope-values".to_string()),
        "{v}"
    );
    // a view that binds but fails when read
    let bad = format!("CREATE VIEW suppliers AS SELECT code AS supplier_id, firma AS name, (gebiet::INTEGER)::VARCHAR AS region FROM source.lieferanten;\n{deliveries}");
    let (code, v) = conform(&s, &bad, &renamed, &["--scope", "region=north,south"]);
    assert_eq!(code, 1, "{v}");
    assert!(codes(&v).contains(&"conform/error".to_string()), "{v}");
}

/// #980: a mapping that would run for ever is stopped by the per-check
/// deadline and reported, not left hanging.
#[test]
fn a_runaway_mapping_is_stopped() {
    if !have_duckdb("a_runaway_mapping_is_stopped") {
        return;
    }
    let s = Scratch::new("runaway");
    let (_, renamed) = sources(&s);
    let suppliers = "CREATE VIEW suppliers AS SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten;\n";
    let runaway = format!("{suppliers}CREATE VIEW deliveries AS SELECT d.code AS supplier_id, d.termin AS due, d.verzug::INTEGER AS days_late, d.gebiet AS region FROM source.lieferungen d, range(1000000000000) r;\n");
    let t0 = std::time::Instant::now();
    let (code, v) = conform(
        &s,
        &runaway,
        &renamed,
        &[
            "--scope",
            "region=north,south",
            "--timeout-s",
            "3",
            "--memory-limit",
            "1GB",
        ],
    );
    assert_eq!(code, 1, "{v}");
    assert!(codes(&v).contains(&"conform/error".to_string()), "{v}");
    assert!(v.to_string().contains("longer than 3 s"), "{v}");
    assert!(t0.elapsed().as_secs() < 120, "took {:?}", t0.elapsed());
}

#[test]
fn second_review_bypasses_are_refused() {
    // #984
    if !have_duckdb("second_review_bypasses_are_refused") {
        return;
    }
    let s = Scratch::new("bypass2");
    let (_, renamed) = sources(&s);
    let deliveries = "CREATE VIEW deliveries AS SELECT code AS supplier_id, termin AS due, verzug::INTEGER AS days_late, gebiet AS region FROM source.lieferungen;\n";
    let scope = ["--scope", "region=north,south"];
    // 1. a * EXCLUDE that drops the scope column, then a literal under its name
    let views = format!("CREATE VIEW suppliers AS SELECT * EXCLUDE (region), 'north' AS region FROM (SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten);\n{deliveries}");
    let (code, v) = conform(&s, &views, &renamed, &scope);
    assert_eq!(code, 1, "{v}");
    assert!(
        codes(&v).contains(&"conform/scope-not-source".to_string()),
        "{v}"
    );
    // …while an EXCLUDE of another column still passes the scope column on
    let views = format!("CREATE VIEW suppliers AS SELECT * EXCLUDE (code) FROM (SELECT code, code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten);\n{deliveries}");
    let (code, v) = conform(&s, &views, &renamed, &scope);
    assert_eq!(code, 0, "{v}");
}

#[test]
fn a_cte_reading_a_later_sibling_is_refused() {
    // #984
    if !have_duckdb("a_cte_reading_a_later_sibling_is_refused") {
        return;
    }
    let s = Scratch::new("fwdcte");
    let (_, renamed) = sources(&s);
    let suppliers = "CREATE VIEW suppliers AS SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten;\n";
    let scope = ["--scope", "region=north,south"];
    // 2. a CTE reading a later sibling's name reads the catalog (another
    //    view), not the CTE: refused by the classifier
    let views = format!("{suppliers}CREATE VIEW deliveries AS WITH a AS (SELECT * FROM suppliers), suppliers AS (SELECT * FROM source.lieferungen) SELECT a.supplier_id, DATE '2026-01-01' AS due, 0 AS days_late, a.region FROM a;\n");
    let (code, v) = conform(&s, &views, &renamed, &scope);
    assert_eq!(code, 1, "{v}");
    assert!(codes(&v).contains(&"mapping/refused".to_string()), "{v}");
}

#[test]
fn a_same_named_table_in_another_schema_lends_no_columns() {
    // #984
    if !have_duckdb("a_same_named_table_in_another_schema_lends_no_columns") {
        return;
    }
    let s = Scratch::new("schemas");
    let (_, renamed) = sources(&s);
    let suppliers = "CREATE VIEW suppliers AS SELECT code AS supplier_id, firma AS name, gebiet AS region FROM source.lieferanten;\n";
    let deliveries = "CREATE VIEW deliveries AS SELECT code AS supplier_id, termin AS due, verzug::INTEGER AS days_late, gebiet AS region FROM source.lieferungen;\n";
    let scope = ["--scope", "region=north,south"];
    // 3. a same-named table in another schema doesn't lend its columns
    let multi = s.0.join("multi.duckdb");
    duckdb(
        &multi,
        &format!("CREATE SCHEMA eu; CREATE TABLE eu.lieferanten AS SELECT code, firma, gebiet AS region FROM (VALUES {DATA}) t(code, firma, gebiet, note);
                  CREATE TABLE main.lieferanten AS SELECT code AS supplier_id, firma AS name FROM (VALUES {DATA}) t(code, firma, gebiet, note);
                  CREATE TABLE lieferungen AS SELECT * FROM (VALUES {DELIVERIES}) t(code, termin, verzug, gebiet);"),
    );
    let views = format!("CREATE VIEW suppliers AS SELECT *, 'north' AS region FROM source.lieferanten;\n{deliveries}");
    let (code, v) = conform(&s, &views, &multi, &scope);
    assert_eq!(code, 1, "{v}");
    assert!(
        codes(&v).contains(&"conform/scope-not-source".to_string()),
        "{v}"
    );
    // 4. an empty --scope list is a usage error, not "allow everything"
    let views = format!("{suppliers}{deliveries}");
    for bad in ["region=", "region=,"] {
        let out = Command::new(env!("CARGO_BIN_EXE_dashboard"))
            .args(["--conform"])
            .arg(s.write("contract.sql", CONTRACT))
            .arg("--views")
            .arg(s.write("views.sql", &views))
            .arg("--db")
            .arg(&renamed)
            .args(["--scope", bad])
            .env("DATAZOO_DISABLE_TELEMETRY", "1")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "--scope {bad}");
    }
}

#[test]
fn a_question_is_bound_with_its_inputs_defaults() {
    // #985: COLUMNS(getvariable('dim')) binds only with the input's default
    if !have_duckdb("a_question_is_bound_with_its_inputs_defaults") {
        return;
    }
    let s = Scratch::new("qdefaults");
    let (_, renamed) = sources(&s);
    let dash = "-- @title Suppliers by a column
-- @vars dim
SELECT 'name' AS dim ::DROPDOWN;
-- @question Which suppliers are there by {dim}?
SELECT COLUMNS(getvariable('dim')) ::TABLE FROM suppliers ORDER BY 1;
";
    let (code, v) = conform_with(
        &s,
        RENAMED,
        &renamed,
        &["--scope", "region=north,south"],
        dash,
    );
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["questions"][0]["available"], true, "{v}");
}

#[test]
fn a_cte_chain_is_bounded_and_a_failed_setup_cleans_up() {
    // #986
    if !have_duckdb("a_cte_chain_is_bounded_and_a_failed_setup_cleans_up") {
        return;
    }
    let s = Scratch::new("chain");
    let (_, renamed) = sources(&s);
    // 8. each CTE reads the one before four times, unqualified: ~4^16
    //    resolver steps without a budget (DISTINCT keeps it two rows)
    let mut ctes =
        vec!["a0 AS (SELECT DISTINCT gebiet AS region FROM source.lieferanten)".to_string()];
    for i in 1..=16 {
        let p = i - 1;
        ctes.push(format!("a{i} AS (SELECT DISTINCT region FROM a{p} x1 JOIN a{p} x2 USING (region) JOIN a{p} x3 USING (region) JOIN a{p} x4 USING (region))"));
    }
    let views = format!(
        "CREATE VIEW suppliers AS WITH {} SELECT l.code AS supplier_id, l.firma AS name, region FROM source.lieferanten l JOIN a16 ON a16.region = l.gebiet;\nCREATE VIEW deliveries AS SELECT code AS supplier_id, termin AS due, verzug::INTEGER AS days_late, gebiet AS region FROM source.lieferungen;\n",
        ctes.join(", ")
    );
    let t0 = std::time::Instant::now();
    let (code, v) = conform(
        &s,
        &views,
        &renamed,
        &["--scope", "region=north,south", "--timeout-s", "60"],
    );
    assert!(code == 0 || code == 1, "{v}");
    assert!(t0.elapsed().as_secs() < 60, "took {:?}", t0.elapsed());
    // 9. a build-instance error exits 2 and leaves no build directory
    let tmp = s.0.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .args(["--conform"])
        .arg(s.write("contract.sql", CONTRACT))
        .arg("--views")
        .arg(s.write("views.sql", RENAMED))
        .arg("--db")
        .arg(s.write("not-a-database.duckdb", "plain text"))
        .env("TMPDIR", &tmp)
        .env("DATAZOO_DISABLE_TELEMETRY", "1")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_dir(&tmp).unwrap().count(),
        0,
        "a build directory was left behind"
    );
}
