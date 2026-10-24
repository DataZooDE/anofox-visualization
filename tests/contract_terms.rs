//! `anofox_plot_terms` on a `mediation_by` output (#409, #445): the
//! producer's own `acme, ade, total, prop_mediated` order must survive
//! `anofox_plot_terms`'s aggregation (`list(tbl)`, `src/macros.rs` — #445
//! dropped the `ORDER BY tbl` that used to alphabetise by `term`, the
//! struct's first varying field), and `prop_mediated` — a ratio, not an
//! outcome-unit effect — gets its own small panel.

use anofox_visualization::sql::contract_call::render_call;

/// `mediation_by`'s rows, in the producer's own order (`acme, ade, total,
/// prop_mediated`) — what `anofox_plot_terms`'s aggregation macro now hands
/// the renderer unchanged (#445; it used to alphabetise by `term` instead,
/// `acme, ade, prop_mediated, total`, which `terms.rs` had its own
/// mediation-specific reorder to undo — #443, removed once this held
/// without it).
const MEDIATION_ROWS: &str = r#"[
    {"term": "acme", "estimate": 1.2, "conf_low": -0.4, "conf_high": 2.8, "conf_level": 0.95, "model_id": "mediation"},
    {"term": "ade", "estimate": -0.6, "conf_low": -2.6, "conf_high": 1.4, "conf_level": 0.95, "model_id": "mediation"},
    {"term": "total", "estimate": 0.6, "conf_low": -1.4, "conf_high": 2.6, "conf_level": 0.95, "model_id": "mediation"},
    {"term": "prop_mediated", "estimate": 0.22, "conf_low": -0.05, "conf_high": 0.49, "conf_level": 0.95, "model_id": "mediation"}
]"#;

fn render(rows: &str) -> String {
    render_call("anofox_plot_terms", rows, "{}", 640, 400, "").expect("renders")
}

/// Each term's y-position (SVG pixels): the `cy` of the `<circle>` right
/// before its tooltip `<title>term: …</title>`.
fn y_of(svg: &str, term: &str) -> f64 {
    let marker = format!("<title>{term}:");
    let at = svg
        .find(&marker)
        .unwrap_or_else(|| panic!("no mark for {term}: {svg}"));
    let before = &svg[..at];
    let circle_start = before.rfind("<circle ").expect("a circle before the title");
    let tag = &before[circle_start..];
    let k = tag.find(" cy=\"").expect("cy attr") + 5;
    tag[k..k + tag[k..].find('"').unwrap()]
        .parse()
        .expect("numeric cy")
}

#[test]
fn mediation_terms_keep_the_producers_order() {
    let svg = render(MEDIATION_ROWS);
    // acme above ade above total (smaller SVG y = higher on the chart) —
    // the producer's own order, not alphabetical (which would put
    // `prop_mediated` between `ade` and `total`).
    let (acme, ade, total) = (y_of(&svg, "acme"), y_of(&svg, "ade"), y_of(&svg, "total"));
    assert!(acme < ade, "acme should sit above ade: {acme} vs {ade}");
    assert!(ade < total, "ade should sit above total: {ade} vs {total}");
}

#[test]
fn mediation_prop_mediated_gets_its_own_panel() {
    let svg = render(MEDIATION_ROWS);
    // Two small panels, not one: the ratio's own axis, named for what it
    // is, and an "effect" panel for the three outcome-unit terms.
    assert!(svg.contains(">effect<"), "{svg}");
    assert!(svg.contains(">share mediated<"), "{svg}");
    assert!(
        svg.contains("proportion mediated"),
        "prop_mediated's own axis label missing: {svg}"
    );
    // The three effects keep their own CI-labelled estimate axis — the two
    // axis labels are genuinely different, not one panel's bleeding into
    // the other's.
    assert!(svg.contains("estimate (95% CI)"), "{svg}");
}

#[test]
fn non_mediation_terms_are_unaffected() {
    // A plain (non-mediation) terms table keeps today's single-panel
    // behaviour — no "effect"/"share mediated" split.
    let rows = r#"[
        {"term": "x1", "estimate": 0.5, "conf_low": 0.2, "conf_high": 0.8, "model_id": "ols"},
        {"term": "x2", "estimate": -0.3, "conf_low": -0.7, "conf_high": 0.1, "model_id": "ols"}
    ]"#;
    let svg = render(rows);
    assert!(!svg.contains(">effect<"), "{svg}");
    assert!(!svg.contains(">share mediated<"), "{svg}");
    assert!(svg.contains(">x1<") && svg.contains(">x2<"), "{svg}");
}
