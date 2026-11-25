//! `-- @materialize` (#727): a heavy setup statement — an engine run, a
//! Monte Carlo simulation — that a locked server runs once and shares
//! between the panel requests that would each run it again. Only the form
//! `CREATE [OR REPLACE] TEMP[ORARY] TABLE|VIEW [IF NOT EXISTS] <name> AS <query>`
//! qualifies ([`split`]); the linter flags the line anywhere else
//! (`materialize/not-table`). A view becomes a TEMP table of the same name
//! there (#1362: a view read by views is planned again at every reference).
//! The browser runs the statement as written (the line is a comment there).
//!
//! ```sql
//! -- @materialize
//! CREATE OR REPLACE TEMP TABLE sim AS SELECT * FROM run_twin(plan_nodes, 10000);
//! ```
//!
//! Parsed like [`super::name`]'s `-- @name`: comment lines directly above the
//! statement. `-- @materialise` is the same line.

use std::borrow::Cow;

use super::lex::{tokenize, TokKind};

/// Is there a `-- @materialize` (or `-- @materialise`) line directly above
/// the statement?
pub fn parse(raw: &str) -> bool {
    raw.lines()
        .filter_map(|l| l.trim().strip_prefix("--"))
        .filter_map(|l| {
            let l = l.trim();
            l.strip_prefix("@materialize")
                .or_else(|| l.strip_prefix("@materialise"))
        })
        .any(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// `sql` as `(head, query)` when it is `CREATE [OR REPLACE] TEMP TABLE|VIEW
/// [IF NOT EXISTS] <name> AS <query>`: `head` is the text up to and including
/// `AS`, with `VIEW` turned into `TABLE` (`head + " FROM x"` creates the
/// table from `x`), `query` the rest. `None` for any other statement.
pub fn split(sql: &str) -> Option<(Cow<'_, str>, &str)> {
    let toks: Vec<_> = tokenize(sql)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    let word = |i: usize, kw: &str| toks.get(i).is_some_and(|t| t.is_word(sql, kw));
    let mut i = 0;
    if !word(i, "CREATE") {
        return None;
    }
    i += 1;
    if word(i, "OR") && word(i + 1, "REPLACE") {
        i += 2;
    }
    if !(word(i, "TEMP") || word(i, "TEMPORARY")) || !(word(i + 1, "TABLE") || word(i + 1, "VIEW"))
    {
        return None;
    }
    let view = word(i + 1, "VIEW").then(|| &toks[i + 1]);
    i += 2;
    if word(i, "IF") && word(i + 1, "NOT") && word(i + 2, "EXISTS") {
        i += 3;
    }
    if !toks
        .get(i)
        .is_some_and(|t| matches!(t.kind, TokKind::Word | TokKind::QuotedIdent))
    {
        return None;
    }
    let as_ = toks.get(i + 1).filter(|t| t.is_word(sql, "AS"))?;
    let query = sql[as_.end..].trim().trim_end_matches(';').trim_end();
    if query.is_empty() {
        return None;
    }
    let head = match view {
        Some(v) => Cow::Owned(format!("{}TABLE{}", &sql[..v.start], &sql[v.end..as_.end])),
        None => Cow::Borrowed(&sql[..as_.end]),
    };
    Some((head, query))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_line() {
        assert!(parse("-- @materialize\nCREATE TEMP TABLE t AS SELECT 1;"));
        assert!(parse(
            "-- heavy\n--   @materialise  \nCREATE TEMP TABLE t AS SELECT 1;"
        ));
        assert!(!parse("-- @materialized\nCREATE TEMP TABLE t AS SELECT 1;"));
        assert!(!parse("CREATE TEMP TABLE t AS SELECT 1;"));
    }

    #[test]
    fn splits_a_temp_table_as_query() {
        assert_eq!(
            split("CREATE OR REPLACE TEMP TABLE sim AS SELECT * FROM range(3);"),
            Some((
                "CREATE OR REPLACE TEMP TABLE sim AS".into(),
                "SELECT * FROM range(3)"
            ))
        );
        assert_eq!(
            split("create temporary table if not exists \"my t\" as\n  from x"),
            Some((
                "create temporary table if not exists \"my t\" as".into(),
                "from x"
            ))
        );
    }

    #[test]
    fn a_temp_view_becomes_a_table() {
        assert_eq!(
            split("CREATE OR REPLACE TEMP VIEW cl AS SELECT * FROM a WHERE n > 1;"),
            Some((
                "CREATE OR REPLACE TEMP TABLE cl AS".into(),
                "SELECT * FROM a WHERE n > 1"
            ))
        );
        assert_eq!(
            split("create temporary view if not exists v as from x"),
            Some(("create temporary TABLE if not exists v as".into(), "from x"))
        );
    }

    #[test]
    fn refuses_anything_else() {
        for sql in [
            "CREATE VIEW v AS SELECT 1",
            "CREATE TEMP VIEW v (a) AS SELECT 1",
            "CREATE TEMP MACRO m() AS 1",
            "CREATE TABLE t AS SELECT 1",
            "CREATE TEMP TABLE t (a INT)",
            "CREATE TEMP TABLE main.t AS SELECT 1",
            "CREATE TEMP TABLE t AS",
            "SET VARIABLE x = 1",
            "SELECT 1",
        ] {
            assert_eq!(split(sql), None, "{sql}");
        }
    }
}
