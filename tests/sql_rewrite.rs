use anofox_visualization::sql;

// An alias/column containing a SQL keyword prefix + `_` must not be mistaken for
// that keyword when locating the top-level FROM (regression: `from_day`).
#[test]
fn identifier_with_keyword_prefix_is_not_a_boundary() {
    let (sql, roles, _) =
        sql::rewrite("SELECT min(day) AS from_day, max(day) AS to_day ::DATERANGE FROM events");
    assert!(!sql.contains("::"), "cast not stripped: {sql}");
    assert!(
        sql.contains("AS from_day") && sql.contains("AS to_day") && sql.contains("FROM events"),
        "{sql}"
    );
    assert!(
        !sql.contains("AS AS") && !sql.contains("c0"),
        "mangled: {sql}"
    );
    assert_eq!(roles.len(), 1);
}

#[test]
fn plain_measure_cast_still_rewrites() {
    let (sql, roles, _) = sql::rewrite("SELECT week::XAXIS, sum(n)::BARCHART FROM t GROUP BY ALL");
    assert!(sql.contains("AS c0") && sql.contains("AS c1"));
    assert_eq!(roles.len(), 2);
}

/// #38: a ::MARKDOWN statement's other columns keep their names, so the text's
/// `{{name}}` placeholders can read them — under the reserved `md:` prefix, so
/// they never collide with the positional `c{i}` keys (#138); a computed
/// column without an alias stays positional.
#[test]
fn markdown_value_columns_keep_their_names() {
    let (sql, roles, _) = sql::rewrite(
        "SELECT 'Grew {{growth:pct}}'::MARKDOWN, 100 * (a / b - 1) AS growth, revenue, a + 1 FROM t",
    );
    assert!(sql.contains("AS c0"), "{sql}");
    assert!(sql.contains(r#"100 * (a / b - 1) AS "md:growth""#), "{sql}");
    assert!(sql.contains(r#", revenue AS "md:revenue","#), "{sql}");
    assert!(sql.contains("a + 1 AS c3"), "{sql}");
    assert_eq!(roles.len(), 1);
    // Other statements are unchanged: untagged columns are positional.
    let (sql, _, _) = sql::rewrite("SELECT x::XAXIS, y::BARCHART, z AS extra FROM t");
    assert!(sql.contains("z AS c2"), "{sql}");
}
