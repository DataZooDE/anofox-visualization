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
    /// Plan indices of the inputs that read one of `vars` (a country
    /// dropdown filtered by this region).
    pub children: Vec<usize>,
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
            children: r.inputs.into_iter().collect(),
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

/// The input-layout lints of `script`: [`diags`], less the placement
/// advice for the inputs an `@inputs auto` page places itself (#950).
pub fn lint(script: &str) -> Vec<Diagnostic> {
    let plan = sql::plan(script);
    let mut out = diags(&plan);
    if auto_on(script) {
        let (_, placed) = placement(&plan);
        let moved: BTreeSet<usize> = placed.iter().map(|p| p.stmt + 1).collect();
        out.retain(|d| {
            !(d.code.starts_with("layout/") || d.code == "design/ungrouped-inputs")
                || !moved.contains(&d.stmt)
        });
    }
    out
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

/// `-- @inputs auto` in the header (the first statement), the last
/// `auto` / `manual` there winning (#950): the page places the loose inputs
/// by what they control ([`placement`]) instead of where they stand.
pub fn auto_on(script: &str) -> bool {
    crate::sql::ask::raw_statements(script)
        .first()
        .map(|raw| {
            raw.lines()
                .filter_map(|l| l.trim().strip_prefix("--"))
                .filter_map(|l| l.trim().strip_prefix("@inputs"))
                .filter(|rest| rest.starts_with(char::is_whitespace))
                .filter_map(|rest| match rest.trim() {
                    "auto" => Some(true),
                    "manual" => Some(false),
                    _ => None,
                })
                .next_back()
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Where an automatically placed input is drawn (#950).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum At {
    /// The page's filter bar, above everything it drives.
    Page,
    /// A section's (`::TAB` / `::SUBTAB`, or the part above the first tab)
    /// own bar, above its panels.
    Section,
    /// A bar right above the one panel it drives.
    Panel,
}

impl At {
    pub fn as_str(self) -> &'static str {
        match self {
            At::Page => "page",
            At::Section => "section",
            At::Panel => "panel",
        }
    }
}

/// One input [`placement`] moved.
#[derive(Debug, Clone, PartialEq)]
pub struct Placed {
    pub stmt: usize,
    pub at: At,
    /// The bar it is drawn in: inputs with the same number share one row.
    pub bar: usize,
    /// [`At::Panel`]: that panel's plan index (its bar goes into its title).
    pub target: Option<usize>,
}

/// The draw order of an `@inputs auto` dashboard: every plan index once,
/// with each loose input (and the directives right before it, a ::SPAN or
/// ::SHOW_IF) moved to what it controls — the page's filter bar (its panels
/// are on several tabs), its section's bar (the top of that ::TAB /
/// ::SUBTAB, after any intro text), or a bar right above its one panel.
/// Inside a bar: time (::PERIOD / ::DATERANGE / ::DATE), place, product and
/// customer, other inputs, then measure / dimension switches; an input that
/// another one filters (country by region) right after it.
///
/// An input stays where it stands when the author placed it: inside a
/// ::GROUP box, a ::VIEW alternative, a ::GUIDE_STEP or a ::FIGURE; a
/// ::FORM (a card of its own); one nothing reads.
pub fn placement(plan: &[Panel]) -> (Vec<usize>, Vec<Placed>) {
    let ins = inputs(plan);
    let n = plan.len();
    let mut grouped = vec![false; n];
    let mut open = false;
    for (i, p) in plan.iter().enumerate() {
        if is_group(p, Role::GroupStart) {
            open = true;
        } else if is_group(p, Role::GroupEnd) {
            open = false;
        }
        grouped[i] = open;
    }
    let movable = |inp: &Input| {
        let p = &plan[inp.stmt];
        !grouped[inp.stmt]
            && p.view.is_none()
            && p.step.is_none()
            && p.figure.is_none()
            && inp.scope != Scope::Unused
            && crate::inputs::input_kind(&p.roles) != Some(crate::InputKind::Form)
    };
    let moved_inputs: Vec<&Input> = ins.iter().filter(|x| movable(x)).collect();
    // what moves with each input: the directives right before it
    let mut moved = vec![false; n];
    let mut units: Vec<(usize, Vec<usize>)> = Vec::new(); // (input, its statements)
    for inp in &moved_inputs {
        let mut k = inp.stmt;
        while k > 0 && is_modifier(&plan[k - 1]) && !moved[k - 1] {
            k -= 1;
        }
        let stmts: Vec<usize> = (k..=inp.stmt).collect();
        for &j in &stmts {
            moved[j] = true;
        }
        units.push((inp.stmt, stmts));
    }
    // a statement the inputs are drawn above
    let content = |j: usize| {
        let p = &plan[j];
        !moved[j]
            && !p.setup
            && ((draws(p) && !is_text(p) && !p.roles.iter().any(|(_, r)| *r == Role::Track))
                || crate::inputs::input_kind(&p.roles).is_some()
                || p.tab_kind().is_some()
                || p.roles.iter().any(|(_, r)| {
                    matches!(
                        r,
                        Role::GroupStart
                            | Role::View
                            | Role::FigureStart
                            | Role::GuideStep
                            | Role::Placeholder
                    )
                }))
    };
    // the first statement of `j`'s unit: its group's start, then the
    // directives right before that (a ::SPAN, a ::HEIGHT)
    let unit_start = |j: usize| {
        let p = &plan[j];
        let mut k = p
            .view
            .as_ref()
            .map(|v| v.group)
            .or(p.step.as_ref().map(|s| s.group))
            .or(p.figure)
            .unwrap_or(j)
            .min(j);
        while k > 0 && is_modifier(&plan[k - 1]) && !moved[k - 1] {
            k -= 1;
        }
        k
    };
    let top = || (0..n).find(|&j| content(j)).map(unit_start).unwrap_or(n);
    let mut slots: Vec<(usize, At, usize, Vec<usize>)> = Vec::new(); // (before, at, key, unit)
    for (inp, (_, stmts)) in moved_inputs.iter().zip(units) {
        let (pos, at, key) = match &inp.scope {
            Scope::Page => (top(), At::Page, n),
            Scope::Section(None) => (top(), At::Section, n),
            Scope::Section(Some(t)) => {
                let sub = plan[*t].tab_kind() == Some(true);
                let end = (t + 1..n)
                    .find(|&k| match plan[k].tab_kind() {
                        Some(true) => sub,
                        Some(false) => true,
                        None => false,
                    })
                    .unwrap_or(n);
                let pos = (t + 1..end)
                    .find(|&j| content(j))
                    .map(|j| unit_start(j).max(t + 1))
                    .unwrap_or(end);
                (pos, At::Section, *t)
            }
            Scope::Panel(j) => {
                // a ::TRACK strip is drawn with the chart above it
                let track = |k: usize| plan[k].roles.iter().any(|(_, r)| *r == Role::Track);
                let mut j = *j;
                while track(j) {
                    match (0..j).rev().find(|&k| draws(&plan[k])) {
                        Some(k) => j = k,
                        None => break,
                    }
                }
                (unit_start(j), At::Panel, j)
            }
            Scope::Unused => unreachable!(),
        };
        slots.push((pos, at, key, stmts));
    }
    // bars: one per (place, scope, target)
    let mut keys: Vec<(usize, At, usize)> = slots.iter().map(|s| (s.0, s.1, s.2)).collect();
    keys.sort();
    keys.dedup();
    let rank_of: HashMap<usize, u8> = moved_inputs
        .iter()
        .map(|x| (x.stmt, rank(&plan[x.stmt], &x.vars)))
        .collect();
    let children: HashMap<usize, &Vec<usize>> =
        moved_inputs.iter().map(|x| (x.stmt, &x.children)).collect();
    let mut by_slot: HashMap<usize, Vec<Vec<usize>>> = HashMap::new(); // pos → units in draw order
    let mut placed = Vec::new();
    for (bar, key) in keys.iter().enumerate() {
        let mut items: Vec<&(usize, At, usize, Vec<usize>)> =
            slots.iter().filter(|s| (s.0, s.1, s.2) == *key).collect();
        let input_of = |s: &(usize, At, usize, Vec<usize>)| *s.3.last().unwrap();
        items.sort_by_key(|s| (rank_of[&input_of(s)], input_of(s)));
        let here: BTreeSet<usize> = items.iter().map(|s| input_of(s)).collect();
        let parent_here = |i: usize| {
            items
                .iter()
                .any(|s| children[&input_of(s)].contains(&i) && input_of(s) != i)
        };
        // depth-first: a filtered input right after the one filtering it
        let mut done = BTreeSet::new();
        let mut seq = Vec::new();
        fn visit(
            i: usize,
            here: &BTreeSet<usize>,
            children: &HashMap<usize, &Vec<usize>>,
            done: &mut BTreeSet<usize>,
            seq: &mut Vec<usize>,
        ) {
            if !done.insert(i) {
                return;
            }
            seq.push(i);
            for &c in children[&i].iter() {
                if here.contains(&c) {
                    visit(c, here, children, done, seq);
                }
            }
        }
        for s in &items {
            let i = input_of(s);
            if !parent_here(i) {
                visit(i, &here, &children, &mut done, &mut seq);
            }
        }
        for s in &items {
            visit(input_of(s), &here, &children, &mut done, &mut seq);
        }
        let entry = by_slot.entry(key.0).or_default();
        for i in seq {
            let s = items.iter().find(|s| input_of(s) == i).unwrap();
            entry.push(s.3.clone());
            placed.push(Placed {
                stmt: i,
                at: key.1,
                bar,
                target: (key.1 == At::Panel).then_some(key.2),
            });
        }
    }
    let mut order = Vec::with_capacity(n);
    for (k, m) in moved.iter().map(Some).chain([None]).enumerate() {
        for u in by_slot.get(&k).into_iter().flatten() {
            order.extend(u);
        }
        if m == Some(&false) {
            order.push(k);
        }
    }
    (order, placed)
}

/// A directive that applies to the statement after it (::SPAN, ::HEIGHT,
/// ::SHOW_IF, a table's ::TOTALS …), not a structure of its own.
fn is_modifier(p: &Panel) -> bool {
    !p.setup
        && crate::inputs::input_kind(&p.roles).is_none()
        && p.tab_kind().is_none()
        && p.view.as_ref().is_none_or(|v| !v.marker)
        && crate::roles::is_directive_panel(&p.roles)
        && !p.roles.iter().any(|(_, r)| {
            crate::roles::is_chrome_directive(*r)
                || matches!(
                    r,
                    Role::GroupStart
                        | Role::GroupEnd
                        | Role::View
                        | Role::ViewEnd
                        | Role::FigureStart
                        | Role::FigureEnd
                        | Role::GuideStep
                        | Role::Columns
                        | Role::Fit
                        | Role::Section
                        | Role::Placeholder
                )
        })
}

/// An input's place in a bar: time, place, product / customer, other,
/// then switches (#950).
fn rank(p: &Panel, vars: &[String]) -> u8 {
    use crate::InputKind as K;
    let kind = crate::inputs::input_kind(&p.roles);
    if matches!(kind, Some(K::Period | K::DateRange | K::Date)) {
        return 0;
    }
    let has = |words: &[&str]| {
        vars.iter().any(|v| {
            v.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| {
                words
                    .iter()
                    .any(|x| w == *x || w.strip_suffix('s') == Some(x))
            })
        })
    };
    if has(&[
        "date", "day", "week", "month", "quarter", "year", "period", "from", "to", "since", "until",
    ]) {
        return 0;
    }
    if has(&[
        "region",
        "country",
        "state",
        "city",
        "location",
        "site",
        "store",
        "market",
        "warehouse",
        "plant",
        "territory",
        "zone",
        "area",
        "geo",
        "branch",
        "depot",
        "land",
        "standort",
    ]) {
        return 1;
    }
    if has(&[
        "product", "sku", "item", "category", "brand", "customer", "client", "segment", "channel",
        "account", "supplier", "vendor", "artikel", "kunde", "produkt",
    ]) {
        return 2;
    }
    if matches!(kind, Some(K::Buttons | K::Toggle))
        || has(&[
            "measure",
            "metric",
            "kpi",
            "dimension",
            "by",
            "mode",
            "view",
            "show",
            "split",
        ])
    {
        return 4;
    }
    3
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

    fn placed(script: &str) -> (Vec<usize>, Vec<(usize, &'static str, usize)>) {
        let (order, p) = placement(&sql::plan(script));
        (
            order,
            p.into_iter()
                .map(|x| (x.stmt, x.at.as_str(), x.bar))
                .collect(),
        )
    }

    #[test]
    fn auto_is_a_header_switch() {
        assert!(auto_on("-- @inputs auto\nSELECT 1::TEXT;"));
        assert!(!auto_on(
            "-- @inputs auto\n-- @inputs manual\nSELECT 1::TEXT;"
        ));
        assert!(!auto_on(
            "SELECT 1::TEXT;\n-- @inputs auto\nSELECT 2::TEXT;"
        ));
        assert!(!auto_on("-- @inputsauto\nSELECT 1::TEXT;"));
    }

    #[test]
    fn inputs_go_to_what_they_control() {
        // 0 table, 1 '# Sales' intro, 2 chart switch (one panel), 3 tab A,
        // 4 tab filter, 5 chart, 6 chart(reads 2), 7 tab B, 8 region (page),
        // 9 chart
        let s = format!(
            "{DATA}SELECT '# Sales'::MARKDOWN;\n\
             SELECT 'Units' AS measure ::BUTTONS;\n\
             SELECT 'A'::TAB;\n\
             SELECT DISTINCT ch AS ch ::DROPDOWN FROM t;\n\
             SELECT region::XAXIS, sum(v)::BARCHART FROM t WHERE region = getvariable('region') AND ch = getvariable('ch') GROUP BY 1;\n\
             SELECT region::XAXIS, CASE getvariable('measure') WHEN 'Units' THEN sum(v) END::LINECHART FROM t WHERE ch = getvariable('ch') GROUP BY 1;\n\
             SELECT 'B'::TAB;\n\
             SELECT DISTINCT region AS region ::DROPDOWN FROM t;\n\
             SELECT region::XAXIS, sum(v)::BARCHART FROM t WHERE region = getvariable('region') GROUP BY 1;\n"
        );
        let (order, p) = placed(&s);
        // the page filter goes above the tabs, after the intro text; the tab
        // filter to its tab's top; the switch right above its chart
        assert_eq!(order, vec![0, 1, 8, 3, 4, 5, 2, 6, 7, 9]);
        let at: Vec<(usize, &str)> = p.iter().map(|x| (x.0, x.1)).collect();
        assert!(at.contains(&(8, "page")), "{p:?}");
        assert!(at.contains(&(4, "section")), "{p:?}");
        assert!(at.contains(&(2, "panel")), "{p:?}");
    }

    #[test]
    fn a_bar_orders_time_place_product_switches_and_keeps_linked_levels() {
        let s = format!(
            "{DATA}SELECT 'Units' AS measure ::BUTTONS;\n\
             SELECT DISTINCT ch AS product ::DROPDOWN FROM t;\n\
             SELECT DISTINCT ch AS country ::DROPDOWN FROM t WHERE region = getvariable('region');\n\
             SELECT DATE '2026-01-01' AS day ::DATE;\n\
             SELECT DISTINCT region AS region ::DROPDOWN FROM t;\n\
             SELECT region::XAXIS, sum(v)::BARCHART FROM t WHERE ch = getvariable('country') AND ch = getvariable('product') AND getvariable('day') IS NOT NULL GROUP BY 1, getvariable('measure');\n\
             SELECT region::XAXIS, sum(v)::LINECHART FROM t WHERE ch = getvariable('country') AND ch = getvariable('product') AND getvariable('day') IS NOT NULL GROUP BY 1, getvariable('measure');\n"
        );
        let (order, p) = placed(&s);
        // day, region, country (filtered by region), product, measure
        assert_eq!(order, vec![0, 4, 5, 3, 2, 1, 6, 7]);
        assert!(p.iter().all(|x| x.2 == p[0].2), "one bar: {p:?}");
    }

    #[test]
    fn grouped_and_unused_inputs_stay_and_a_span_moves_along() {
        let s = format!(
            "{DATA}SELECT 'Filters'::GROUP;\n\
             SELECT DISTINCT region AS region ::DROPDOWN FROM t;\n\
             SELECT 1::ENDGROUP;\n\
             SELECT region::XAXIS, sum(v)::BARCHART FROM t WHERE region = getvariable('region') GROUP BY 1;\n\
             SELECT 6::SPAN;\n\
             SELECT DISTINCT ch AS ch ::DROPDOWN FROM t;\n\
             SELECT DISTINCT ch AS nobody ::DROPDOWN FROM t;\n\
             SELECT 6::SPAN;\n\
             SELECT ch::XAXIS, sum(v)::BARCHART FROM t WHERE ch = getvariable('ch') GROUP BY 1;\n"
        );
        let (order, p) = placed(&s);
        assert_eq!(p.iter().map(|x| x.0).collect::<Vec<_>>(), vec![6]);
        // ch (with its ::SPAN) goes before the second chart's own ::SPAN
        assert_eq!(order, vec![0, 1, 2, 3, 4, 7, 5, 6, 8, 9]);
    }

    #[test]
    fn auto_drops_the_placement_advice_for_what_it_moves() {
        let body = format!(
            "{DATA}SELECT region::XAXIS, sum(v)::BARCHART FROM t WHERE region = getvariable('region') GROUP BY 1;\n\
             SELECT ch::XAXIS, sum(v)::BARCHART FROM t WHERE region = getvariable('region') GROUP BY 1;\n\
             SELECT DISTINCT region AS region ::DROPDOWN FROM t;\n"
        );
        let has = |s: &str| {
            lint(s)
                .iter()
                .any(|d| d.code == "layout/filter-below-content")
        };
        assert!(has(&body));
        assert!(!has(&format!("-- @inputs auto\n{body}")));
    }
}
