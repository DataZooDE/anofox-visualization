//! Which request variables a statement's answer can depend on (#210). The
//! locked server keys its result cache and single-flight on those only, and
//! the page re-requests a panel only when one of them changed
//! (`window.__served.deps`).
//!
//! Conservative by construction: whatever the analysis can't see through
//! makes the statement depend on every variable ([`Deps::All`]).
//! * `getvariable('name')` — a plain `'…'` literal as its only argument,
//!   optionally schema-qualified — reads `name`. Any other use of the word
//!   (another argument, an `E'…'` / `$$…$$` string, a named or second
//!   argument, the method form `('x').getvariable()`, `"getvariable"`, the
//!   bare word) is All.
//! * `duckdb_variables()`, `query()`, `query_table()` and
//!   `json_execute_serialized_sql()` list variables or run SQL from a value:
//!   All.
//! * A catalog macro or view that reads variables (`implicit`, from
//!   [`catalog_reads`]) adds what it reads; a string literal that mentions
//!   `getvariable` / `duckdb_variables` or names such an object (a table
//!   function can take a view's name) is All.
//! * `${name}` in the statement's `-- @requires` (and its tab's) reads `name`.
//! * Setup before a statement: `SET VARIABLE v = expr` adds what `expr` reads
//!   when `v` is needed; any other setup statement (a TEMP view, macro or
//!   table can hide a read) adds what it reads to every later statement; a
//!   `SET` / `RESET` of a variable in any other form makes them All. Closed to
//!   a fixpoint that ignores the order (a superset).
//!
//! Names are compared in lower case (DuckDB's variables are
//! case-insensitive); a name that is not ASCII is All.

use super::lex::{tokenize, TokKind, Token};
use super::requires::{all_vars, Requirement};
use std::collections::{BTreeSet, HashMap};

/// What an answer can depend on among the request's variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deps {
    /// Every variable (the analysis could not see through the statement).
    All,
    /// These variables only (lower case).
    Vars(BTreeSet<String>),
}

impl Default for Deps {
    fn default() -> Deps {
        Deps::Vars(BTreeSet::new())
    }
}

impl Deps {
    pub fn is_all(&self) -> bool {
        matches!(self, Deps::All)
    }

    /// Does the answer depend on variable `name` (any case)?
    pub fn contains(&self, name: &str) -> bool {
        match self {
            Deps::All => true,
            Deps::Vars(v) => v.contains(&name.to_ascii_lowercase()),
        }
    }

    fn insert(&mut self, name: &str) {
        if let Deps::Vars(v) = self {
            v.insert(name.to_ascii_lowercase());
        }
    }

    /// Add `other`; true if this grew.
    pub fn extend(&mut self, other: &Deps) -> bool {
        match (&mut *self, other) {
            (Deps::All, _) => false,
            (_, Deps::All) => {
                *self = Deps::All;
                true
            }
            (Deps::Vars(a), Deps::Vars(b)) => {
                let n = a.len();
                a.extend(b.iter().cloned());
                a.len() != n
            }
        }
    }

    /// For the page: `null` = every variable, else the sorted names.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Deps::All => serde_json::Value::Null,
            Deps::Vars(v) => serde_json::json!(v),
        }
    }
}

/// Functions that list the variables or run SQL from a value.
const DYNAMIC: &[&str] = &[
    "duckdb_variables",
    "query",
    "query_table",
    "json_execute_serialized_sql",
];

fn code_tokens(sql: &str) -> Vec<Token> {
    tokenize(sql)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect()
}

/// A plain `'…'` literal's text (`''` unescaped).
fn plain_string(sql: &str, t: &Token) -> Option<String> {
    let s = t.text(sql);
    (t.kind == TokKind::String && s.len() >= 2 && s.starts_with('\'') && s.ends_with('\''))
        .then(|| s[1..s.len() - 1].replace("''", "'"))
}

/// A `"quoted identifier"`'s name.
fn unquote(s: &str) -> String {
    let inner = s.strip_prefix('"').unwrap_or(s);
    let inner = inner.strip_suffix('"').unwrap_or(inner);
    inner.replace("\"\"", "\"")
}

/// The identifier-like words of a string literal (lower case).
fn words(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

/// What one statement reads by itself (no setup, no `-- @requires`).
/// `implicit`: the catalog's macros and views that read variables, by
/// lower-case name ([`catalog_reads`]).
pub fn reads(sql: &str, implicit: &HashMap<String, Deps>) -> Deps {
    let toks = code_tokens(sql);
    let mut out = Deps::default();
    for (k, t) in toks.iter().enumerate() {
        let text = t.text(sql);
        match t.kind {
            TokKind::Word => {
                let w = text.to_lowercase();
                if w == "getvariable" {
                    let qualified_ok = k == 0
                        || !toks[k - 1].is_punct(sql, '.')
                        || (k > 1
                            && matches!(toks[k - 2].kind, TokKind::Word | TokKind::QuotedIdent));
                    let lit = qualified_ok
                        .then(|| {
                            (toks.get(k + 1).is_some_and(|n| n.is_punct(sql, '('))
                                && toks.get(k + 3).is_some_and(|n| n.is_punct(sql, ')')))
                            .then(|| toks.get(k + 2).and_then(|t2| plain_string(sql, t2)))
                            .flatten()
                        })
                        .flatten();
                    match lit {
                        Some(name) if name.is_ascii() => out.insert(&name),
                        _ => return Deps::All,
                    }
                } else if DYNAMIC.contains(&w.as_str())
                    && toks.get(k + 1).is_some_and(|n| n.is_punct(sql, '('))
                {
                    return Deps::All;
                } else if let Some(d) = implicit.get(&w) {
                    out.extend(d);
                }
            }
            TokKind::QuotedIdent => {
                let w = unquote(text).to_lowercase();
                if w == "getvariable" || DYNAMIC.contains(&w.as_str()) {
                    return Deps::All;
                }
                if let Some(d) = implicit.get(&w) {
                    out.extend(d);
                }
            }
            TokKind::String | TokKind::DollarString => {
                let low = text.to_lowercase();
                if low.contains("getvariable")
                    || low.contains("duckdb_variables")
                    || words(&low).any(|w| implicit.contains_key(&w))
                {
                    return Deps::All;
                }
            }
            _ => {}
        }
        if out.is_all() {
            return Deps::All;
        }
    }
    out
}

/// The variables `${name}` in `-- @requires` lines name.
fn requires_reads(reqs: &[Requirement]) -> Deps {
    let mut d = Deps::default();
    for v in all_vars(reqs) {
        d.insert(&v); // ([A-Za-z_][A-Za-z0-9_]*: requires::vars checks)
    }
    d
}

/// What each catalog macro / view reads, by lower-case name. `objects`:
/// `(name, definition)` — every overload of a macro, a view's SQL. Closed
/// over objects that use one another; objects that read nothing are absent.
pub fn catalog_reads(objects: &[(String, String)]) -> HashMap<String, Deps> {
    let mut m: HashMap<String, Deps> = HashMap::new();
    loop {
        let mut grew = false;
        for (name, def) in objects {
            let d = reads(def, &m);
            if d == Deps::default() {
                continue;
            }
            grew |= m.entry(name.to_lowercase()).or_default().extend(&d);
        }
        if !grew {
            return m;
        }
    }
}

enum Setup {
    /// `SET VARIABLE <name> = …` (lower case).
    Assign(String),
    /// A `SET` / `RESET` of a variable in another form.
    Unknown,
    Other,
}

fn classify(sql: &str) -> Setup {
    let toks = code_tokens(sql);
    let word = |i: usize, kw: &str| toks.get(i).is_some_and(|t| t.is_word(sql, kw));
    if word(0, "set") && word(1, "variable") {
        let name = toks.get(2).and_then(|t| match t.kind {
            TokKind::Word => Some(t.text(sql).to_string()),
            TokKind::QuotedIdent => Some(unquote(t.text(sql))),
            _ => None,
        });
        let assign = toks
            .get(3)
            .is_some_and(|t| t.is_punct(sql, '=') || t.is_word(sql, "to"));
        return match name {
            Some(n) if assign && n.is_ascii() => Setup::Assign(n.to_ascii_lowercase()),
            _ => Setup::Unknown,
        };
    }
    if (word(0, "set") || word(0, "reset")) && toks.iter().any(|t| t.is_word(sql, "variable")) {
        return Setup::Unknown;
    }
    Setup::Other
}

/// One planned statement, as the server runs it.
pub struct Stmt<'a> {
    pub setup: bool,
    pub sql: &'a str,
    /// Its own `-- @requires`.
    pub requires: &'a [Requirement],
    /// The `::TAB` / `::SUBTAB` statements whose `-- @requires` gate it.
    pub tabs: &'a [usize],
}

/// Per statement: the request variables its answer can depend on. A setup
/// statement is answered `[]` on its own (the server runs setup with every
/// panel): no variables.
pub fn script_deps(stmts: &[Stmt], implicit: &HashMap<String, Deps>) -> Vec<Deps> {
    let own: Vec<Deps> = stmts
        .iter()
        .map(|s| {
            let mut d = reads(s.sql, implicit);
            d.extend(&requires_reads(s.requires));
            d
        })
        .collect();
    let kind: Vec<Setup> = stmts
        .iter()
        .map(|s| {
            if s.setup {
                classify(s.sql)
            } else {
                Setup::Other
            }
        })
        .collect();
    (0..stmts.len())
        .map(|p| {
            if stmts[p].setup {
                return Deps::default();
            }
            let mut need = own[p].clone();
            for &g in stmts[p].tabs {
                match stmts.get(g) {
                    Some(s) => {
                        need.extend(&requires_reads(s.requires));
                    }
                    None => return Deps::All,
                }
            }
            loop {
                let mut grew = false;
                for i in (0..p).filter(|&i| stmts[i].setup) {
                    grew |= match &kind[i] {
                        Setup::Unknown => need.extend(&Deps::All),
                        Setup::Assign(v) if need.contains(v) => need.extend(&own[i]),
                        Setup::Assign(_) => false,
                        Setup::Other => need.extend(&own[i]),
                    };
                }
                if !grew || need.is_all() {
                    return need;
                }
            }
        })
        .collect()
}

/// Per statement: the request variables the setup statements before it read
/// (#388) — what a connection's state after that setup can depend on, so the
/// locked server may reuse such a connection for another request with the
/// same values. Every setup statement counts (all of them run), and a `SET` /
/// `RESET` the analysis can't read makes it [`Deps::All`].
pub fn setup_deps(stmts: &[Stmt], implicit: &HashMap<String, Deps>) -> Vec<Deps> {
    let mut acc = Deps::default();
    let mut out = Vec::with_capacity(stmts.len());
    for s in stmts {
        out.push(acc.clone());
        if s.setup {
            if matches!(classify(s.sql), Setup::Unknown) {
                acc.extend(&Deps::All);
            } else {
                acc.extend(&reads(s.sql, implicit));
                acc.extend(&requires_reads(s.requires));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_deps_collects_every_setup_read_before_a_statement() {
        let st = |setup, sql| Stmt {
            setup,
            sql,
            requires: &[],
            tabs: &[],
        };
        let stmts = [
            st(true, "SET VARIABLE a = coalesce(getvariable('a'), 1)"),
            st(true, "CREATE TEMP TABLE w AS SELECT getvariable('k') AS v"),
            st(false, "SELECT getvariable('b')"),
            st(true, "SET VARIABLE x = 1; SET y = 2"),
            st(false, "SELECT 1"),
        ];
        let d = setup_deps(&stmts, &HashMap::new());
        assert_eq!(d[0], Deps::default());
        assert_eq!(d[1], Deps::Vars(["a".to_string()].into_iter().collect()));
        assert_eq!(
            d[2],
            Deps::Vars(["a".to_string(), "k".to_string()].into_iter().collect())
        );
        assert!(
            !d[2].contains("b"),
            "a panel's own reads are not setup reads"
        );
        assert_eq!(d[3], d[2]);
    }

    fn v(names: &[&str]) -> Deps {
        Deps::Vars(names.iter().map(|s| s.to_lowercase()).collect())
    }
    fn none() -> HashMap<String, Deps> {
        HashMap::new()
    }
    fn stmt(setup: bool, sql: &str) -> (bool, &str, Vec<Requirement>, Vec<usize>) {
        (setup, sql, Vec::new(), Vec::new())
    }
    fn plan_deps(
        stmts: &[(bool, &str, Vec<Requirement>, Vec<usize>)],
        implicit: &HashMap<String, Deps>,
    ) -> Vec<Deps> {
        let views: Vec<Stmt> = stmts
            .iter()
            .map(|(setup, sql, requires, tabs)| Stmt {
                setup: *setup,
                sql,
                requires,
                tabs,
            })
            .collect();
        script_deps(&views, implicit)
    }

    // --- literals ---
    #[test]
    fn literal_getvariable_reads_its_name() {
        assert_eq!(
            reads("SELECT getvariable('Series') FROM t", &none()),
            v(&["series"])
        );
        assert_eq!(
            reads(
                "SELECT GetVariable ( /*c*/ 'a' ) + getvariable('b')",
                &none()
            ),
            v(&["a", "b"])
        );
        assert_eq!(reads("SELECT main.getvariable('a')", &none()), v(&["a"]));
        assert_eq!(
            reads("SELECT system.main.getvariable('a')", &none()),
            v(&["a"])
        );
        assert_eq!(reads("SELECT getvariable('it''s')", &none()), v(&["it's"]));
        assert_eq!(reads("SELECT 1", &none()), v(&[]));
        assert_eq!(reads("SELECT 'getvar'", &none()), v(&[]));
        assert_eq!(reads("SELECT query FROM log", &none()), v(&[]));
    }

    /// Security review of #210/#222 (I1): a `--` comment that a lone CR ends
    /// (DuckDB ends it there too, not only at LF) must not hide a real read —
    /// the analysed text must match what DuckDB executes.
    #[test]
    fn a_cr_ended_comment_does_not_hide_a_read() {
        assert_eq!(
            reads("SELECT 1 --c\r, getvariable('secret') AS z", &none()),
            v(&["secret"])
        );
    }

    // --- fallbacks to All ---
    #[test]
    fn unresolvable_getvariable_uses_are_all() {
        let all = [
            "SELECT getvariable(name) FROM t",
            "SELECT getvariable('a' || 'b')",
            "SELECT getvariable(E'a')",
            "SELECT getvariable($$a$$)",
            "SELECT getvariable(x := 'a')",
            "SELECT getvariable('a', 1)",
            "SELECT getvariable('a' COLLATE nocase)",
            "SELECT getvariable('a'::VARCHAR)",
            "SELECT ('a').getvariable()",
            "SELECT \"getvariable\"('a')",
            "SELECT getvariable FROM t",
            "SELECT getvariable",
            "SELECT getvariable('Ä')",
            "SELECT 1 FROM duckdb_variables()",
            "SELECT 1 FROM query('SELECT 1')",
            "SELECT 1 FROM query_table('t')",
            "SELECT 1 FROM json_execute_serialized_sql(x)",
            "SELECT \"query\"('x')",
            "SELECT 'getvariable(''a'')'",
            "SELECT $$duckdb_variables$$",
        ];
        for sql in all {
            assert_eq!(reads(sql, &none()), Deps::All, "{sql}");
        }
    }

    // --- implicit objects ---
    #[test]
    fn implicit_catalog_objects_add_their_deps() {
        let mut implicit = HashMap::new();
        implicit.insert("m".to_string(), v(&["x"]));
        implicit.insert("v".to_string(), Deps::All);
        implicit.insert("w".to_string(), v(&["y"]));
        assert_eq!(reads("SELECT m()", &implicit), v(&["x"]));
        assert_eq!(reads("SELECT \"M\"()", &implicit), v(&["x"]));
        assert_eq!(reads("FROM v", &implicit), Deps::All);
        assert_eq!(reads("FROM w JOIN t USING (k)", &implicit), v(&["y"]));
        assert_eq!(
            reads("FROM ts_fill_forward('w', 'd')", &implicit),
            Deps::All
        );
        assert_eq!(reads("SELECT 'wide'", &implicit), v(&[]));
    }

    // --- catalog_reads ---
    #[test]
    fn catalog_reads_closes_over_objects() {
        let objects = vec![
            ("m".to_string(), "getvariable('x')".to_string()),
            ("n".to_string(), "m() + 1".to_string()),
            (
                "w".to_string(),
                "CREATE VIEW w AS SELECT n() AS z".to_string(),
            ),
            ("p".to_string(), "getvariable(q)".to_string()),
            ("plain".to_string(), "1 + 1".to_string()),
            ("M2".to_string(), "getvariable('Y')".to_string()),
        ];
        let expect = |m: &HashMap<String, Deps>| {
            assert_eq!(m.get("m"), Some(&v(&["x"])));
            assert_eq!(m.get("n"), Some(&v(&["x"])));
            assert_eq!(m.get("w"), Some(&v(&["x"])));
            assert_eq!(m.get("p"), Some(&Deps::All));
            assert_eq!(m.get("m2"), Some(&v(&["y"])));
            assert!(!m.contains_key("plain"));
        };
        expect(&catalog_reads(&objects));
        let mut reversed = objects.clone();
        reversed.reverse();
        expect(&catalog_reads(&reversed));
    }

    // --- closure through setup (the issue's example) ---
    #[test]
    fn closure_through_setup_the_issue_example() {
        let stmts = [
            stmt(
                true,
                "SET VARIABLE sel = coalesce(getvariable('series'), 'a')",
            ),
            stmt(false, "SELECT * FROM t WHERE s = getvariable('sel')"),
            stmt(false, "SELECT count(*) FROM t"),
            stmt(false, "SELECT getvariable('region')"),
        ];
        assert_eq!(
            plan_deps(&stmts, &none()),
            vec![v(&[]), v(&["sel", "series"]), v(&[]), v(&["region"])]
        );
    }

    // --- chains ---
    #[test]
    fn chains_of_set_variable() {
        let stmts = [
            stmt(true, "SET VARIABLE a = getvariable('x')"),
            stmt(true, "SET VARIABLE b = getvariable('a')"),
            stmt(true, "SET VARIABLE c = getvariable('y')"),
            stmt(false, "SELECT getvariable('b')"),
        ];
        assert_eq!(plan_deps(&stmts, &none())[3], v(&["a", "b", "x"]));

        // Reverse declaration order: the fixpoint ignores order among the
        // setups before `p`, so it may give a superset (`x` allowed too).
        let stmts2 = [
            stmt(true, "SET VARIABLE b = getvariable('a')"),
            stmt(true, "SET VARIABLE a = getvariable('x')"),
            stmt(false, "SELECT getvariable('b')"),
        ];
        let d = plan_deps(&stmts2, &none());
        let Deps::Vars(got) = &d[2] else {
            panic!("expected Vars, got {:?}", d[2]);
        };
        assert!(got.contains("a") && got.contains("b"));
    }

    // --- order-free macro case ---
    #[test]
    fn order_free_macro_case() {
        let stmts = [
            stmt(true, "CREATE TEMP MACRO f() AS getvariable('x')"),
            stmt(true, "SET VARIABLE x = getvariable('y')"),
            stmt(false, "SELECT f()"),
        ];
        assert_eq!(plan_deps(&stmts, &none())[2], v(&["x", "y"]));
    }

    // --- other setup propagates forward only ---
    #[test]
    fn other_setup_propagates_forward_only() {
        let stmts = [
            stmt(false, "SELECT 1 FROM t"),
            stmt(
                true,
                "CREATE TEMP VIEW tv AS SELECT * FROM t WHERE r = getvariable('r')",
            ),
            stmt(false, "SELECT * FROM tv"),
        ];
        assert_eq!(plan_deps(&stmts, &none()), vec![v(&[]), v(&[]), v(&["r"])]);
    }

    // --- unknown SET / RESET ---
    #[test]
    fn unknown_set_reset_makes_everything_later_all() {
        let stmts = [
            stmt(false, "SELECT 1 AS a"),
            stmt(true, "RESET VARIABLE x"),
            stmt(false, "SELECT 1 AS b"),
        ];
        assert_eq!(plan_deps(&stmts, &none()), vec![v(&[]), v(&[]), Deps::All]);

        let stmts2 = [
            stmt(false, "SELECT 1 AS a"),
            stmt(true, "SET VARIABLE x.y = 1"),
            stmt(false, "SELECT 1 AS b"),
        ];
        assert_eq!(plan_deps(&stmts2, &none())[2], Deps::All);

        let stmts3 = [
            stmt(true, "CREATE TEMP MACRO g(n) AS getvariable(n)"),
            stmt(false, "SELECT 1"),
        ];
        assert_eq!(plan_deps(&stmts3, &none())[1], Deps::All);
    }

    // --- assign forms ---
    #[test]
    fn assign_forms() {
        let stmts = [
            stmt(true, "SET VARIABLE \"Sel\" = getvariable('q')"),
            stmt(false, "SELECT getvariable('sel')"),
        ];
        assert_eq!(plan_deps(&stmts, &none())[1], v(&["q", "sel"]));

        let stmts2 = [
            stmt(true, "SET VARIABLE y TO getvariable('z')"),
            stmt(false, "SELECT getvariable('y')"),
        ];
        assert_eq!(plan_deps(&stmts2, &none())[1], v(&["y", "z"]));
    }

    // --- requires ---
    #[test]
    fn requires_variables() {
        let r1: Vec<Requirement> = vec![vec!["${rd}/a.parquet".to_string()]];
        let stmts = [(false, "SELECT 1", r1, Vec::new())];
        assert_eq!(plan_deps(&stmts, &none())[0], v(&["rd"]));

        let r_set: Vec<Requirement> = Vec::new();
        let stmts2 = [
            (
                true,
                "SET VARIABLE rd = getvariable('anofox_data') || '/' || getvariable('run')",
                r_set,
                Vec::new(),
            ),
            (
                false,
                "SELECT 1",
                vec![vec!["${rd}/a.parquet".to_string()]],
                Vec::new(),
            ),
        ];
        assert_eq!(
            plan_deps(&stmts2, &none())[1],
            v(&["anofox_data", "rd", "run"])
        );

        let malformed: Vec<Requirement> = vec![vec!["${1x}/a".to_string()]];
        let stmts3 = [(false, "SELECT 1", malformed, Vec::new())];
        assert_eq!(plan_deps(&stmts3, &none())[0], v(&[]));

        let stmts4 = [
            (
                true,
                "SET VARIABLE v = 1",
                vec![vec!["${q}".to_string()]],
                Vec::new(),
            ),
            (false, "SELECT getvariable('v')", Vec::new(), Vec::new()),
        ];
        assert_eq!(plan_deps(&stmts4, &none())[1], v(&["q", "v"]));
    }

    // --- tab gates ---
    #[test]
    fn tab_gates() {
        let stmts = [
            (
                false,
                "SELECT 'A'::TAB",
                vec![vec!["${run}".to_string()]],
                Vec::new(),
            ),
            (false, "SELECT 1", Vec::new(), vec![0usize]),
        ];
        assert_eq!(plan_deps(&stmts, &none())[1], v(&["run"]));

        let stmts2 = [(false, "SELECT 1", Vec::new(), vec![9usize])];
        assert_eq!(plan_deps(&stmts2, &none())[0], Deps::All);
    }

    // --- Deps API ---
    #[test]
    fn deps_api() {
        assert!(Deps::All.contains("anything"));
        assert!(v(&["a"]).contains("A"));
        assert!(!v(&["a"]).contains("b"));

        let mut d = v(&["a"]);
        assert!(!d.extend(&v(&["a"])));
        assert!(d.extend(&v(&["b"])));
        assert!(d.extend(&Deps::All));
        assert!(d.is_all());

        assert_eq!(Deps::All.to_json(), serde_json::Value::Null);
        assert_eq!(v(&["b", "a"]).to_json(), serde_json::json!(["a", "b"]));
    }
}
