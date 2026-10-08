//! SQL surgery edge cases: one shared lexer drives statement splitting, item
//! splitting, FROM/clause detection, comment stripping and role detection.

use anofox_visualization::sql::{self, lex};
use anofox_visualization::{Kind, Role};

fn roles_of(stmt: &str) -> Vec<(usize, Role)> {
    sql::rewrite(stmt).1
}

#[test]
fn statement_splitting_table() {
    // (script, expected statement count)
    let cases: &[(&str, usize)] = &[
        ("SELECT 1; SELECT 2", 2),
        ("SELECT ';' AS semi; SELECT 2;", 2),
        ("SELECT 'it''s; fine'; SELECT 2", 2),
        ("SELECT \"we;ird\" FROM t; SELECT 2", 2),
        ("SELECT $$a;b$$; SELECT 2", 2),
        ("SELECT $tag$ a;b $x$ ; $tag$; SELECT 2", 2),
        ("SELECT E'a\\';b'; SELECT 2", 2),
        ("SELECT 1 -- ; not a split\n; SELECT 2", 2),
        ("SELECT 1 /* ; nor ; this */; SELECT 2", 2),
        ("SELECT 1 /* nested /* ; */ still ; comment */; SELECT 2", 2),
        ("SELECT 'unterminated; SELECT 2", 1),
        (";;; SELECT 1 ;;", 1),
        ("", 0),
    ];
    for (script, n) in cases {
        let stmts: Vec<String> = sql::split_statements(&sql::strip_comments(script))
            .into_iter()
            .filter(|s| !s.trim().is_empty())
            .collect();
        assert_eq!(stmts.len(), *n, "{script:?} → {stmts:?}");
    }
}

#[test]
fn comment_stripping_respects_strings() {
    // `---` inside a multi-line ::MARKDOWN string is content, not a comment.
    let md = "SELECT '# Title\n---\nbody -- still body'::MARKDOWN; -- real comment";
    let clean = sql::strip_comments(md);
    assert!(clean.contains("---\nbody -- still body"), "{clean}");
    assert!(!clean.contains("real comment"), "{clean}");
    let p = sql::plan(md);
    assert_eq!(p.len(), 1);
    assert!(p[0].sql.contains("---"), "{}", p[0].sql);
    assert_eq!(p[0].roles, vec![(0, Role::Markdown)]);
    // Block comments vanish; `/*` inside strings stays.
    assert_eq!(
        sql::strip_comments("SELECT '/* x */' /* y */ AS a").trim(),
        "SELECT '/* x */'   AS a"
    );
}

#[test]
fn item_splitting_table() {
    let cases: &[(&str, usize)] = &[
        ("a, b, c", 3),
        ("f(a, b), c", 2),
        ("'a,b', c", 2),
        ("\"x,y\", c", 2),
        ("[1, 2, 3] AS l, c", 2),
        ("{'a': 1, 'b': 2} AS s, c", 2),
        ("$$a,b$$, c", 2),
        ("a -- , b\n, c", 2),
        ("CASE WHEN a IN (1,2) THEN 'x,y' END, c", 2),
    ];
    for (list, n) in cases {
        assert_eq!(sql::split_top_commas(list).len(), *n, "{list:?}");
    }
}

#[test]
fn from_detection_ignores_literals_and_nesting() {
    let (s, roles, _) =
        sql::rewrite("SELECT 'FROM' AS f ::LABEL, (SELECT max(v) FROM x)::METRIC FROM t");
    assert_eq!(roles.len(), 2, "{s}");
    assert!(s.ends_with("FROM t"), "{s}");
    let (s, roles, _) = sql::rewrite("SELECT \"from\"::XAXIS, n::BARCHART FROM t");
    assert_eq!(roles.len(), 2, "{s}");
    // `from_day` is an identifier, not FROM.
    let (s, _, _) = sql::rewrite("SELECT from_day::XAXIS, n::BARCHART FROM t");
    assert!(s.contains("from_day AS c0"), "{s}");
    // No FROM: the list ends at another clause keyword.
    let (s, roles, _) = sql::rewrite("SELECT 'a'::LABEL ORDER BY 1");
    assert_eq!(roles, vec![(0, Role::Label)]);
    assert!(s.contains("ORDER BY 1"), "{s}");
    // A role token that is also a keyword (`::GROUP`) is not a clause boundary.
    assert_eq!(roles_of("SELECT 'Box'::GROUP"), vec![(0, Role::GroupStart)]);
}

#[test]
fn ddl_and_dml_are_never_panels() {
    for stmt in [
        "CREATE TABLE t AS SELECT ts::DATE FROM raw",
        "CREATE OR REPLACE VIEW v AS SELECT name::TEXT, n::NUMERIC FROM raw",
        "INSERT INTO t SELECT x::DATE FROM raw",
        "CREATE TABLE g AS SELECT ST_Point(1,2)::GEOMETRY AS g",
        "CREATE VIEW v AS SELECT x::XAXIS, n::BARCHART FROM t",
        "SET VARIABLE d = DATE '2024-01-01'",
        "COPY (SELECT 1::METRIC) TO 'x.csv'",
    ] {
        let p = sql::plan(stmt);
        assert!(p[0].setup, "{stmt} became a panel");
        assert_eq!(p[0].sql, stmt, "setup SQL must be untouched");
    }
}

#[test]
fn ambiguous_type_tokens() {
    // Standalone input statements (per DOCS): roles.
    assert_eq!(
        roles_of("SELECT DATE '2024-01-01' AS start ::DATE"),
        vec![(0, Role::Input(anofox_visualization::InputKind::Date))]
    );
    assert_eq!(
        roles_of("SELECT 'acme' AS q ::TEXT"),
        vec![(0, Role::Input(anofox_visualization::InputKind::Text))]
    );
    // In a chart panel they are real casts — the cast is kept.
    let (s, roles, _) = sql::rewrite("SELECT day::DATE, n::BARCHART, d::DATE::XAXIS FROM t");
    assert_eq!(
        roles,
        vec![(1, Role::Value(Kind::Bar)), (2, Role::X)],
        "{s}"
    );
    assert!(s.contains("day::DATE AS c0"), "{s}");
    assert!(s.contains("d::DATE AS c2"), "{s}");
    // MAP/GEOMETRY are the map role in a query.
    assert_eq!(
        roles_of("SELECT geom::GEOMETRY, pop::BARCHART FROM c"),
        vec![(0, Role::Geometry), (1, Role::Value(Kind::Bar))]
    );
    assert_eq!(
        roles_of("SELECT geom::MAP FROM c"),
        vec![(0, Role::Geometry)]
    );
    // Inside ::TABLE real casts stay; role casts are stripped.
    let (s, roles, _) =
        sql::rewrite("SELECT name::TEXT AS \"Name\" ::TABLE, n::NUMERIC AS \"N\", v::MONEY FROM t");
    assert!(s.contains("name::TEXT AS \"Name\""), "{s}");
    assert!(s.contains("n::NUMERIC AS \"N\""), "{s}");
    assert!(!s.contains("::MONEY") && !s.contains("::TABLE"), "{s}");
    assert_eq!(roles[0], (0, Role::Table));
    assert!(roles.contains(&(2, Role::Metric(anofox_visualization::MetricFmt::Money))));
}

#[test]
fn aliases_and_star_items() {
    // A non-role aliased column: the alias is replaced, not doubled.
    let (s, _, _) = sql::rewrite("SELECT x::XAXIS, count(*) AS n, y::BARCHART FROM t");
    assert!(s.contains("count(*) AS c1"), "{s}");
    assert!(!s.contains("AS n AS"), "{s}");
    // A role after an alias (any role, not only measures/tables).
    let (s, roles, _) = sql::rewrite("SELECT week AS \"Week\" ::XAXIS, n AS cnt ::BARCHART FROM t");
    assert!(
        s.contains("week AS c0") && !s.contains("\"Week\" AS"),
        "{s}"
    );
    assert_eq!(roles.len(), 2);
    // `*` and friends expand in place, unaliased.
    for star in ["*", "t.*", "* EXCLUDE (a)", "COLUMNS('v.*')", "\"T\".*"] {
        let (s, roles, _) = sql::rewrite(&format!("SELECT {star}, v::BARCHART FROM t"));
        assert!(
            s.contains(&format!("{star}, CAST(v AS DOUBLE) AS c1")),
            "{s}"
        );
        assert!(!s.contains("AS c0"), "{s}");
        assert_eq!(roles, vec![(1, Role::Value(Kind::Bar))]);
    }
}

#[test]
fn leading_with_cte_panels_work() {
    let stmt = "WITH s AS (SELECT week, sum(n)::INT AS n FROM t GROUP BY 1) \
                SELECT week::XAXIS, n::LINECHART FROM s ORDER BY week";
    let p = sql::plan(stmt);
    assert!(!p[0].setup, "CTE panel dropped to setup");
    assert_eq!(p[0].roles, vec![(0, Role::X), (1, Role::Value(Kind::Line))]);
    // The CTE body is untouched; only the main list is rewritten.
    assert!(p[0].sql.contains("sum(n)::INT AS n FROM t"), "{}", p[0].sql);
    assert!(p[0].sql.contains("week AS c0"), "{}", p[0].sql);
    // Recursive / materialized CTEs and nested parens.
    let p = sql::plan(
        "WITH RECURSIVE r(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM r WHERE i < 3), \
         m AS MATERIALIZED (SELECT (SELECT 2) AS z) SELECT i::XAXIS, i::BARCHART FROM r",
    );
    assert_eq!(p[0].roles.len(), 2);
}

#[test]
fn trailing_role_detection() {
    assert_eq!(
        lex::trailing_cast("sum(x) ::BARCHART"),
        Some(("sum(x)", "BARCHART"))
    );
    assert_eq!(lex::trailing_cast("x:: BAR -- c"), Some(("x", "BAR")));
    assert_eq!(lex::trailing_cast("x::DECIMAL(10,2)"), None);
    assert_eq!(lex::trailing_cast("'a::XAXIS'"), None);
    assert_eq!(lex::trailing_cast("x: :XAXIS"), None);
    // Roles parsed from strings / comments never count.
    assert!(roles_of("SELECT 'x::XAXIS' AS a, 1 /* ::BARCHART */ FROM t").is_empty());
}

#[test]
fn iso_timestamps() {
    use sql::parse_iso_epoch as p;
    assert_eq!(p("1970-01-01"), Some(0));
    assert_eq!(p("1970-01-01 01:00:00"), Some(3600));
    assert_eq!(p("1970-01-01T01:00:00Z"), Some(3600));
    assert_eq!(p("1970-01-01 03:00:00+02"), Some(3600));
    assert_eq!(p("1970-01-01 03:30:00+02:30"), Some(3600));
    assert_eq!(p("1970-01-01 00:00:00-0100"), Some(3600));
    assert_eq!(p("1970-01-01 01:00:00.123456+00"), Some(3600));
    assert_eq!(p("1970-01-01 01:00"), Some(3600));
    assert_eq!(p("2024-02-29"), Some(1_709_164_800));
    for bad in [
        "2024-02-31",
        "2023-02-29",
        "2024-13-01",
        "2024-04-31",
        "2024-01-01x",
        "2024-01-01 25:00:00",
        "2024-01-01 12:00:00+99",
        "2024-01-01 12:00:00 junk",
        "２０２４-01-01",
        "2024-1-01",
    ] {
        assert_eq!(p(bad), None, "{bad}");
    }
}

#[test]
fn non_finite_json() {
    let s = sql::sanitize_json_numbers(
        r#"[{"a":NaN,"b":Infinity,"c":-Infinity,"d":"NaN","e":[nan, inf, 1],"f":"x NaN"}]"#,
    );
    let v: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(v[0]["a"].is_null() && v[0]["b"].is_null() && v[0]["c"].is_null());
    assert_eq!(v[0]["d"], "NaN", "strings untouched");
    assert_eq!(v[0]["f"], "x NaN");
    assert!(v[0]["e"][0].is_null() && v[0]["e"][1].is_null());
    assert!(matches!(
        sql::sanitize_json_numbers(r#"{"a":1}"#),
        std::borrow::Cow::Borrowed(_)
    ));
    // "NaN"/"inf" strings in a numeric role → missing, not NaN.
    let rows =
        sql::parse_rows_json(r#"[{"c0":"NaN"},{"c0":"inf"},{"c0":"2.5"},{"c0":NaN}]"#).unwrap();
    let cols = sql::columns_from_rows(&rows, &[(0, Role::Value(Kind::Line))]);
    let v = &cols[0].values;
    assert_eq!(v.iter().filter(|x| x.as_f64().is_some()).count(), 1);
    assert!(v.iter().all(|x| x.as_f64().is_none_or(|f| f.is_finite())));
}
