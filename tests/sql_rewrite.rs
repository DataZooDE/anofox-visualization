use duckplot::sql;

// An alias/column containing a SQL keyword prefix + `_` must not be mistaken for
// that keyword when locating the top-level FROM (regression: `from_day`).
#[test]
fn identifier_with_keyword_prefix_is_not_a_boundary() {
    let (sql, roles) = sql::rewrite("SELECT min(day) AS from_day, max(day) AS to_day ::DATERANGE FROM events");
    assert!(!sql.contains("::"), "cast not stripped: {sql}");
    assert!(sql.contains("AS from_day") && sql.contains("AS to_day") && sql.contains("FROM events"), "{sql}");
    assert!(!sql.contains("AS AS") && !sql.contains("c0"), "mangled: {sql}");
    assert_eq!(roles.len(), 1);
}

#[test]
fn plain_measure_cast_still_rewrites() {
    let (sql, roles) = sql::rewrite("SELECT week::XAXIS, sum(n)::BARCHART FROM t GROUP BY ALL");
    assert!(sql.contains("AS c0") && sql.contains("AS c1"));
    assert_eq!(roles.len(), 2);
}
