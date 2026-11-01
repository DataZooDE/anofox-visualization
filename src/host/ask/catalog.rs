//! The static catalog (plan-ai-assistant.md §12.2, security review H1): what
//! the model knows of a dashboard before it reads a panel — built from the
//! dashboard's **source text only**, so its bytes are the same for every
//! viewer and it can sit in the shared, cached prompt prefix.
//!
//! Per panel: its index, its tab (when the tab's label is a literal in the
//! source), its `-- @name`, its kind and its columns with their roles. Role
//! *values* (`::TITLE`, `::LABEL`, `::INFO`, `::MARKDOWN`, …) are query
//! results computed for a viewer, so they reach the model only through
//! `get_panel`; a panel whose title comes from data says so. No SQL text.
//!
//! Panels left out by the source itself (`@ask skip`, write-back,
//! `viewer_state`, [`crate::sql::ask::exclusions`]) are not in the catalog.
//! A tab hidden for one viewer is decided per request, after the cache
//! breakpoint: the catalog never changes with the viewer.

use crate::host::serving::DashboardMeta;
use crate::sql::{self, Panel};
use crate::{InputKind, Kind, Role};

/// One panel the model may ask for.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub index: usize,
    /// The tab's label (a literal in the source), else `tab <index>`.
    pub tab: Option<String>,
    pub name: Option<String>,
    pub kind: String,
    /// `(display name, role tokens)`, in output order.
    pub columns: Vec<(String, Vec<&'static str>)>,
    pub title_from_data: bool,
    pub input: Option<InputSpec>,
}

/// A variable an input panel binds (security review M3: the only variables
/// `get_panel` may set).
#[derive(Debug, Clone, PartialEq)]
pub struct InputSpec {
    pub var: String,
    pub kind: InputKind,
}

impl InputSpec {
    /// An input whose value must be one of its options.
    pub fn has_options(&self) -> bool {
        matches!(
            self.kind,
            InputKind::Dropdown | InputKind::Multiselect | InputKind::Buttons
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Catalog {
    pub title: String,
    pub description: Option<String>,
    pub entries: Vec<Entry>,
    /// The roles the dashboard uses, for the role note.
    pub roles: Vec<Role>,
}

/// The display name of each role column of statement `i` (`c{n}` → its
/// alias or bare column name): [`sql::column_labels`] of the source, then
/// the plan's own names.
pub fn labels(script: &str, plan: &[Panel], i: usize) -> Vec<(usize, String)> {
    let mut l = sql::ask::raw_statements(script)
        .get(i)
        .map(|raw| sql::column_labels(sql::strip_comments(raw).trim()))
        .unwrap_or_default();
    for (j, n) in plan.get(i).map(|p| p.names.as_slice()).unwrap_or(&[]) {
        if !l.iter().any(|(k, _)| k == j) {
            l.push((*j, n.clone()));
        }
    }
    l
}

/// A literal string a statement selects (`SELECT 'Overview' AS c0`): source
/// text, not a computed value.
pub fn static_literal(sql: &str) -> Option<String> {
    let s = sql.trim().trim_end_matches(';').trim();
    let rest = s
        .get(..6)
        .filter(|h| h.eq_ignore_ascii_case("select"))
        .map(|_| s[6..].trim_start())?;
    let body = rest.strip_prefix('\'')?;
    let mut out = String::new();
    let mut chars = body.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '\'' {
            if chars.peek().is_some_and(|(_, n)| *n == '\'') {
                chars.next();
                out.push('\'');
                continue;
            }
            let tail = body[i + 1..].trim();
            let alias_only = tail.is_empty()
                || tail
                    .strip_prefix("AS ")
                    .or_else(|| tail.strip_prefix("as "))
                    .is_some_and(|a| {
                        a.trim()
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    });
            return alias_only.then_some(out);
        }
        out.push(c);
    }
    None
}

fn kind_of(p: &Panel) -> String {
    if let Some(k) = crate::inputs::input_kind(&p.roles) {
        let t = Role::Input(k).token().to_ascii_lowercase();
        return format!("input ({t})");
    }
    if p.contract.is_some() {
        return "statistics plot".into();
    }
    let has = |f: &dyn Fn(&Role) -> bool| p.roles.iter().any(|(_, r)| f(r));
    if has(&|r| matches!(r, Role::Table | Role::PagedTable)) {
        return if has(&|r| matches!(r, Role::PagedTable)) {
            "table (paged)".into()
        } else {
            "table".into()
        };
    }
    if has(&|r| matches!(r, Role::Record)) {
        return "record".into();
    }
    if let Some(k) = p.roles.iter().find_map(|(_, r)| match r {
        Role::Value(k) => Some(*k),
        _ => None,
    }) {
        return match k {
            Kind::Sparkline | Kind::Sparkbar => "KPI".into(),
            k => format!("{} chart", Role::Value(k).token().to_ascii_lowercase()),
        };
    }
    if has(&|r| matches!(r, Role::Metric(_))) {
        return "KPI".into();
    }
    if has(&|r| matches!(r, Role::Markdown | Role::Callout)) {
        return "text".into();
    }
    "panel".into()
}

/// Is statement `p` something `get_panel` can read (a panel or an input)?
fn readable(p: &Panel) -> bool {
    !p.setup
        && (crate::inputs::input_kind(&p.roles).is_some()
            || !crate::roles::is_directive_panel(&p.roles))
}

impl Catalog {
    pub fn build(script: &str, meta: &DashboardMeta) -> Catalog {
        let plan = sql::plan(script);
        let tab_label = |i: usize| -> String {
            plan.get(i)
                .and_then(|t| static_literal(&t.sql))
                .unwrap_or_else(|| format!("tab {i}"))
        };
        // `@ask skip`, write-back and `viewer_state` panels are decided from
        // the source alone: they are not in the catalog at all.
        let excluded: Vec<usize> = sql::ask::exclusions(script)
            .into_iter()
            .map(|(i, _)| i)
            .collect();
        let mut roles: Vec<Role> = Vec::new();
        let mut entries = Vec::new();
        for (i, p) in plan.iter().enumerate() {
            if !readable(p) || excluded.contains(&i) {
                continue;
            }
            let names = labels(script, &plan, i);
            let mut columns: Vec<(String, Vec<&'static str>)> = Vec::new();
            let mut idx: Vec<usize> = p.roles.iter().map(|(c, _)| *c).collect();
            idx.sort_unstable();
            idx.dedup();
            for c in idx {
                let name = names
                    .iter()
                    .find(|(j, _)| *j == c)
                    .map_or_else(|| format!("c{c}"), |(_, n)| n.clone());
                let toks = p
                    .roles
                    .iter()
                    .filter(|(j, _)| *j == c)
                    .map(|(_, r)| {
                        if !roles.contains(r) {
                            roles.push(*r);
                        }
                        r.token()
                    })
                    .collect();
                columns.push((name, toks));
            }
            let input = crate::inputs::input_kind(&p.roles).map(|kind| {
                let c = crate::inputs::input_index(&p.roles);
                InputSpec {
                    var: names
                        .iter()
                        .find(|(j, _)| *j == c)
                        .map_or_else(|| format!("c{c}"), |(_, n)| n.clone()),
                    kind,
                }
            });
            entries.push(Entry {
                index: i,
                tab: p.tabs.last().map(|t| tab_label(*t)),
                name: p.name.clone(),
                kind: kind_of(p),
                columns,
                title_from_data: p.roles.iter().any(|(_, r)| matches!(r, Role::Title)),
                input,
            });
        }
        Catalog {
            title: meta.title.clone(),
            description: meta.description.clone(),
            entries,
            roles,
        }
    }

    pub fn entry(&self, index: usize) -> Option<&Entry> {
        self.entries.iter().find(|e| e.index == index)
    }

    /// The variables the dashboard's inputs bind (M3).
    pub fn inputs(&self) -> impl Iterator<Item = (&Entry, &InputSpec)> {
        self.entries
            .iter()
            .filter_map(|e| e.input.as_ref().map(|s| (e, s)))
    }

    /// One line per panel.
    pub fn entry_line(e: &Entry) -> String {
        let mut s = format!("[{}]", e.index);
        if let Some(t) = &e.tab {
            s.push_str(&format!(" tab \"{t}\" ·"));
        }
        if let Some(n) = &e.name {
            s.push_str(&format!(" @name {n} ·"));
        }
        s.push_str(&format!(" {}", e.kind));
        if let Some(i) = &e.input {
            s.push_str(&format!(" · sets variable {}", i.var));
        }
        let cols: Vec<String> = e
            .columns
            .iter()
            .map(|(n, r)| format!("{n} ({})", r.join(", ")))
            .collect();
        if !cols.is_empty() {
            s.push_str(&format!(" · columns: {}", cols.join("; ")));
        }
        if e.title_from_data {
            s.push_str(" · title from data");
        }
        s
    }

    /// The catalog as the model reads it.
    pub fn text(&self) -> String {
        let mut s = format!("# Dashboard: {}\n", self.title);
        if let Some(d) = &self.description {
            s.push_str(&format!("{d}\n"));
        }
        s.push_str("\n# Panels\n(index · tab · kind · columns with their roles)\n");
        for e in &self.entries {
            s.push_str(&Self::entry_line(e));
            s.push('\n');
        }
        s
    }

    /// A short glossary of only the roles this dashboard uses.
    pub fn role_note(&self) -> String {
        let mut lines: Vec<String> = self
            .roles
            .iter()
            .map(|r| {
                let spec = crate::roles::spec_of(*r);
                format!("::{} — {}", spec.token, spec.summary)
            })
            .collect();
        lines.sort();
        lines.dedup();
        format!("# Roles on this dashboard\n{}\n", lines.join("\n"))
    }
}
