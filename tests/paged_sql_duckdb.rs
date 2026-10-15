//! The `::PAGED` request SQL (rows, count, column filters, stats, distinct) runs
//! on a real DuckDB and returns what it should. Skips without a `duckdb` CLI.
use anofox_visualization::host::serving::{page_sql_inline, parse_page};
use std::process::Command;

fn duck(sql: &str) -> Option<String> {
    let out = Command::new("duckdb")
        .args(["-json", "-c", sql])
        .output()
        .ok()?;
    if !out.status.success() {
        panic!(
            "duckdb failed: {}\n{sql}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

const BASE: &str = "SELECT i AS id, ['EU','US','APAC'][1 + i % 3] AS region, i * 1.5 AS amount, \
                    DATE '2026-01-01' + i::INTEGER AS day, CASE WHEN i % 7 = 0 THEN NULL ELSE 'n' || i END AS note \
                    FROM range(1, 101) t(i)";

fn run(page: &str) -> Option<serde_json::Value> {
    let p = parse_page(&serde_json::from_str(page).unwrap()).unwrap();
    let out = duck(&page_sql_inline(BASE, &p))?;
    Some(serde_json::from_str(if out.trim().is_empty() { "[]" } else { &out }).unwrap())
}

#[test]
fn paged_requests_run_on_duckdb() {
    let Some(rows) = run(r#"{"limit":5,"offset":10,"sort":"amount","desc":true}"#) else {
        eprintln!("skipped: no duckdb CLI");
        return;
    };
    assert_eq!(rows.as_array().unwrap().len(), 5);
    assert_eq!(rows[0]["id"], 90);
    // filters: region = EU and amount >= 30 (id >= 20) → ids 21, 24, …, 99
    let f = r#""filters":[{"col":"region","op":"eq","value":"EU"},{"col":"amount","op":"ge","value":30}]"#;
    let n = run(&format!(r#"{{"count":true,{f}}}"#)).unwrap();
    assert_eq!(n[0]["n"], 27);
    // a date column takes a numeric filter without a cast error (it never matches)
    let n = run(r#"{"count":true,"filters":[{"col":"day","op":"gt","value":1}]}"#).unwrap();
    assert_eq!(n[0]["n"], 0);
    // stats over the filtered rows; text columns have counts but no sum
    let s = run(&format!(r#"{{"stats":["amount","note","day"],{f}}}"#)).unwrap();
    let s = &s[0];
    assert_eq!(s["n"], 27);
    assert_eq!(s["n0"], 27);
    assert!(
        (s["s0"].as_f64().unwrap() - (21..=99).step_by(3).map(|i| i as f64 * 1.5).sum::<f64>())
            .abs()
            < 1e-6
    );
    assert_eq!(s["lo0"], 31.5);
    // 27 values 31.5, 36, …, 148.5: the median is the 14th
    assert_eq!(s["md0"], 90.0);
    assert!(s["s1"].is_null());
    // distinct: lists every region although the region filter is on
    let d = run(&format!(r#"{{"distinct":"region",{f}}}"#)).unwrap();
    let vals: Vec<&str> = d
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["v"].as_str().unwrap())
        .collect();
    assert_eq!(vals, vec!["APAC", "EU", "US"]);
    // contains with a quote in the value
    let n =
        run(r#"{"count":true,"filters":[{"col":"note","op":"contains","value":"n1'"}]}"#).unwrap();
    assert_eq!(n[0]["n"], 0);
}
