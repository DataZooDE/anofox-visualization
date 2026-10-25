//! `::GUIDE_STEP` (aliases `::TUTORIAL_STEP`, `::WIZARD_STEP`): a step of a
//! guided sequence — Back/Next, "Step 2 of 5", `?v.<name>=n` (#516).
//!
//! A **step group** is a run of `::GUIDE_STEP` markers inside one top-level
//! section: the whole dashboard when there are no tabs, otherwise the
//! current `::TAB`. Unlike a `::VIEW` group (whose markers are consecutive
//! alternatives), a step group's markers are spread out — each marker opens
//! the next step, and everything up to the next marker or the group's close
//! is that step's content.
//!
//! A group closes at the next `::TAB`, a `::SUBTAB` (`step/mixed-with-
//! subtab`, a warning — the owner principle: a `::SUBTAB` always ends a
//! step sequence, it never nests inside one), or end of script. A marker
//! reached while inside a `::SUBTAB` section never opens or extends a
//! group at all (`step/inside-subtab`, an error — [`scan`]'s
//! `inside_subtab` list; the renderer draws it as a plain `::SECTION`
//! heading instead).
//!
//! [`assign_steps`] fills every statement's [`super::Panel::step`] (called
//! from [`super::plan`], after [`super::views::assign_views`]); [`scan`] is
//! its shared analysis, also used by the `step/*` lint pass
//! (`crate::lint`) and by the static renderers' flattening ([`StaticSteps`]).

use super::views::{literal_text, valid_id};
use super::Panel;
use crate::Role;

/// One statement's place in a `::GUIDE_STEP` group — set on every statement
/// of the group, setup included (see [`assign_steps`]).
#[derive(Debug, Clone, PartialEq)]
pub struct StepAt {
    /// The plan index of the group's first `::GUIDE_STEP` marker.
    pub group: usize,
    /// 0-based position among the group's markers (hidden steps included —
    /// see the module docs; a shown, 1-based step number is computed at
    /// render time, since `-- @requires` hiding needs a query).
    pub index: usize,
    /// `true` only for the `::GUIDE_STEP` statement itself, not the other
    /// statements of its step.
    pub marker: bool,
    /// The variable this group's current step is kept in (decision 4 of
    /// plan-516.md): the alias of the group's first marker that is a valid
    /// identifier, else `"step"` for the first unnamed group on the
    /// dashboard, `"step_<n>"` for the n-th unnamed one after it.
    pub var: String,
    /// This step's label — the literal text of its marker's value (a plain
    /// `'…'`/`E'…'` string). `None` for a computed label (not a lint error
    /// by itself — `step/computed-label` is a note, like `view/computed-
    /// label`).
    pub label: Option<String>,
}

/// One `::GUIDE_STEP` group, as [`scan`] resolved it.
#[derive(Debug, Clone)]
pub struct StepGroup {
    /// Plan index of the first marker (== [`StepAt::group`] of every
    /// statement in it).
    pub start: usize,
    /// Plan index just past the group (exclusive): the closer's index
    /// (`::TAB`/`::SUBTAB`), or the plan length when end of script closed
    /// it.
    pub end: usize,
    /// Plan indices of its `::GUIDE_STEP` markers, in order.
    pub markers: Vec<usize>,
    pub var: String,
    /// `var` came from a marker's alias; `false` means the positional
    /// fallback (`"step"` / `"step_<n>"`) was used.
    pub named: bool,
    /// Closed by an explicit `::SUBTAB` while still open — `step/mixed-
    /// with-subtab`.
    pub mixed_with_subtab: bool,
}

/// [`scan`]'s result.
#[derive(Debug, Clone, Default)]
pub struct Scan {
    pub groups: Vec<StepGroup>,
    /// Plan indices of `::GUIDE_STEP` markers found while inside a
    /// `::SUBTAB` section — never open or extend a group (`step/inside-
    /// subtab`, an error; the renderer draws these as a plain `::SECTION`
    /// heading).
    pub inside_subtab: Vec<usize>,
}

fn has_role(p: &Panel, want: Role) -> bool {
    p.roles.iter().any(|(_, r)| *r == want)
}

/// The output position of a statement's own `::GUIDE_STEP` cast, if any.
fn marker_role_idx(p: &Panel) -> Option<usize> {
    p.roles
        .iter()
        .find(|(_, r)| *r == Role::GuideStep)
        .map(|(i, _)| *i)
}

/// A `::GUIDE_STEP` marker's own alias (`'Intro' AS lesson ::GUIDE_STEP` →
/// `"lesson"`). `None` for an unaliased marker.
pub fn marker_alias(p: &Panel) -> Option<String> {
    let idx = marker_role_idx(p)?;
    p.names
        .iter()
        .find(|(j, _)| *j == idx)
        .map(|(_, n)| n.clone())
}

/// A `::GUIDE_STEP` marker's own label: its select item's literal text (see
/// [`super::views::literal_text`]), read back from the rewritten `p.sql` at
/// the role's output position.
fn marker_literal(p: &Panel, idx: usize) -> Option<String> {
    let (start, end) = super::main_select_list(&p.sql)?;
    let items = super::lex::split_top_commas(&p.sql[start..end]);
    let item = items.get(idx)?.trim();
    let core = item.strip_suffix(&format!(" AS c{idx}"))?.trim();
    literal_text(core)
}

/// Scan a plan for `::GUIDE_STEP` groups (the module docs' closer rules).
/// Used by [`assign_steps`] and the `step/*` lint pass (`crate::lint`).
pub fn scan(panels: &[Panel]) -> Scan {
    struct Open {
        start: usize,
        markers: Vec<usize>,
    }

    let mut open: Option<Open> = None;
    let mut in_subtab = false;
    let mut groups: Vec<StepGroup> = Vec::new();
    let mut inside_subtab = Vec::new();
    let mut unnamed_no = 0usize;

    let finish = |open: &mut Option<Open>,
                  end: usize,
                  mixed_with_subtab: bool,
                  groups: &mut Vec<StepGroup>,
                  unnamed_no: &mut usize| {
        let Some(o) = open.take() else { return };
        let alias = o
            .markers
            .iter()
            .filter_map(|&mi| marker_alias(&panels[mi]))
            .find(|a| valid_id(a));
        let (var, named) = match alias {
            Some(a) => (a, true),
            None => {
                *unnamed_no += 1;
                let v = if *unnamed_no == 1 {
                    "step".to_string()
                } else {
                    format!("step_{unnamed_no}")
                };
                (v, false)
            }
        };
        groups.push(StepGroup {
            start: o.start,
            end,
            markers: o.markers,
            var,
            named,
            mixed_with_subtab,
        });
    };

    for (i, p) in panels.iter().enumerate() {
        if has_role(p, Role::Tab) {
            finish(&mut open, i, false, &mut groups, &mut unnamed_no);
            in_subtab = false;
            continue;
        }
        if has_role(p, Role::SubTab) {
            let was_open = open.is_some();
            finish(&mut open, i, was_open, &mut groups, &mut unnamed_no);
            in_subtab = true;
            continue;
        }
        if has_role(p, Role::GuideStep) {
            if in_subtab {
                inside_subtab.push(i);
                continue;
            }
            match &mut open {
                Some(g) => g.markers.push(i),
                None => {
                    open = Some(Open {
                        start: i,
                        markers: vec![i],
                    })
                }
            }
        }
    }
    finish(&mut open, panels.len(), false, &mut groups, &mut unnamed_no);

    Scan {
        groups,
        inside_subtab,
    }
}

/// The groups a [`scan`] found, without the `inside_subtab` list.
pub fn groups(panels: &[Panel]) -> Vec<StepGroup> {
    scan(panels).groups
}

/// Fill every statement's [`super::Panel::step`] from [`scan`]'s groups —
/// called from [`super::plan`], after [`super::views::assign_views`] and
/// before [`super::section_requires`] (which reads `step.marker` to gate a
/// step's statements, like a `::SUBTAB`'s).
pub fn assign_steps(panels: &mut [Panel]) {
    let Scan { groups, .. } = scan(panels);
    for g in &groups {
        let labels: Vec<Option<String>> = g
            .markers
            .iter()
            .map(|&mi| {
                marker_role_idx(&panels[mi]).and_then(|idx| marker_literal(&panels[mi], idx))
            })
            .collect();
        for (j, p) in panels.iter_mut().enumerate().take(g.end).skip(g.start) {
            let index = g.markers.iter().rposition(|&m| m <= j).unwrap_or(0);
            let marker = g.markers.contains(&j);
            p.step = Some(StepAt {
                group: g.start,
                index,
                marker,
                var: g.var.clone(),
                label: labels[index].clone(),
            });
        }
    }
}

/// A static (non-interactive) render's event for one statement, from
/// [`StaticSteps::step`]: `--svg`, the `dashboard` CLI's static HTML and the
/// script-free server page flatten every `::GUIDE_STEP` (decision 8 of
/// plan-516.md) — there is no stepper, every step is drawn in order with a
/// `::SECTION`-style heading, and gates are ignored.
#[derive(Debug, Clone, PartialEq)]
pub enum StaticEvent {
    /// Draw a "Step n of N · Label" heading, the same way as a `::SECTION`.
    Heading { n: u32, of: u32, label: String },
    /// Bind the step variable to `n` before this step's own panels run.
    SetVar { var: String, n: u32 },
}

/// The stepper the static renderers use: construct from the plan **after**
/// `-- @requires` hiding already dropped the statements it leaves out (so
/// every marker [`step`](Self::step) sees is a shown one — "of N" only
/// counts shown markers, decision 8), then call `step` once per surviving
/// statement, in order.
#[derive(Debug, Default)]
pub struct StaticSteps {
    totals: std::collections::HashMap<usize, u32>,
    seen: std::collections::HashMap<usize, u32>,
}

impl StaticSteps {
    /// `shown`: the plan's statements that survived `-- @requires` / `::VIEW`
    /// filtering, in order (setup included) — used only to total each
    /// group's markers; [`step`](Self::step) is still called per statement.
    pub fn new(shown: &[&Panel]) -> Self {
        let mut totals = std::collections::HashMap::new();
        for p in shown {
            if let Some(s) = &p.step {
                if s.marker {
                    *totals.entry(s.group).or_insert(0) += 1;
                }
            }
        }
        StaticSteps {
            totals,
            seen: std::collections::HashMap::new(),
        }
    }

    /// This (shown) statement's events, if it is a `::GUIDE_STEP` marker —
    /// empty otherwise. Markers should be fed in plan order.
    pub fn step(&mut self, p: &Panel) -> Vec<StaticEvent> {
        let Some(s) = &p.step else { return Vec::new() };
        if !s.marker {
            return Vec::new();
        }
        let n = self.seen.entry(s.group).or_insert(0);
        *n += 1;
        let of = *self.totals.get(&s.group).unwrap_or(&1);
        vec![
            StaticEvent::SetVar {
                var: s.var.clone(),
                n: *n,
            },
            StaticEvent::Heading {
                n: *n,
                of,
                label: s.label.clone().unwrap_or_default(),
            },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::plan;

    fn step_of(p: &[Panel], i: usize) -> &StepAt {
        p[i].step
            .as_ref()
            .unwrap_or_else(|| panic!("stmt {i} has no step"))
    }

    #[test]
    fn unnamed_group_var_is_step() {
        let p =
            plan("SELECT 'Intro'::GUIDE_STEP; SELECT 'x'::MARKDOWN; SELECT 'Wrap'::GUIDE_STEP;");
        assert_eq!(step_of(&p, 0).var, "step");
        assert_eq!(step_of(&p, 2).var, "step");
        assert!(step_of(&p, 0).marker);
        assert!(!step_of(&p, 1).marker);
        assert_eq!(step_of(&p, 0).index, 0);
        assert_eq!(step_of(&p, 1).index, 0);
        assert_eq!(step_of(&p, 2).index, 1);
    }

    #[test]
    fn named_group_var_is_the_alias() {
        let p = plan("SELECT 'Intro' AS lesson ::GUIDE_STEP; SELECT 'Wrap' ::GUIDE_STEP;");
        assert_eq!(step_of(&p, 0).var, "lesson");
        assert_eq!(step_of(&p, 1).var, "lesson");
        let g = groups(&p);
        assert!(g[0].named);
    }

    #[test]
    fn two_groups_in_two_tabs_are_step_and_step_2() {
        let p = plan(
            "SELECT 'a'::TAB; SELECT 'S1'::GUIDE_STEP; SELECT 'S2'::GUIDE_STEP;\n\
             SELECT 'b'::TAB; SELECT 'S3'::GUIDE_STEP; SELECT 'S4'::GUIDE_STEP;",
        );
        let g = groups(&p);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].var, "step");
        assert_eq!(g[1].var, "step_2");
    }

    #[test]
    fn group_closes_at_tab() {
        let p = plan(
            "SELECT 'S1'::GUIDE_STEP; SELECT 'x'::MARKDOWN; SELECT 'p'::TAB; SELECT 'y'::MARKDOWN;",
        );
        assert!(step_of(&p, 0).marker);
        assert_eq!(step_of(&p, 1).group, 0);
        assert!(p[2].step.is_none(), "the ::TAB itself is not a member");
        assert!(p[3].step.is_none(), "after the closer: no group");
    }

    #[test]
    fn group_closes_at_subtab_with_mixed_flag() {
        let p = plan(
            "SELECT 'S1'::GUIDE_STEP; SELECT 'x'::MARKDOWN; SELECT 'sub'::SUBTAB; SELECT 'y'::MARKDOWN;",
        );
        assert!(step_of(&p, 0).marker);
        assert!(p[2].step.is_none());
        assert!(p[3].step.is_none());
        let g = groups(&p);
        assert_eq!(g.len(), 1);
        assert!(g[0].mixed_with_subtab);
    }

    #[test]
    fn marker_inside_subtab_is_never_a_group() {
        let p = plan("SELECT 'sub'::SUBTAB; SELECT 'S1'::GUIDE_STEP; SELECT 'x'::MARKDOWN;");
        assert!(p[1].step.is_none());
        let s = scan(&p);
        assert!(s.groups.is_empty());
        assert_eq!(s.inside_subtab, vec![1]);
    }

    #[test]
    fn marker_closes_an_open_view() {
        // decision 3: a ::GUIDE_STEP marker closes an open ::VIEW group,
        // like a ::TAB does (checked via views::scan directly).
        let p = plan("SELECT 'Lines'::VIEW; SELECT 'S1'::GUIDE_STEP;");
        let g = super::super::views::groups(&p);
        assert_eq!(g.len(), 1);
        assert_eq!(
            g[0].end, 1,
            "the view group ends at the ::GUIDE_STEP marker"
        );
    }

    #[test]
    fn intro_before_the_first_marker_has_no_step() {
        let p =
            plan("SELECT 'intro text'::MARKDOWN; SELECT 'S1'::GUIDE_STEP; SELECT 'x'::MARKDOWN;");
        assert!(p[0].step.is_none());
        assert!(step_of(&p, 1).marker);
    }

    #[test]
    fn computed_label_is_none() {
        let p = plan("SELECT a || 'x' ::GUIDE_STEP FROM t;");
        assert_eq!(step_of(&p, 0).label, None);
    }

    #[test]
    fn literal_label_is_captured() {
        let p = plan("SELECT 'The three doors'::GUIDE_STEP;");
        assert_eq!(step_of(&p, 0).label.as_deref(), Some("The three doors"));
    }

    #[test]
    fn static_steps_count_only_shown_markers() {
        let p = plan("SELECT 'S1'::GUIDE_STEP; SELECT 'S2'::GUIDE_STEP; SELECT 'S3'::GUIDE_STEP;");
        // Simulate "S2" being hidden by `-- @requires`: only S1 and S3 are "shown".
        let shown: Vec<&Panel> = vec![&p[0], &p[2]];
        let mut st = StaticSteps::new(&shown);
        let e1 = st.step(&p[0]);
        assert_eq!(
            e1,
            vec![
                StaticEvent::SetVar {
                    var: "step".into(),
                    n: 1
                },
                StaticEvent::Heading {
                    n: 1,
                    of: 2,
                    label: "S1".into()
                },
            ]
        );
        let e3 = st.step(&p[2]);
        assert_eq!(
            e3,
            vec![
                StaticEvent::SetVar {
                    var: "step".into(),
                    n: 2
                },
                StaticEvent::Heading {
                    n: 2,
                    of: 2,
                    label: "S3".into()
                },
            ]
        );
        // Only shown markers are ever fed to `step` (the caller skips a
        // hidden one with `continue` before reaching it) — not exercised
        // here.
    }
}
