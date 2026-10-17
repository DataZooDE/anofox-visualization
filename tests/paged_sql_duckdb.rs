//! The `::PAGED` request SQL (rows, count, column filters, stats, profile, distinct) runs
//! on a real DuckDB and returns what it should. Skips without a `duckdb` CLI
//! (fails without one on CI, see `common::skip_without`).
mod common;

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
        common::skip_without("duckdb", "paged_requests_run_on_duckdb");
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
    // profile: a 10-bin histogram of the numbers and the top 3 values, per column
    let pr = run(&format!(r#"{{"profile":["amount","region","note"],{f}}}"#)).unwrap();
    let rows = pr.as_array().unwrap();
    let of = |c: i64, t: &str| -> Vec<&serde_json::Value> {
        rows.iter().filter(|r| r["c"] == c && r["t"] == t).collect()
    };
    let hist = of(0, "h");
    assert_eq!(
        hist.iter().map(|r| r["n"].as_i64().unwrap()).sum::<i64>(),
        27
    );
    assert!(hist
        .iter()
        .all(|r| (0..=9).contains(&r["k"].as_i64().unwrap())));
    assert_eq!(hist.first().unwrap()["k"], 0);
    assert_eq!(hist.last().unwrap()["k"], 9);
    // the region filter leaves one value; text has no histogram
    let top = of(1, "t");
    assert_eq!(top.len(), 1);
    assert_eq!(
        (top[0]["v"].as_str(), top[0]["n"].as_i64()),
        (Some("EU"), Some(27))
    );
    assert!(of(1, "h").is_empty());
    // nulls are not counted among the top values
    assert_eq!(of(2, "t").len(), 3);
    assert!(of(2, "t").iter().all(|r| !r["v"].is_null()));
    // a constant column falls into one bin
    let c =
        run(r#"{"profile":["region"],"filters":[{"col":"id","op":"eq","value":"3"}]}"#).unwrap();
    assert_eq!(c.as_array().unwrap().len(), 1);
    let c = run(r#"{"profile":["id"],"filters":[{"col":"id","op":"eq","value":"3"}]}"#).unwrap();
    assert_eq!(c[0]["k"], 0);
}

/// Text that merely reads as a huge number — an md5 like `1e99…` — must not
/// turn the stats / profile into ±inf (bare `Infinity` is not JSON).
#[test]
fn stats_ignore_text_that_reads_as_infinity() {
    let base = "SELECT * FROM (VALUES ('12e45678901234567890123456789012', 1), \
                ('9e999', 2), ('nan', 3), ('inf', 4), ('5', 5)) t(h, k)";
    let page = |j: &str| {
        let p = parse_page(&serde_json::from_str(j).unwrap()).unwrap();
        duck(&page_sql_inline(base, &p))
    };
    let Some(out) = page(r#"{"stats":["h","k"]}"#) else {
        common::skip_without("duckdb", "stats_ignore_text_that_reads_as_infinity");
        return;
    };
    let s: serde_json::Value = serde_json::from_str(&out).expect("stats are valid JSON");
    let s = &s[0];
    assert_eq!(s["n0"], 5);
    // only "5" is a finite number
    assert_eq!(
        (s["s0"].as_f64(), s["lo0"].as_f64(), s["hi0"].as_f64()),
        (Some(5.0), Some(5.0), Some(5.0))
    );
    assert_eq!(s["s1"].as_f64(), Some(15.0));
    let out = page(r#"{"profile":["h"]}"#).unwrap();
    let rows: serde_json::Value = serde_json::from_str(&out).expect("profile is valid JSON");
    let hist: Vec<_> = rows
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["t"] == "h")
        .collect();
    assert_eq!(hist.len(), 1);
    assert_eq!(hist[0]["n"], 1);
    // a numeric filter skips them too
    let out = page(r#"{"count":true,"filters":[{"col":"h","op":"gt","value":0}]}"#).unwrap();
    let n: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(n[0]["n"], 1);
}

/// The profile groups by position: a table whose columns are named like the
/// profile's own aliases (`k`, `v`, `n`) still profiles (it used to bind
/// `GROUP BY v` to the table's column).
#[test]
fn profile_of_columns_named_like_its_aliases() {
    let base =
        "SELECT 'p' || (i % 4) AS code, i % 9 AS k, i * 0.5 AS v, i AS n FROM range(40) t(i)";
    let p = parse_page(&serde_json::json!({ "profile": ["code", "k", "v", "n"] })).unwrap();
    let Some(out) = duck(&page_sql_inline(base, &p)) else {
        common::skip_without("duckdb", "profile_of_columns_named_like_its_aliases");
        return;
    };
    let rows: serde_json::Value = serde_json::from_str(&out).unwrap();
    let rows = rows.as_array().unwrap();
    for c in 0..4 {
        let top: Vec<_> = rows
            .iter()
            .filter(|r| r["c"] == c && r["t"] == "t")
            .collect();
        assert_eq!(top.len(), 3, "column {c}: {rows:?}");
    }
    let hist: i64 = rows
        .iter()
        .filter(|r| r["c"] == 2 && r["t"] == "h")
        .map(|r| r["n"].as_i64().unwrap())
        .sum();
    assert_eq!(hist, 40);
}

#[test]
fn an_info_column_is_omitted_from_rows_and_search() {
    // A ::PAGED table's ::INFO column is the panel's definition (#152): with
    // `omit` it is in no answer, and the search never matches its text.
    let base = "SELECT i AS id, 'Revenue net of returns' AS about FROM range(1, 6) t(i)";
    let run = |page: &str| {
        let p = parse_page(&serde_json::from_str(page).unwrap()).unwrap();
        duck(&page_sql_inline(base, &p)).map(|out| {
            serde_json::from_str::<serde_json::Value>(if out.trim().is_empty() {
                "[]"
            } else {
                &out
            })
            .unwrap()
        })
    };
    let Some(rows) = run(r#"{"limit":10,"omit":["about"]}"#) else {
        common::skip_without("duckdb", "an_info_column_is_omitted_from_rows_and_search");
        return;
    };
    assert_eq!(rows.as_array().unwrap().len(), 5);
    assert!(rows[0].get("about").is_none(), "{rows}");
    let n = run(r#"{"count":true,"filter":"returns","omit":["about"]}"#).unwrap();
    assert_eq!(n[0]["n"], 0);
    let n = run(r#"{"count":true,"filter":"returns"}"#).unwrap();
    assert_eq!(n[0]["n"], 5);
    let s = run(r#"{"stats":["id"],"omit":["about"]}"#).unwrap();
    assert_eq!(s[0]["n"], 5);
    // a column name is quoted, never interpolated
    let p = parse_page(&serde_json::json!({"limit": 1, "omit": ["a\") FROM x; --"]})).unwrap();
    assert!(page_sql_inline(base, &p).contains(r#"EXCLUDE ("a"") FROM x; --")"#));
    assert!(parse_page(&serde_json::json!({"omit": "about"})).is_err());
    assert!(parse_page(&serde_json::json!({"omit": vec!["c"; 9]})).is_err());
}
