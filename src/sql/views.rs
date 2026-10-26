//! `::VIEW` / `::ENDVIEW`: alternative views of one panel (#214).
//!
//! A run of consecutive `::VIEW` markers with no closer between them is one
//! *group*; each marker is one *alternative*. A group closes on `::ENDVIEW`,
//! `::TAB`, `::SUBTAB`, `::SECTION`, an `::ENDGROUP` that closes a `::GROUP`
//! opened before the group, or end of script. A `::VIEW` right after a
//! closed group starts a new one — views don't nest, so there is no way to
//! get a second group without a closer in between.
//!
//! [`assign_views`] fills every statement's [`super::Panel::view`] (called
//! from [`super::plan`]); [`scan`] is its (and the `view/*` lint pass's)
//! shared analysis. [`StaticPick`] is the stepper the non-interactive
//! renderers (`--svg`, the static HTML page, the script-free server page)
//! use to keep only the first shown alternative of each group.

use super::{lex, Panel};
use crate::Role;

/// One statement's place in a `::VIEW` group — set on every statement of the
/// group, setup included (see [`assign_views`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ViewAt {
    /// The plan index of the group's first `::VIEW` marker.
    pub group: usize,
    /// 0-based alternative index within the group.
    pub alt: usize,
    /// `true` only for the `::VIEW` statement itself, not the other
    /// statements of its alternative.
    pub marker: bool,
    /// The group id: the alias of its first marker that has a valid one
    /// (`^[A-Za-z_][A-Za-z0-9_]{0,63}$`, see [`valid_id`]), else the group's
    /// 1-based position in the script (`"1"`, `"2"`, …).
    pub id: String,
    /// This alternative's label — the literal text of its marker's value
    /// (a plain `'…'`/`E'…'` string, or `NULL` → `Some(String::new())`).
    /// `None` for a computed label (not a lint error by itself —
    /// `view/computed-label` is a warning).
    pub label: Option<String>,
}

/// One `::VIEW` group, as [`scan`] resolved it — the structural facts a
/// per-statement [`ViewAt`] alone can't answer, for the `view/*` lint pass.
#[derive(Debug, Clone)]
pub struct Group {
    /// Plan index of the first marker (== [`ViewAt::group`] of every
    /// statement in it).
    pub start: usize,
    /// Plan index just past the group (exclusive): its closer's index, or
    /// the plan length when end of script closed it.
    pub end: usize,
    /// Plan indices of its `::VIEW` markers, in order (one per alternative).
    pub markers: Vec<usize>,
    pub id: String,
    /// `id` came from a marker's alias; `false` means the positional
    /// fallback was used (`view/unnamed`).
    pub named: bool,
    /// A `::GROUP` opened inside the group and not matched by an
    /// `::ENDGROUP` before an *explicit* closer (`::ENDVIEW`/`::TAB`/
    /// `::SUBTAB`/`::SECTION`/end of script) — `view/unbalanced-group`. An
    /// outer `::ENDGROUP` that closes this group along with its own
    /// enclosing box is the expected way to close a view inside a box, not
    /// this.
    pub unbalanced_group: bool,
    /// Closed by an explicit `::ENDVIEW` (as opposed to `::TAB`/`::SUBTAB`/
    /// `::SECTION`/an outer `::ENDGROUP`/end of script) — see
    /// `view/open-ended`.
    pub closed_by_endview: bool,
}

/// [`scan`]'s result.
#[derive(Debug, Clone, Default)]
pub struct Scan {
    pub groups: Vec<Group>,
    /// Plan indices of `::ENDVIEW` statements with no open group to close
    /// (`view/stray-end`).
    pub stray_ends: Vec<usize>,
}

/// Is `s` a valid group id (`^[A-Za-z_][A-Za-z0-9_]{0,63}$`)?
pub fn valid_id(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    let rest: Vec<char> = chars.collect();
    rest.len() <= 63 && rest.iter().all(|c| c.is_ascii_alphanumeric() || *c == '_')
}

/// The output position of a statement's own cast to `want` (`Role::View` or
/// `Role::ViewEnd`), if it carries one.
fn marker_role_idx(p: &Panel, want: Role) -> Option<usize> {
    p.roles.iter().find(|(_, r)| *r == want).map(|(i, _)| *i)
}

/// A `::VIEW` marker's own alias (`'Lines' AS yoy ::VIEW` → `"yoy"`), as
/// [`super::rewrite`] preserves it into [`Panel::names`]. `None` for an
/// unaliased marker — not necessarily invalid; see [`valid_id`].
pub fn marker_alias(p: &Panel) -> Option<String> {
    let idx = marker_role_idx(p, Role::View)?;
    p.names
        .iter()
        .find(|(j, _)| *j == idx)
        .map(|(_, n)| n.clone())
}

/// A select item's literal text: exactly one `'…'`/`E'…'` string token
/// (`''` unescaped), or the bare word `NULL` (→ `Some(String::new())`, the
/// same as an empty string literal — both are `view/empty-label`). Anything
/// else (a column, a function call, any other expression) is "computed" —
/// `None`.
pub(super) fn literal_text(core: &str) -> Option<String> {
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
        [t] if t.kind == lex::TokKind::Word && t.text(core).eq_ignore_ascii_case("NULL") => {
            Some(String::new())
        }
        _ => None,
    }
}

/// A `::VIEW` marker's own label: its select item's literal text (see
/// [`literal_text`]), read back from the rewritten `p.sql` at the role's
/// output position — `rewrite()`'s `Role::View` branch keeps the value's
/// text verbatim before ` AS c{idx}`, so this is exact, not a guess.
fn marker_literal(p: &Panel, idx: usize) -> Option<String> {
    let (start, end) = super::main_select_list(&p.sql)?;
    let items = lex::split_top_commas(&p.sql[start..end]);
    let item = items.get(idx)?.trim();
    let core = item.strip_suffix(&format!(" AS c{idx}"))?.trim();
    literal_text(core)
}

/// Scan a plan for `::VIEW` groups and stray `::ENDVIEW`s (the closer rules
/// — see the module docs). Used by [`assign_views`] and by the `view/*`
/// lint pass (`crate::lint`).
pub fn scan(panels: &[Panel]) -> Scan {
    struct Open {
        start: usize,
        depth_at_open: i32,
        markers: Vec<usize>,
        group_no: usize,
    }
    struct Closed {
        open: Open,
        end: usize,
        closed_by_endview: bool,
        unbalanced_group: bool,
    }

    let mut open: Option<Open> = None;
    let mut depth = 0i32;
    let mut group_no = 0usize;
    let mut closed: Vec<Closed> = Vec::new();
    let mut stray_ends = Vec::new();

    fn finish(
        open: &mut Option<Open>,
        depth: i32,
        end: usize,
        closed_by_endview: bool,
        closed: &mut Vec<Closed>,
    ) {
        if let Some(g) = open.take() {
            let unbalanced_group = depth > g.depth_at_open;
            closed.push(Closed {
                open: g,
                end,
                closed_by_endview,
                unbalanced_group,
            });
        }
    }

    for (i, p) in panels.iter().enumerate() {
        let has = |r: Role| p.roles.iter().any(|(_, x)| *x == r);
        if has(Role::Tab) || has(Role::SubTab) || has(Role::Section) || has(Role::GuideStep) {
            finish(&mut open, depth, i, false, &mut closed);
        }
        if has(Role::GroupEnd) {
            let new_depth = depth - 1;
            if let Some(g) = &open {
                if new_depth < g.depth_at_open {
                    finish(&mut open, depth, i, false, &mut closed);
                }
            }
            depth = new_depth;
        }
        if has(Role::View) {
            match &mut open {
                Some(g) => g.markers.push(i),
                None => {
                    group_no += 1;
                    open = Some(Open {
                        start: i,
                        depth_at_open: depth,
                        markers: vec![i],
                        group_no,
                    });
                }
            }
        } else if has(Role::ViewEnd) {
            if open.is_some() {
                finish(&mut open, depth, i, true, &mut closed);
            } else {
                stray_ends.push(i);
            }
        }
        if has(Role::GroupStart) {
            depth += 1;
        }
    }
    finish(&mut open, depth, panels.len(), false, &mut closed);

    let groups = closed
        .into_iter()
        .map(|c| {
            let alias = c
                .open
                .markers
                .iter()
                .filter_map(|&mi| marker_alias(&panels[mi]))
                .find(|a| valid_id(a));
            let (id, named) = match alias {
                Some(a) => (a, true),
                None => (c.open.group_no.to_string(), false),
            };
            Group {
                start: c.open.start,
                end: c.end,
                markers: c.open.markers,
                id,
                named,
                unbalanced_group: c.unbalanced_group,
                closed_by_endview: c.closed_by_endview,
            }
        })
        .collect();

    Scan { groups, stray_ends }
}

/// The groups a [`scan`] found, without the stray-`::ENDVIEW` list.
pub fn groups(panels: &[Panel]) -> Vec<Group> {
    scan(panels).groups
}

/// Fill every statement's [`Panel::view`] from [`scan`]'s groups — called
/// from [`super::plan`], before [`super::section_requires`] (which reads
/// `view.marker` to gate a `::VIEW` alternative the same way a `::SUBTAB`
/// is gated).
pub fn assign_views(panels: &mut [Panel]) {
    let Scan { groups, .. } = scan(panels);
    for g in &groups {
        let alt_labels: Vec<Option<String>> = g
            .markers
            .iter()
            .map(|&mi| {
                marker_role_idx(&panels[mi], Role::View)
                    .and_then(|idx| marker_literal(&panels[mi], idx))
            })
            .collect();
        for (j, p) in panels.iter_mut().enumerate().take(g.end).skip(g.start) {
            let alt = g.markers.iter().rposition(|&m| m <= j).unwrap_or(0);
            let marker = g.markers.contains(&j);
            p.view = Some(ViewAt {
                group: g.start,
                alt,
                marker,
                id: g.id.clone(),
                label: alt_labels[alt].clone(),
            });
        }
    }
}

/// Keeps the first SHOWN alternative of each `::VIEW` group for a
/// non-interactive (static) render: `--svg`, the `dashboard` CLI's static
/// HTML, and the script-free server page. It drops the non-setup statements
/// of the other alternatives; setup statements run everywhere, the same way
/// a hidden tab's setup still runs.
///
/// "Shown" is whatever reaches [`Self::drop_panel`] — pair this with
/// `-- @requires` filtering (e.g. [`super::requires::Sections`]) upstream: a
/// requires-hidden alternative's marker is `continue`d before it ever
/// reaches `drop_panel`, so the first one that *does* reach it is the first
/// shown one, which becomes the default.
#[derive(Debug, Default)]
pub struct StaticPick {
    group: Option<usize>,
    default_alt: Option<usize>,
}

impl StaticPick {
    /// Should `p` be left out of a static render?
    pub fn drop_panel(&mut self, p: &Panel) -> bool {
        let Some(v) = p.view.as_ref() else {
            return false;
        };
        if self.group != Some(v.group) {
            self.group = Some(v.group);
            self.default_alt = Some(v.alt);
        }
        !p.setup && Some(v.alt) != self.default_alt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql::plan;

    fn view_of(p: &[Panel], i: usize) -> &ViewAt {
        p[i].view
            .as_ref()
            .unwrap_or_else(|| panic!("stmt {i} has no view"))
    }

    #[test]
    fn one_group_from_consecutive_markers() {
        let p = plan("SELECT 'Lines'::VIEW; SELECT 'Bars'::VIEW; SELECT 'Dots'::VIEW;");
        assert_eq!(p.len(), 3);
        for (i, v) in p.iter().enumerate() {
            let v = v.view.as_ref().unwrap_or_else(|| panic!("stmt {i}"));
            assert_eq!(v.group, 0);
            assert_eq!(v.alt, i);
            assert!(v.marker);
            assert_eq!(v.id, "1"); // the script's only (1st) group, unnamed
        }
    }

    #[test]
    fn group_id_is_positional_without_an_alias() {
        let p = plan("SELECT 'Lines'::VIEW; SELECT 'Bars'::VIEW;");
        assert_eq!(view_of(&p, 0).id, "1");
        assert_eq!(view_of(&p, 1).id, "1");
    }

    #[test]
    fn alias_becomes_the_group_id() {
        let p = plan("SELECT 'Lines' AS yoy ::VIEW; SELECT 'Bars' AS yoy2 ::VIEW;");
        assert_eq!(view_of(&p, 0).id, "yoy");
        assert_eq!(view_of(&p, 1).id, "yoy");
        assert_eq!(view_of(&p, 0).label.as_deref(), Some("Lines"));
        assert_eq!(view_of(&p, 1).label.as_deref(), Some("Bars"));
    }

    #[test]
    fn first_valid_alias_among_several_wins() {
        // alt 0 has no alias; alt 1's is used as the group id.
        let p = plan("SELECT 'Lines'::VIEW; SELECT 'Bars' AS yoy ::VIEW;");
        assert_eq!(view_of(&p, 0).id, "yoy");
        assert_eq!(view_of(&p, 1).id, "yoy");
    }

    #[test]
    fn setup_statements_in_a_view_carry_its_view_at() {
        let p = plan(
            "SELECT 'Lines' AS yoy ::VIEW;\nSET VARIABLE x = 1;\nSELECT a::XAXIS, b::LINECHART FROM t;",
        );
        assert_eq!(p.len(), 3);
        assert!(p[1].setup);
        let v1 = view_of(&p, 1);
        assert_eq!(v1.group, 0);
        assert_eq!(v1.alt, 0);
        assert!(!v1.marker);
        assert_eq!(v1.id, "yoy");
        let v2 = view_of(&p, 2);
        assert_eq!(v2.alt, 0);
        assert!(!v2.marker);
    }

    #[test]
    fn closer_endview() {
        let p =
            plan("SELECT 'Lines'::VIEW; SELECT 1::ENDVIEW; SELECT a::XAXIS, b::BARCHART FROM t;");
        assert!(view_of(&p, 0).marker);
        assert!(
            p[1].view.is_none(),
            "the ::ENDVIEW itself closes, not a member"
        );
        assert!(p[2].view.is_none(), "after the closer: no group");
    }

    #[test]
    fn closer_tab() {
        let p = plan("SELECT 'Lines'::VIEW; SELECT 'x'::TAB;");
        assert!(view_of(&p, 0).marker);
        assert!(p[1].view.is_none());
    }

    #[test]
    fn closer_subtab() {
        let p = plan("SELECT 'p'::TAB; SELECT 'Lines'::VIEW; SELECT 'x'::SUBTAB;");
        assert!(view_of(&p, 1).marker);
        assert!(p[2].view.is_none());
    }

    #[test]
    fn closer_section() {
        let p = plan("SELECT 'Lines'::VIEW; SELECT 'Heading'::SECTION;");
        assert!(view_of(&p, 0).marker);
        assert!(p[1].view.is_none());
    }

    #[test]
    fn closer_outer_endgroup() {
        // The ::GROUP opened before the ::VIEW; its ::ENDGROUP closes both.
        let p = plan("SELECT 1::GROUP; SELECT 'Lines'::VIEW; SELECT 1::ENDGROUP; SELECT a::XAXIS, b::BARCHART FROM t;");
        assert!(view_of(&p, 1).marker);
        assert!(
            p[2].view.is_none(),
            "the outer ::ENDGROUP itself is not a member"
        );
        assert!(p[3].view.is_none(), "after the closer: no group");
        let g = groups(&p);
        assert_eq!(g.len(), 1);
        assert!(!g[0].unbalanced_group, "the box was opened BEFORE the view");
    }

    #[test]
    fn closer_end_of_script() {
        let p = plan("SELECT 'Lines'::VIEW; SELECT a::XAXIS, b::BARCHART FROM t;");
        assert!(view_of(&p, 0).marker);
        assert_eq!(view_of(&p, 1).alt, 0);
        let g = groups(&p);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].end, p.len());
        assert!(!g[0].closed_by_endview);
    }

    #[test]
    fn two_groups_separated_by_endview() {
        let p = plan(
            "SELECT 'Lines'::VIEW; SELECT 1::ENDVIEW; SELECT 'Bars'::VIEW; SELECT 'Dots'::VIEW;",
        );
        assert_eq!(view_of(&p, 0).group, 0);
        assert!(p[1].view.is_none());
        assert_eq!(view_of(&p, 2).group, 2);
        assert_eq!(view_of(&p, 2).alt, 0);
        assert_eq!(view_of(&p, 3).group, 2);
        assert_eq!(view_of(&p, 3).alt, 1);
        assert_ne!(view_of(&p, 0).id, view_of(&p, 2).id);
        let g = groups(&p);
        assert_eq!(g.len(), 2);
        assert!(g[0].closed_by_endview);
        assert_eq!(g[0].id, "1");
        assert_eq!(g[1].id, "2");
    }

    #[test]
    fn mixed_aliases_are_all_kept_as_each_markers_own() {
        let p = plan("SELECT 'Lines' AS yoy ::VIEW; SELECT 'Bars' AS qoq ::VIEW;");
        // The group id is the FIRST valid alias; mixing is a lint concern
        // (view/mixed-names), not something assign_views refuses.
        assert_eq!(view_of(&p, 0).id, "yoy");
        assert_eq!(view_of(&p, 1).id, "yoy");
        assert_eq!(marker_alias(&p[0]).as_deref(), Some("yoy"));
        assert_eq!(marker_alias(&p[1]).as_deref(), Some("qoq"));
    }

    #[test]
    fn a_bad_alias_falls_back_to_the_positional_id() {
        // "2x" is not a valid identifier (starts with a digit).
        let p = plan("SELECT 'Lines' AS \"2x\" ::VIEW;");
        assert_eq!(view_of(&p, 0).id, "1");
        assert_eq!(marker_alias(&p[0]).as_deref(), Some("2x"));
        assert!(!valid_id("2x"));
    }

    #[test]
    fn literal_label_extraction() {
        let p = plan(
            "SELECT 'Lines'::VIEW; SELECT ''::VIEW; SELECT NULL::VIEW; SELECT a||'x'::VIEW FROM t;",
        );
        assert_eq!(view_of(&p, 0).label.as_deref(), Some("Lines"));
        assert_eq!(view_of(&p, 1).label.as_deref(), Some(""));
        assert_eq!(view_of(&p, 2).label.as_deref(), Some(""));
        assert_eq!(
            view_of(&p, 3).label,
            None,
            "a computed expression is not a literal"
        );
    }

    #[test]
    fn a_lone_view_right_after_a_closed_group_opens_a_new_one() {
        let p = plan("SELECT 'Lines'::VIEW; SELECT 1::ENDVIEW; SELECT 'Bars'::VIEW;");
        let g = groups(&p);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].markers, vec![0]);
        assert_eq!(g[1].markers, vec![2]);
    }

    #[test]
    fn views_do_not_nest_without_an_explicit_closer() {
        // Two ::VIEW markers with nothing between them: one group, two alts
        // — never two separate groups.
        let p = plan("SELECT 'Lines'::VIEW; SELECT 'Bars'::VIEW;");
        let g = groups(&p);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].markers, vec![0, 1]);
    }

    #[test]
    fn stray_endview_is_reported() {
        let p = plan("SELECT a::XAXIS, b::BARCHART FROM t; SELECT 1::ENDVIEW;");
        let s = scan(&p);
        assert_eq!(s.stray_ends, vec![1]);
        assert!(s.groups.is_empty());
    }

    #[test]
    fn unbalanced_group_inside_a_view() {
        // A ::GROUP opened INSIDE the view, never closed before the view's
        // own (explicit) closer.
        let p = plan("SELECT 'Lines'::VIEW; SELECT 1::GROUP; SELECT 1::ENDVIEW;");
        let g = groups(&p);
        assert_eq!(g.len(), 1);
        assert!(g[0].unbalanced_group);
    }

    #[test]
    fn group_not_named_without_any_valid_alias() {
        let p = plan("SELECT 'Lines'::VIEW; SELECT 'Bars'::VIEW;");
        let g = groups(&p);
        assert!(!g[0].named);
    }

    #[test]
    fn static_pick_keeps_only_the_first_alternative() {
        let p = plan(
            "SELECT 'Lines'::VIEW;\nSET VARIABLE x = 1;\nSELECT a::XAXIS, b::LINECHART FROM t;\n\
             SELECT 'Bars'::VIEW;\nSELECT a::XAXIS, b::BARCHART FROM t;",
        );
        let mut pick = StaticPick::default();
        let dropped: Vec<bool> = p.iter().map(|x| pick.drop_panel(x)).collect();
        assert_eq!(
            dropped,
            vec![false, false, false, true, true],
            "alt 0's marker+panel (and its setup) stay; alt 1's marker+panel drop"
        );
    }

    #[test]
    fn static_pick_always_keeps_setup_even_in_a_dropped_alternative() {
        let p = plan(
            "SELECT 'Lines'::VIEW; SELECT a::XAXIS, b::LINECHART FROM t;\n\
             SELECT 'Bars'::VIEW;\nSET VARIABLE x = 1;\nSELECT a::XAXIS, b::BARCHART FROM t;",
        );
        let mut pick = StaticPick::default();
        let dropped: Vec<bool> = p.iter().map(|x| pick.drop_panel(x)).collect();
        assert_eq!(dropped, vec![false, false, true, false, true]);
        // index 2: alt 1's marker drops; index 3: its SET VARIABLE (setup)
        // still runs; index 4: alt 1's panel drops.
    }

    #[test]
    fn static_pick_resets_across_groups() {
        let p = plan(
            "SELECT 'Lines'::VIEW; SELECT 1::ENDVIEW;\n\
             SELECT 'Bars'::VIEW; SELECT 'Dots'::VIEW;",
        );
        let mut pick = StaticPick::default();
        let dropped: Vec<bool> = p.iter().map(|x| pick.drop_panel(x)).collect();
        // group 1's single alt (index 0) is kept; group 2's first alt kept,
        // second dropped.
        assert_eq!(dropped, vec![false, false, false, true]);
    }
}
