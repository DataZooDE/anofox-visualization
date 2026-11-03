//! Where a dashboard's inputs stand against what they control (#949): from
//! the plan alone (no query runs), each input's variables, the statements
//! whose answer reads them ([`crate::sql::deps`], #210) and the sections
//! (`::TAB` / `::SUBTAB`) and `::GROUP` boxes around them.
//!
//! An input's **scope**: the one panel it drives, the one section whose
//! panels it drives, or the page (panels in more than one section). The
//! lints (`dashboard --check`, design pass):
//! - `input/unused`: nothing reads its variable;
//! - `layout/input-scope`: it stands in a section none of its panels is in,
//!   or inside one section while it drives panels of several;
//! - `layout/mixed-input-row`: one row of inputs (consecutive, or one
//!   `::GROUP`) mixes scopes — a page filter next to one chart's switch;
//! - `layout/filter-below-content`: it drives a panel above it in its own
//!   section (a reader scans top-down: filters first);
//! - `design/ungrouped-inputs`: three or more loose top-level inputs in a
//!   row, not in a `::GROUP` filter bar.

use crate::lint::{Diagnostic, Severity};
use crate::sql::{self, deps, Panel};
use crate::Role;
use std::collections::{BTreeSet, HashMap};

/// What an input drives.
#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    /// Nothing reads its variables.
    Unused,
    /// Exactly one panel (plan index).
    Panel(usize),
    /// Panels of one section (the `::TAB`/`::SUBTAB` statement, or `None`
    /// for the part above the first tab).
    Section(Option<usize>),
    /// Panels of several sections.
    Page,
}

/// One input of the plan.
#[derive(Debug, Clone)]
pub struct Input {
    pub stmt: usize,
    pub vars: Vec<String>,
    /// Plan indices of the panels (non-setup, non-input statements that
    /// draw) whose answer reads one of `vars`, directly or through a
    /// dependent input (a country dropdown filtered by this region).
    pub readers: Vec<usize>,
    pub scope: Scope,
}

fn section(p: &Panel) -> Option<usize> {
    p.tabs.last().copied()
}

fn draws(p: &Panel) -> bool {
    !p.setup
        && crate::inputs::input_kind(&p.roles).is_none()
        && p.tab_kind().is_none()
        && p.roles.iter().any(|(_, r)| r.renders())
}

/// A text panel (`::MARKDOWN`, `::TEXT_*`).
fn is_text(p: &Panel) -> bool {
    p.roles
        .iter()
        .any(|(_, r)| matches!(r, Role::Markdown | Role::Text(_)))
}

fn is_group(p: &Panel, want: Role) -> bool {
    !p.setup && p.roles.iter().any(|(_, r)| *r == want)
}

/// Every input of `plan`, with what it drives.
pub fn inputs(plan: &[Panel]) -> Vec<Input> {
    let d = reads_per_statement(plan);
    struct Raw {
        stmt: usize,
        vars: Vec<String>,
        readers: BTreeSet<usize>,
        inputs: BTreeSet<usize>,
        other: bool,
        opaque: bool,
    }
    let mut raw: Vec<Raw> = Vec::new();
    for (i, p) in plan.iter().enumerate() {
        if p.setup {
            continue;
        }
        let Some(kind) = crate::inputs::input_kind(&p.roles) else {
            continue;
        };
        let vars: Vec<String> = sql::vars::input_names(p, kind)
            .into_iter()
            .map(|n| n.to_ascii_lowercase())
            .collect();
        if vars.is_empty() {
            continue;
        }
        // a statement the analysis can't see through (`None`) uses every
        // input, but drives no placement
        let reads = |j: usize| {
            j != i
                && !plan[j].setup
                && d[j]
                    .as_ref()
                    .is_some_and(|r| vars.iter().any(|v| r.contains(v)))
        };
        let opaque = (0..plan.len()).any(|j| j != i && !plan[j].setup && d[j].is_none());
        let is_input = |j: usize| crate::inputs::input_kind(&plan[j].roles).is_some();
        raw.push(Raw {
            stmt: i,
            readers: (0..plan.len())
                .filter(|&j| draws(&plan[j]) && reads(j))
                .collect(),
            inputs: (0..plan.len())
                .filter(|&j| is_input(j) && reads(j))
                .collect(),
            // a ::SHOW_IF / ::GATE / a tab's @requires reading it
            other: (0..plan.len()).any(|j| reads(j) && !draws(&plan[j]) && !is_input(j)),
            opaque,
            vars,
        });
    }
    // a dependent input (country reads region) passes on what it drives
    loop {
        let mut grew = false;
        for k in 0..raw.len() {
            let extra: BTreeSet<usize> = raw
                .iter()
                .filter(|r| raw[k].inputs.contains(&r.stmt))
                .flat_map(|r| r.readers.iter().copied())
                .collect();
            let before = raw[k].readers.len();
            raw[k].readers.extend(extra);
            grew |= raw[k].readers.len() != before;
        }
        if !grew {
            break;
        }
    }
    let mut out = Vec::new();
    for r in raw {
        let p = &plan[r.stmt];
        let readers: Vec<usize> = r.readers.into_iter().collect();
        let sections: BTreeSet<Option<usize>> =
            readers.iter().map(|&j| section(&plan[j])).collect();
        let scope = match (readers.as_slice(), sections.len()) {
            ([], _) if r.other || r.opaque || !r.inputs.is_empty() => Scope::Section(section(p)),
            ([], _) => Scope::Unused,
            ([one], _) => Scope::Panel(*one),
            (_, 1) => Scope::Section(*sections.iter().next().unwrap()),
            _ => Scope::Page,
        };
        out.push(Input {
            stmt: r.stmt,
            vars: r.vars,
            readers,
            scope,
        });
    }
    out
}

/// Per statement, the variables its own text reads (`None`: it can't be
/// seen through) — precise, unlike the server's cache key
/// ([`deps::script_deps`], which adds a setup statement's reads to every
/// later one): through the macros and views the plan itself defines, and
/// through `SET VARIABLE x = …getvariable('y')…` (reading `x` reads `y`).
fn reads_per_statement(plan: &[Panel]) -> Vec<Option<BTreeSet<String>>> {
    let mut objects = Vec::new();
    let mut assigns: Vec<(String, String)> = Vec::new();
    for p in plan.iter().filter(|p| p.setup) {
        if let Some(n) = created_name(&p.sql) {
            objects.push((n, p.sql.clone()));
        } else if let Some((n, expr)) = assigned(&p.sql) {
            assigns.push((n, expr));
        }
    }
    let implicit = deps::catalog_reads(&objects);
    let set = |sql: &str| match deps::reads(sql, &implicit) {
        deps::Deps::All => None,
        deps::Deps::Vars(v) => Some(v),
    };
    let assigned: Vec<(String, Option<BTreeSet<String>>)> =
        assigns.iter().map(|(n, e)| (n.clone(), set(e))).collect();
    plan.iter()
        .map(|p| {
            let mut r = set(&p.sql)?;
            loop {
                let extra: Vec<String> = assigned
                    .iter()
                    .filter(|(n, _)| r.contains(n))
                    .flat_map(|(_, v)| v.iter().flatten().cloned())
                    .filter(|v| !r.contains(v))
                    .collect();
                if extra.is_empty() {
                    break;
                }
                r.extend(extra);
            }
            Some(r)
        })
        .collect()
}

/// `CREATE [OR REPLACE] [TEMP] MACRO|VIEW|TABLE [IF NOT EXISTS] <name>` →
/// the name (lower case, unqualified).
fn created_name(sql: &str) -> Option<String> {
    let toks: Vec<_> = crate::sql::lex::tokenize(sql)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    let w = |i: usize, kw: &str| toks.get(i).is_some_and(|t| t.is_word(sql, kw));
    if !w(0, "create") {
        return None;
    }
    let mut i = 1;
    if w(i, "or") && w(i + 1, "replace") {
        i += 2;
    }
    if w(i, "temp") || w(i, "temporary") {
        i += 1;
    }
    if !(w(i, "macro") || w(i, "view") || w(i, "table") || w(i, "function")) {
        return None;
    }
    i += 1;
    if w(i, "if") && w(i + 1, "not") && w(i + 2, "exists") {
        i += 3;
    }
    // a qualified name: its last part
    let mut name = toks
        .get(i)?
        .text(sql)
        .trim_matches('"')
        .to_ascii_lowercase();
    while toks.get(i + 1).is_some_and(|t| t.is_punct(sql, '.')) {
        i += 2;
        name = toks
            .get(i)?
            .text(sql)
            .trim_matches('"')
            .to_ascii_lowercase();
    }
    Some(name)
}

/// `SET VARIABLE <name> = <expr>` → (name, expr).
fn assigned(sql: &str) -> Option<(String, String)> {
    let toks: Vec<_> = crate::sql::lex::tokenize(sql)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    let w = |i: usize, kw: &str| toks.get(i).is_some_and(|t| t.is_word(sql, kw));
    if !(w(0, "set") && w(1, "variable")) {
        return None;
    }
    let name = toks
        .get(2)?
        .text(sql)
        .trim_matches('"')
        .to_ascii_lowercase();
    let eq = toks.get(3)?;
    if !(eq.is_punct(sql, '=') || eq.is_word(sql, "to")) {
        return None;
    }
    Some((name, format!("SELECT {}", &sql[eq.end..])))
}

fn diag(
    severity: Severity,
    i: usize,
    code: &'static str,
    message: String,
    plan: &[Panel],
) -> Diagnostic {
    let sql: String = plan[i].sql.split_whitespace().collect::<Vec<_>>().join(" ");
    Diagnostic {
        severity,
        stmt: i + 1,
        code,
        message,
        sql: sql.chars().take(80).collect(),
    }
}

fn scope_word(s: &Scope, plan: &[Panel]) -> String {
    let label = |i: usize| {
        crate::sql::vars::literal_value(&plan[i])
            .map(|l| format!("'{l}'"))
            .unwrap_or_else(|| format!("statement {}", i + 1))
    };
    match s {
        Scope::Unused => "nothing".into(),
        Scope::Panel(j) => format!("one panel (statement {})", j + 1),
        Scope::Section(Some(t)) => format!("the {} tab", label(*t)),
        Scope::Section(None) => "the panels above the first tab".into(),
        Scope::Page => "panels on several tabs".into(),
    }
}

/// The input-layout lints of `plan` (see the module docs).
pub fn diags(plan: &[Panel]) -> Vec<Diagnostic> {
    let ins = inputs(plan);
    let mut out = Vec::new();
    // a script of inputs only (a snippet, a form) has nothing to drive yet
    let has_panels = plan.iter().any(draws);
    for inp in &ins {
        let i = inp.stmt;
        let here = section(&plan[i]);
        let vars = inp.vars.join(", ");
        match &inp.scope {
            Scope::Unused if has_panels => out.push(diag(
                Severity::Note,
                i,
                "input/unused",
                format!("no statement of this dashboard reads this input's variable ({vars}), so it seems to change nothing — read it with getvariable('{}') or drop it (a macro defined outside the dashboard, e.g. in serve --init, can't be seen here)", inp.vars[0]),
                plan,
            )),
            Scope::Page if here.is_some() => out.push(diag(
                Severity::Warning,
                i,
                "layout/input-scope",
                format!("this input ({vars}) drives panels on several tabs but stands inside one: put it above the first ::TAB, where it reads as a filter for the whole page"),
                plan,
            )),
            Scope::Panel(j) if section(&plan[*j]) != here => out.push(diag(
                Severity::Warning,
                i,
                "layout/input-scope",
                format!("this input ({vars}) drives only statement {}, in another section: put it right above that panel", j + 1),
                plan,
            )),
            Scope::Section(s) if *s != here && here.is_some() => out.push(diag(
                Severity::Warning,
                i,
                "layout/input-scope",
                format!("this input ({vars}) drives only {}, not the section it stands in: move it there", scope_word(&inp.scope, plan)),
                plan,
            )),
            _ => {}
        }
        // a reader above it, in its own section
        // (prose above it — an intro quoting the input's value — is no panel
        // it filters)
        if let Some(&j) = inp
            .readers
            .iter()
            .find(|&&j| j < i && section(&plan[j]) == here && !is_text(&plan[j]))
        {
            out.push(diag(
                Severity::Warning,
                i,
                "layout/filter-below-content",
                format!("this input ({vars}) filters statement {}, which stands above it: put inputs before the panels they drive", j + 1),
                plan,
            ));
        }
    }
    // rows of inputs: consecutive inputs with only directives between them,
    // or the inputs of one ::GROUP box
    let by_stmt: HashMap<usize, &Input> = ins.iter().map(|x| (x.stmt, x)).collect();
    let mut rows: Vec<(Vec<usize>, bool)> = Vec::new(); // (inputs, in a group)
    let mut cur: Vec<usize> = Vec::new();
    let mut in_group = false;
    let flush = |cur: &mut Vec<usize>, rows: &mut Vec<(Vec<usize>, bool)>, g: bool| {
        if !cur.is_empty() {
            rows.push((std::mem::take(cur), g));
        }
    };
    for (i, p) in plan.iter().enumerate() {
        if p.setup {
            continue;
        }
        if is_group(p, Role::GroupStart) {
            flush(&mut cur, &mut rows, in_group);
            in_group = true;
        } else if is_group(p, Role::GroupEnd) {
            flush(&mut cur, &mut rows, in_group);
            in_group = false;
        } else if by_stmt.contains_key(&i) {
            cur.push(i);
        } else if draws(p) || p.tab_kind().is_some() {
            flush(&mut cur, &mut rows, in_group);
        }
    }
    flush(&mut cur, &mut rows, in_group);
    for (row, grouped) in &rows {
        let scopes: Vec<&Scope> = row
            .iter()
            .map(|i| &by_stmt[i].scope)
            .filter(|s| **s != Scope::Unused)
            .collect();
        let first = scopes.first().copied();
        if let Some(other) = scopes.iter().find(|s| Some(**s) != first) {
            let a = row[0];
            out.push(diag(
                Severity::Warning,
                a,
                "layout/mixed-input-row",
                format!(
                    "this row of inputs mixes what they control: one drives {}, another {} — put a page or section filter in the filter bar and a switch for one panel right above that panel",
                    scope_word(first.unwrap(), plan),
                    scope_word(other, plan)
                ),
                plan,
            ));
        }
        let top = plan[row[0]].tabs.is_empty();
        if !grouped && top && row.len() >= 3 {
            out.push(diag(
                Severity::Note,
                row[0],
                "design/ungrouped-inputs",
                format!("{} inputs in a row at top level — wrap them in a 'Filters'::GROUP … ::ENDGROUP box so they read as one filter bar", row.len()),
                plan,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(script: &str) -> Vec<(&'static str, usize)> {
        diags(&sql::plan(script))
            .into_iter()
            .map(|d| (d.code, d.stmt))
            .collect()
    }

    const DATA: &str = "CREATE TABLE t AS SELECT 'a' AS region, 'x' AS ch, 1 AS v;\n";

    #[test]
    fn a_good_layout_is_quiet() {
        let s = format!(
            "{DATA}SELECT 'Filters'::GROUP;\n\
             SELECT DISTINCT region AS region ::DROPDOWN FROM t;\n\
             SELECT DISTINCT ch AS ch ::DROPDOWN FROM t;\n\
             SELECT 1::ENDGROUP;\n\
             SELECT 'Sales'::TAB;\n\
             SELECT region::XAXIS, sum(v)::BARCHART FROM t WHERE region = getvariable('region') AND ch = getvariable('ch') GROUP BY 1;\n\
             SELECT 'Units'::TAB;\n\
             SELECT region::XAXIS, sum(v)::LINECHART FROM t WHERE region = getvariable('region') AND ch = getvariable('ch') GROUP BY 1;\n"
        );
        assert_eq!(codes(&s), vec![]);
    }

    #[test]
    fn unused_and_misplaced_inputs_are_named() {
        let s = format!(
            "{DATA}SELECT DISTINCT region AS region ::DROPDOWN FROM t;\n\
             SELECT DISTINCT ch AS nobody ::DROPDOWN FROM t;\n\
             SELECT region::XAXIS, sum(v)::BARCHART FROM t WHERE region = getvariable('region') GROUP BY 1;\n\
             SELECT 'Detail'::TAB;\n\
             SELECT DISTINCT ch AS metric ::BUTTONS FROM t;\n\
             SELECT 'Other'::TAB;\n\
             SELECT ch::XAXIS, sum(v)::BARCHART FROM t WHERE ch = getvariable('metric') GROUP BY 1;\n"
        );
        let c = codes(&s);
        assert!(c.contains(&("input/unused", 3)), "{c:?}");
        assert!(c.contains(&("layout/input-scope", 6)), "{c:?}");
        // region (page top, one panel) next to `nobody` (unused): not mixed
        assert!(!c.iter().any(|x| x.0 == "layout/mixed-input-row"), "{c:?}");
    }

    #[test]
    fn a_page_filter_next_to_one_charts_switch_is_mixed() {
        let s = format!(
            "{DATA}SELECT DISTINCT region AS region ::DROPDOWN FROM t;\n\
             SELECT 'Units' AS measure ::BUTTONS;\n\
             SELECT region::XAXIS, sum(v)::BARCHART FROM t WHERE region = getvariable('region') GROUP BY 1;\n\
             SELECT ch::XAXIS, CASE getvariable('measure') WHEN 'Units' THEN sum(v) END::LINECHART FROM t WHERE region = getvariable('region') GROUP BY 1;\n\
             SELECT ch::XAXIS, sum(v)::BARCHART FROM t WHERE region = getvariable('region') GROUP BY 1;\n"
        );
        let c = codes(&s);
        assert!(c.contains(&("layout/mixed-input-row", 2)), "{c:?}");
    }

    #[test]
    fn a_filter_below_what_it_filters() {
        let s = format!(
            "{DATA}SELECT region::XAXIS, sum(v)::BARCHART FROM t WHERE region = getvariable('region') GROUP BY 1;\n\
             SELECT ch::XAXIS, sum(v)::BARCHART FROM t WHERE region = getvariable('region') GROUP BY 1;\n\
             SELECT DISTINCT region AS region ::DROPDOWN FROM t;\n"
        );
        assert!(codes(&s).contains(&("layout/filter-below-content", 4)));
    }

    #[test]
    fn three_loose_inputs_suggest_a_filter_bar() {
        let s = format!(
            "{DATA}SELECT DISTINCT region AS a ::DROPDOWN FROM t;\n\
             SELECT DISTINCT region AS b ::DROPDOWN FROM t;\n\
             SELECT DISTINCT region AS c ::DROPDOWN FROM t;\n\
             SELECT 'S'::TAB;\n\
             SELECT region::XAXIS, sum(v)::BARCHART FROM t WHERE region IN (getvariable('a'), getvariable('b'), getvariable('c')) GROUP BY 1;\n\
             SELECT 'T'::TAB;\n\
             SELECT region::XAXIS, sum(v)::LINECHART FROM t WHERE region IN (getvariable('a'), getvariable('b'), getvariable('c')) GROUP BY 1;\n"
        );
        let c = codes(&s);
        assert_eq!(c, vec![("design/ungrouped-inputs", 2)], "{c:?}");
    }

    #[test]
    fn a_dependent_dropdown_counts_as_use() {
        let s = format!(
            "{DATA}SELECT DISTINCT region AS region ::DROPDOWN FROM t;\n\
             SELECT DISTINCT ch AS ch ::DROPDOWN FROM t WHERE region = getvariable('region');\n\
             SELECT ch::XAXIS, sum(v)::BARCHART FROM t WHERE ch = getvariable('ch') GROUP BY 1;\n"
        );
        let c = codes(&s);
        assert!(!c.iter().any(|x| x.0 == "input/unused"), "{c:?}");
    }
}
