//! The `::PAGED` column profile (`page.profile`) on a real DuckDB: golden rows
//! for a fixture with NULLs, ties, numbers stored as text, non-finite values
//! and a constant column; and a profile of one column is exactly that column's
//! part of a profile of several (the browser asks for one column per request,
//! #118). Skips without a `duckdb` CLI (fails without one on CI).
mod common;

use anofox_visualization::host::serving::{page_sql_inline, parse_page};
use std::process::Command;

const BASE: &str = "SELECT * FROM (VALUES
  (1, 'EU', 10.0, '3', DATE '2024-01-01', 7),
  (2, 'EU', 20.0, 'x', DATE '2024-01-01', 7),
  (3, 'US', NULL, '1e999', DATE '2024-01-02', 7),
  (4, 'US', 40.0, 'nan', NULL, 7),
  (5, NULL, 50.0, '-2.5', DATE '2024-01-03', 7),
  (6, 'APAC', 100.0, NULL, DATE '2024-01-03', 7),
  (7, 'MEA', -5.0, '3', DATE '2024-01-04', NULL)
) t(id, region, amt, txt, day, k)";

const COLS: [&str; 6] = ["id", "region", "amt", "txt", "day", "k"];

/// The profile rows as CSV (`c,t,k,v,n`), or `None` without a duckdb CLI.
fn profile(cols: &[&str]) -> Option<String> {
    let page = parse_page(&serde_json::json!({ "profile": cols })).unwrap();
    let sql = page_sql_inline(BASE, &page);
    let out = Command::new("duckdb")
        .args(["-csv", "-noheader", "-c", &sql])
        .output()
        .ok()?;
    assert!(
        out.status.success(),
        "{}\n{sql}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn profile_rows_are_golden() {
    let Some(all) = profile(&COLS) else {
        return common::skip_without("duckdb", "profile_duckdb");
    };
    // c = column index; 'h' = histogram bin k of the numeric values; 't' = a
    // top value v; n = count. Bins: floor((x - lo) / (hi - lo) * 10), at
    // most 9; 'nan' and '1e999' are not finite, so not numbers.
    let golden = "\
0,h,0,NULL,1
0,h,1,NULL,1
0,h,3,NULL,1
0,h,5,NULL,1
0,h,6,NULL,1
0,h,8,NULL,1
0,h,9,NULL,1
0,t,NULL,1,1
0,t,NULL,2,1
0,t,NULL,3,1
1,t,NULL,EU,2
1,t,NULL,US,2
1,t,NULL,APAC,1
2,h,0,NULL,1
2,h,1,NULL,1
2,h,2,NULL,1
2,h,4,NULL,1
2,h,5,NULL,1
2,h,9,NULL,1
2,t,NULL,-5.0,1
2,t,NULL,10.0,1
2,t,NULL,100.0,1
3,h,0,NULL,1
3,h,9,NULL,2
3,t,NULL,3,2
3,t,NULL,-2.5,1
3,t,NULL,1e999,1
4,t,NULL,2024-01-01,2
4,t,NULL,2024-01-03,2
4,t,NULL,2024-01-02,1
5,h,0,NULL,6
5,t,NULL,7,6
";
    assert_eq!(all, golden);
}

#[test]
fn one_column_is_its_part_of_several() {
    let Some(all) = profile(&COLS) else {
        return common::skip_without("duckdb", "profile_duckdb");
    };
    for (i, col) in COLS.iter().enumerate() {
        let one = profile(&[col]).unwrap();
        let part: String = all
            .lines()
            .filter_map(|l| l.strip_prefix(&format!("{i},")))
            .map(|l| format!("0,{l}\n"))
            .collect();
        assert_eq!(one, part, "column {col}");
    }
}
