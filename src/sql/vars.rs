//! The variables "this dashboard sets", anywhere in the plan — a static,
//! name-only collection (no query execution) used by `show_if/unknown-
//! variable` (#515) and reused by #516's `step/var-clash` /
//! `step/gate-unknown-variable`.
//!
//! [`declared`] is deliberately conservative: a name it misses only risks a
//! false "unknown" warning (the author can silence it), never a false
//! "known" that would hide a real typo. `__claim_*` names (the viewer's
//! login, set by the server) are not enumerable from the plan at all — the
//! `show_if/unknown-variable` check skips any name with that prefix itself,
//! rather than listing them here.

use super::{lex, Panel};
use crate::{InputKind, Role};

/// Every variable name (lower case) this plan sets somewhere: input
/// variables (every kind; `::DATERANGE`'s two columns; a `::PERIOD`'s
/// `_from`/`_to`/`_prev_from`/`_prev_to`/`_grain`/`_compare`/`_label`/
/// `_prev_label` suite), a setup `SET VARIABLE x = …`, a literal
/// `::SELECT_AS`'s name (+ `_from`/`_to`), a literal `::DRILL`'s
/// `<prefix>_level`/`_depth`/`_path` plus one variable per level, an
/// `::ROWSELECT`'s key column (the next table's first non-`::TITLE`/
/// `::INFO` column, read from its own SQL text), and the fixed
/// `anofox_data`. A computed (non-literal) `::SELECT_AS`/`::DRILL` value, or
/// an input/`SET VARIABLE` with no simple name, is silently left out (see
/// the module docs).
pub fn declared(panels: &[Panel]) -> Vec<String> {
    let mut out = vec!["anofox_data".to_string()];
    for (i, p) in panels.iter().enumerate() {
        if let Some(kind) = crate::inputs::input_kind(&p.roles) {
            out.extend(input_names(p, kind));
        }
        if p.setup {
            out.extend(set_variable_name(&p.sql));
        }
        if has_role(p, Role::SelectAs) {
            if let Some(v) = literal_value(p) {
                if let Ok(sets) = select_as_prelude(&v) {
                    out.extend(names_from_sets(&sets));
                }
            }
        }
        if has_role(p, Role::Drill) {
            if let Some(v) = literal_value(p) {
                if let Ok(sets) = drill_prelude(&v) {
                    out.extend(names_from_sets(&sets));
                }
            }
        }
        if has_role(p, Role::RowSelect) {
            out.extend(rowselect_key(panels, i));
        }
    }
    out.iter().map(|s| s.to_ascii_lowercase()).collect()
}

fn has_role(p: &Panel, want: Role) -> bool {
    p.roles.iter().any(|(_, r)| *r == want)
}

/// The `SET VARIABLE <name>` names a `sets` text (one per line, as
/// [`select_as_prelude`] / [`drill_prelude`] emit) assigns.
pub(crate) fn names_from_sets(sets: &str) -> Vec<String> {
    sets.lines()
        .filter_map(|l| l.strip_prefix("SET VARIABLE "))
        .filter_map(|l| l.split(' ').next())
        .map(|n| n.trim_matches('"').to_string())
        .collect()
}

/// `'[prefix:] name [single|multi]'` of a `::SELECT_AS` → the browser's
/// initial variables: the name itself, `_from` and `_to` (a brush).
pub(crate) fn select_as_prelude(v: &str) -> Result<String, String> {
    let mut parts = v
        .split(|c: char| c.is_whitespace() || c == ':' || c == ',')
        .filter(|p| !p.is_empty());
    let name = parts.next().unwrap_or("");
    let mode = parts.next().unwrap_or("single").to_ascii_lowercase();
    if !is_ident(name) || !matches!(mode.as_str(), "single" | "multi") || parts.next().is_some() {
        return Err(format!(
            "::SELECT_AS `{v}` — expected a variable name, optionally followed by \
             'single' or 'multi' (e.g. 'region' or 'region multi'); the chart keeps \
             the shared `selected` click"
        ));
    }
    let sel = if mode == "multi" {
        "[]::VARCHAR[]"
    } else {
        "''"
    };
    Ok(format!(
        "SET VARIABLE {n} = {sel};\nSET VARIABLE {n}_from = NULL;\nSET VARIABLE {n}_to = NULL;\n",
        n = name
    ))
}

/// `'[prefix:] a > b > c'` of a `::DRILL` → the browser's initial variables:
/// the top level, depth 0, an empty path and every level variable `''`.
pub(crate) fn drill_prelude(v: &str) -> Result<String, String> {
    let (prefix, path) = match v.split_once(':') {
        Some((p, rest)) => (p.trim(), rest),
        None => ("drill", v),
    };
    let levels: Vec<&str> = path.split('>').map(str::trim).collect();
    if !is_ident(prefix) || levels.len() < 2 || !levels.iter().all(|l| is_ident(l)) {
        return Err(format!(
            "::DRILL `{v}` — expected two or more column names joined by '>' \
             (e.g. 'region > country > city', optionally 'geo: region > …'); \
             the chart does not drill"
        ));
    }
    let mut out = format!(
        "SET VARIABLE {prefix}_level = '{}';\nSET VARIABLE {prefix}_depth = 0;\n\
         SET VARIABLE {prefix}_path = []::VARCHAR[];\n",
        levels[0]
    );
    for l in &levels {
        out.push_str(&format!("SET VARIABLE {l} = '';\n"));
    }
    Ok(out)
}

/// A plain identifier (`^[A-Za-z_][A-Za-z0-9_]*$`, at most 60 chars).
fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s.len() <= 60
}

/// The suffixes a `::PERIOD` named `prefix` sets (see [`crate::period::default_sets`]).
const PERIOD_SUFFIXES: &[&str] = &[
    "from",
    "to",
    "prev_from",
    "prev_to",
    "grain",
    "compare",
    "label",
    "prev_label",
];

/// The variable name(s) an input panel sets.
fn input_names(p: &Panel, kind: InputKind) -> Vec<String> {
    match kind {
        InputKind::DateRange => daterange_columns(p),
        InputKind::Period => {
            let idx = crate::inputs::input_index(&p.roles);
            let name = p
                .names
                .iter()
                .find(|(j, _)| *j == idx)
                .map(|(_, n)| n.clone());
            let Some(name) = name else { return Vec::new() };
            let prefix = crate::period::var_prefix(&name);
            PERIOD_SUFFIXES
                .iter()
                .map(|s| format!("{prefix}_{s}"))
                .collect()
        }
        _ => {
            let idx = crate::inputs::input_index(&p.roles);
            p.names
                .iter()
                .find(|(j, _)| *j == idx)
                .map(|(_, n)| vec![n.clone()])
                .unwrap_or_default()
        }
    }
}

/// A `::DATERANGE` keeps every column's own name in its rewritten SQL (see
/// `sql::rewrite_list`'s `keep_intact` branch) — its first two select items
/// are the `from` / `to` variables, read straight from the SQL text (no
/// query needed).
fn daterange_columns(p: &Panel) -> Vec<String> {
    let Some((start, end)) = super::main_select_list(&p.sql) else {
        return Vec::new();
    };
    lex::split_top_commas(&p.sql[start..end])
        .iter()
        .take(2)
        .filter_map(|item| {
            let (core, alias) = lex::trailing_alias(item.trim());
            alias.or_else(|| lex::simple_ident(core))
        })
        .collect()
}

/// This statement's literal select-item text for the role it names alone
/// (a `::SELECT_AS`/`::DRILL` marker is a one-column statement): a plain
/// `'…'`/`E'…'` string (`''` unescaped), or `None` for a computed value —
/// the same split `views::literal_text` uses for a `::VIEW` label.
fn literal_value(p: &Panel) -> Option<String> {
    let (start, end) = super::main_select_list(&p.sql)?;
    let item = lex::split_top_commas(&p.sql[start..end])
        .into_iter()
        .next()?;
    let core = item.trim().strip_suffix(" AS c0")?.trim();
    let toks: Vec<lex::Token> = lex::tokenize(core)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    match toks.as_slice() {
        [t] if t.kind == lex::TokKind::String => {
            let s = t.text(core);
            let s = s.strip_prefix(['E', 'e']).unwrap_or(s);
            let inner = s
                .strip_prefix('\'')
                .and_then(|r| r.strip_suffix('\''))
                .unwrap_or("");
            Some(inner.replace("''", "'"))
        }
        _ => None,
    }
}

/// A setup `SET VARIABLE <name> = …` (or `TO`)'s name, lower case already
/// being applied by the caller — `None` for any other setup shape (a TEMP
/// view/table/macro, `RESET`, a quoted or computed name this analysis can't
/// read statically).
fn set_variable_name(sql: &str) -> Option<String> {
    let toks: Vec<lex::Token> = lex::tokenize(sql)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    let word = |i: usize, kw: &str| toks.get(i).is_some_and(|t| t.is_word(sql, kw));
    if !(word(0, "set") && word(1, "variable")) {
        return None;
    }
    match toks.get(2)?.kind {
        lex::TokKind::Word => Some(toks[2].text(sql).to_string()),
        lex::TokKind::QuotedIdent => {
            let s = toks[2].text(sql);
            let inner = s.strip_prefix('"').unwrap_or(s);
            let inner = inner.strip_suffix('"').unwrap_or(inner);
            Some(inner.replace("\"\"", "\""))
        }
        _ => None,
    }
}

/// An `::ROWSELECT`'s key column: the first non-`::TITLE`/`::INFO` column of
/// the next table-shaped statement (skipping other directives in between,
/// the same statements `show_if::is_modifier` treats as pass-through),
/// read from its own SQL text.
fn rowselect_key(panels: &[Panel], at: usize) -> Vec<String> {
    for p in panels.iter().skip(at + 1) {
        if p.setup || crate::roles::is_directive_panel(&p.roles) {
            continue;
        }
        if !p
            .roles
            .iter()
            .any(|(_, r)| matches!(r, Role::Table | Role::PagedTable))
        {
            return Vec::new();
        }
        let Some((start, end)) = super::main_select_list(&p.sql) else {
            return Vec::new();
        };
        let skip: Vec<usize> = p
            .roles
            .iter()
            .filter(|(_, r)| matches!(r, Role::Title | Role::Info))
            .map(|(i, _)| *i)
            .collect();
        for (i, item) in lex::split_top_commas(&p.sql[start..end]).iter().enumerate() {
            if skip.contains(&i) {
                continue;
            }
            let item = item.trim();
            if lex::is_star_item(item) {
                continue;
            }
            let (core, alias) = lex::trailing_alias(item);
            if let Some(name) = alias.or_else(|| lex::simple_ident(core)) {
                return vec![name];
            }
        }
        return Vec::new();
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::plan;

    #[test]
    fn anofox_data_is_always_declared() {
        assert!(declared(&plan("SELECT 1")).contains(&"anofox_data".to_string()));
    }

    #[test]
    fn plain_input_name() {
        let p = plan("SELECT v AS q1 ::DROPDOWN FROM t");
        assert!(declared(&p).contains(&"q1".to_string()));
    }

    #[test]
    fn daterange_both_columns() {
        let p = plan("SELECT min(d) AS from_day, max(d) AS to_day ::DATERANGE FROM t");
        let d = declared(&p);
        assert!(d.contains(&"from_day".to_string()));
        assert!(d.contains(&"to_day".to_string()));
    }

    #[test]
    fn period_suite() {
        let p = plan("SELECT 'presets=7d,30d' AS p ::PERIOD");
        let d = declared(&p);
        for s in PERIOD_SUFFIXES {
            assert!(d.contains(&format!("p_{s}")), "p_{s}");
        }
    }

    #[test]
    fn set_variable_in_setup() {
        let p = plan("SET VARIABLE myvar = 1; SELECT 1;");
        assert!(declared(&p).contains(&"myvar".to_string()));
    }

    #[test]
    fn literal_select_as_names() {
        let p = plan("SELECT 'region'::SELECT_AS; SELECT a::XAXIS, b::BARCHART FROM t;");
        let d = declared(&p);
        assert!(d.contains(&"region".to_string()));
        assert!(d.contains(&"region_from".to_string()));
        assert!(d.contains(&"region_to".to_string()));
    }

    #[test]
    fn computed_select_as_is_skipped_not_guessed() {
        let p = plan("SELECT a || 'x' ::SELECT_AS FROM t; SELECT a::XAXIS, b::BARCHART FROM t;");
        // Nothing crashes, and nothing is invented for a computed value.
        let _ = declared(&p);
    }

    #[test]
    fn literal_drill_levels() {
        let p = plan("SELECT 'region > country' ::DRILL; SELECT a::XAXIS, b::BARCHART FROM t;");
        let d = declared(&p);
        for n in [
            "drill_level",
            "drill_depth",
            "drill_path",
            "region",
            "country",
        ] {
            assert!(d.contains(&n.to_string()), "{n}");
        }
    }

    #[test]
    fn rowselect_key_column_name() {
        let p = plan("SELECT 1::ROWSELECT; SELECT customer_id, n::TABLE FROM t;");
        assert!(declared(&p).contains(&"customer_id".to_string()));
    }
}
