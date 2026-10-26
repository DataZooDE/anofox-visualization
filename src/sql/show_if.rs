//! `::SHOW_IF`: show the next panel (or `::GROUP` box) only while a
//! BOOLEAN condition is `TRUE` (#515).
//!
//! [`assign`] fills every statement's [`super::Panel::show_if`] (called from
//! [`super::plan`], after [`views::assign_views`] and `section_requires`);
//! [`scan`] is its shared analysis, the same split as [`super::views::scan`]
//! / [`super::views::assign_views`], also used by the `show_if/*` lint pass
//! (`crate::lint`).
//!
//! A `::SHOW_IF` statement is a layout directive, like `::SPAN`/`::EMPTY`: a
//! statement on its own (one column; combined with other roles it is
//! ignored here — `crate::lint` reports `show_if/not-alone` separately, the
//! panel shows). Its **target** — what it governs — is the first statement
//! after it (and after any further `::SHOW_IF` stacked directly on top, AND
//! together) that is not setup, not another `::SHOW_IF`, and not a
//! next-panel modifier (`::SPAN`/`::COL`, `::HEIGHT`, `::EMPTY`,
//! `::ROWSELECT`, a `TableOpt`, a lone `::THRESHOLDS`, `::SELECT_AS`,
//! `::DRILL`). Those modifiers, wherever they sit directly before the
//! `::SHOW_IF` stack or between two stacked conditions, go with the target —
//! so a `::COL` before a hidden target never widens the next panel. A
//! `::GROUP` target's range runs to its matching `::ENDGROUP` (depth-
//! counted) — or, unclosed, to the next `::TAB`/`::SUBTAB`/`::VIEW` marker
//! or end of script, matching the browser's container reset. A chart
//! target's range also takes the `::TRACK` statements (and a `::HEIGHT`
//! right before each) that follow it directly.
//!
//! Some statements can never be a target — `::TAB`, `::SUBTAB`, `::VIEW`,
//! `::ENDVIEW`, `::ENDGROUP`, `::FIT`, `::COLUMNS`, chrome directives and
//! `::RELOAD` (`show_if/bad-target`, an error: the target always shows). No
//! statement following at all is `show_if/no-target` (a warning).

use super::Panel;
use crate::roles::is_chrome_directive;
use crate::Role;
use std::ops::Range;

/// One `::SHOW_IF` condition, as [`scan`] resolved it.
#[derive(Debug, Clone)]
pub struct Cond {
    /// Plan index of the `::SHOW_IF` statement itself.
    pub at: usize,
    /// What this condition governs — `None` when there is no target at all,
    /// or the next statement is a disallowed kind (`problem` says which).
    pub target: Option<usize>,
    /// The plan-index range (`[start, end)`) this condition's index is
    /// pushed to — empty when `target` is `None`.
    pub range: Range<usize>,
    /// `Some("show_if/bad-target")` or `Some("show_if/no-target")` when
    /// `target` is `None`; `None` for a resolved condition.
    pub problem: Option<&'static str>,
}

/// A statement whose only role is `::SHOW_IF` — a condition marker.
fn is_marker(roles: &[(usize, Role)]) -> bool {
    matches!(roles, [(_, Role::ShowIf)])
}

/// A next-panel modifier (decision 3): a statement whose roles are entirely
/// drawn from the modifier set, and carries at least one.
fn is_modifier(roles: &[(usize, Role)]) -> bool {
    if crate::roles::is_table_thresholds(roles) {
        return true;
    }
    !roles.is_empty()
        && roles.iter().all(|(_, r)| {
            matches!(
                r,
                Role::Span
                    | Role::Height
                    | Role::Empty
                    | Role::RowSelect
                    | Role::TableOpt(_)
                    | Role::SelectAs
                    | Role::Drill
            )
        })
}

/// Is this panel allowed to be a `::SHOW_IF` target (decision 3)?
fn allowed_target(p: &Panel) -> bool {
    p.contract.is_some()
        || p.roles.iter().any(|(_, r)| r.renders())
        || crate::inputs::input_kind(&p.roles).is_some()
        || p.roles.iter().any(|(_, r)| {
            matches!(
                r,
                Role::Section
                    | Role::Placeholder
                    | Role::HeaderImage
                    | Role::FooterLink
                    | Role::Download(_)
                    | Role::GroupStart
            )
        })
}

/// Is this panel a disallowed target (`show_if/bad-target`)?
fn bad_target(p: &Panel) -> bool {
    p.roles.iter().any(|(_, r)| {
        matches!(
            r,
            Role::Tab
                | Role::SubTab
                | Role::View
                | Role::ViewEnd
                | Role::GroupEnd
                | Role::Fit
                | Role::Columns
                | Role::Reload
                | Role::GuideStep
        ) || is_chrome_directive(*r)
    })
}

fn has_role(p: &Panel, want: Role) -> bool {
    p.roles.iter().any(|(_, r)| *r == want)
}

fn is_chart(p: &Panel) -> bool {
    p.roles.iter().any(|(_, r)| matches!(r, Role::Value(_)))
}

/// `panels[k]` is a lone `::HEIGHT` modifier, and `panels[k+1]` (if any) is a
/// `::TRACK` — the pair a chart target's range absorbs (decision 3).
fn height_before_track(panels: &[Panel], k: usize) -> bool {
    matches!(
        panels.get(k).map(|p| &p.roles[..]),
        Some([(_, Role::Height)])
    ) && panels.get(k + 1).is_some_and(|p| has_role(p, Role::Track))
}

/// The end of a `::GROUP` target's range: just past its matching
/// `::ENDGROUP` (depth-counted from `open + 1`), or — unclosed — the index
/// of the next `::TAB`/`::SUBTAB`/`::VIEW` marker, or `panels.len()`.
fn group_range_end(panels: &[Panel], open: usize) -> usize {
    let mut depth = 1i32;
    let mut k = open + 1;
    while k < panels.len() {
        if has_role(&panels[k], Role::GroupStart) {
            depth += 1;
        }
        if has_role(&panels[k], Role::GroupEnd) {
            depth -= 1;
            if depth == 0 {
                return k + 1;
            }
        }
        if has_role(&panels[k], Role::Tab)
            || has_role(&panels[k], Role::SubTab)
            || has_role(&panels[k], Role::View)
        {
            return k;
        }
        k += 1;
    }
    panels.len()
}

/// Scan a plan for `::SHOW_IF` conditions (the module docs' decision 3).
/// Used by [`assign`] and by the `show_if/*` lint pass (`crate::lint`).
pub fn scan(panels: &[Panel]) -> Vec<Cond> {
    let mut conds = Vec::new();
    let mut i = 0;
    while i < panels.len() {
        if !is_marker(&panels[i].roles) {
            i += 1;
            continue;
        }
        // Back up over contiguous modifiers directly before this (first)
        // marker of the run.
        let mut range_start = i;
        while range_start > 0
            && !panels[range_start - 1].setup
            && is_modifier(&panels[range_start - 1].roles)
        {
            range_start -= 1;
        }
        // Walk the stacked markers (modifiers and setup between them pass
        // through without ending the run).
        let mut markers = Vec::new();
        let mut j = i;
        loop {
            if is_marker(&panels[j].roles) {
                markers.push(j);
                j += 1;
            } else if panels[j].setup || is_modifier(&panels[j].roles) {
                j += 1;
            } else {
                break;
            }
            if j >= panels.len() {
                break;
            }
        }
        let (target, range, problem) = if j >= panels.len() {
            (None, range_start..range_start, Some("show_if/no-target"))
        } else if bad_target(&panels[j]) || !allowed_target(&panels[j]) {
            (None, range_start..range_start, Some("show_if/bad-target"))
        } else if has_role(&panels[j], Role::GroupStart) {
            let end = group_range_end(panels, j);
            (Some(j), range_start..end, None)
        } else {
            let mut end = j + 1;
            if is_chart(&panels[j]) {
                while end < panels.len() {
                    if has_role(&panels[end], Role::Track) {
                        end += 1;
                    } else if height_before_track(panels, end) {
                        end += 2;
                    } else {
                        break;
                    }
                }
            }
            (Some(j), range_start..end, None)
        };
        for &m in &markers {
            conds.push(Cond {
                at: m,
                target,
                range: range.clone(),
                problem,
            });
        }
        i = j + 1;
    }
    conds
}

/// Fill every statement's [`Panel::show_if`] from [`scan`]'s conditions —
/// called from [`super::plan`]. Every non-setup, non-marker statement in a
/// resolved condition's range gets that condition's plan index pushed;
/// stacked conditions AND together (every index in `show_if` must hold).
pub fn assign(panels: &mut [Panel]) {
    let conds = scan(panels);
    for c in &conds {
        if c.target.is_none() {
            continue;
        }
        for idx in c.range.clone() {
            if panels[idx].setup || is_marker(&panels[idx].roles) {
                continue;
            }
            panels[idx].show_if.push(c.at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::plan;

    #[test]
    fn plain_target() {
        let p = plan("SELECT true::SHOW_IF; SELECT 'hi'::MARKDOWN;");
        assert_eq!(p[1].show_if, vec![0]);
        assert!(p[0].show_if.is_empty());
    }

    #[test]
    fn modifier_before_and_after_the_condition() {
        // The issue's own ordering: a ::COL before the condition must not
        // leak to the panel after the hidden target.
        let p = plan(
            "SELECT 12::COL; SELECT true::SHOW_IF; SELECT 'hi'::MARKDOWN; SELECT 'next'::MARKDOWN;",
        );
        assert_eq!(p[0].show_if, vec![1]); // the ::COL
        assert_eq!(p[2].show_if, vec![1]); // the target
        assert!(p[3].show_if.is_empty()); // not leaked
    }

    #[test]
    fn stacked_conditions_and_together() {
        let p = plan("SELECT true::SHOW_IF; SELECT false::SHOW_IF; SELECT 'hi'::MARKDOWN;");
        assert_eq!(p[2].show_if, vec![0, 1]);
    }

    #[test]
    fn group_with_nesting() {
        let p = plan(
            "SELECT true::SHOW_IF;\nSELECT 1::GROUP;\nSELECT 1::GROUP;\nSELECT 1::ENDGROUP;\n\
             SELECT 1::ENDGROUP;\nSELECT 'after'::MARKDOWN;",
        );
        // indices: 0 cond, 1 group, 2 nested group, 3 inner endgroup, 4 outer endgroup, 5 after
        for (i, stmt) in p.iter().enumerate().take(5).skip(1) {
            assert_eq!(stmt.show_if, vec![0], "stmt {i}");
        }
        assert!(p[5].show_if.is_empty());
    }

    #[test]
    fn unclosed_group_runs_to_next_tab() {
        let p = plan(
            "SELECT true::SHOW_IF;\nSELECT 1::GROUP;\nSELECT 'x'::MARKDOWN;\nSELECT 'T'::TAB;",
        );
        assert_eq!(p[1].show_if, vec![0]);
        assert_eq!(p[2].show_if, vec![0]);
        assert!(p[3].show_if.is_empty(), "the ::TAB itself is not in range");
    }

    #[test]
    fn chart_plus_track() {
        let p = plan(
            "SELECT true::SHOW_IF;\nSELECT a::XAXIS, b::BARCHART FROM t;\n\
             SELECT 40::HEIGHT;\nSELECT c::TRACK FROM t;\nSELECT 'after'::MARKDOWN;",
        );
        assert_eq!(p[1].show_if, vec![0]);
        assert_eq!(
            p[2].show_if,
            vec![0],
            "the ::HEIGHT right before the ::TRACK"
        );
        assert_eq!(p[3].show_if, vec![0]);
        assert!(p[4].show_if.is_empty());
    }

    #[test]
    fn bad_target_always_shows() {
        let p = plan("SELECT true::SHOW_IF; SELECT 'p'::TAB;");
        assert!(p[1].show_if.is_empty());
        let s = scan(&p);
        assert_eq!(s[0].problem, Some("show_if/bad-target"));
        assert!(s[0].target.is_none());
    }

    #[test]
    fn no_target() {
        let p = plan("SELECT 'x'::MARKDOWN; SELECT true::SHOW_IF;");
        let s = scan(&p);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].at, 1);
        assert_eq!(s[0].problem, Some("show_if/no-target"));
    }

    #[test]
    fn setup_inside_a_range_always_runs() {
        let p = plan(
            "SELECT true::SHOW_IF;\nSELECT 1::GROUP;\nSET VARIABLE x = 1;\n\
             SELECT a::XAXIS, b::BARCHART FROM t;\nSELECT 1::ENDGROUP;",
        );
        assert!(p[2].setup);
        assert!(
            p[2].show_if.is_empty(),
            "a setup statement in the range always runs"
        );
        assert_eq!(p[3].show_if, vec![0]);
    }

    #[test]
    fn show_if_beside_other_roles_is_not_a_condition() {
        // Decision 1: invalid combination — ignored here (lint flags it).
        let p = plan("SELECT true::SHOW_IF, 1::COL; SELECT 'x'::MARKDOWN;");
        assert!(p[1].show_if.is_empty());
    }
}
