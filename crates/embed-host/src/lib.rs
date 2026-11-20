//! A host that uses only `anofox_visualization::embed` (#1266). If a test
//! here stops compiling, the embedding API changed: say so in the release
//! notes under "Embedding API" (see docs/embedding.md).

use anofox_visualization::embed::{
    check_opts, is_directive_panel, lookup, plan, render_dashboard_svg, render_spec_checked,
    resolve_json, DashboardOptions, DataProvider, Diagnostic, LintOptions, Locale, Panel,
    RenderLimits, ResolvedTheme, Role, RoleSpec, Severity, ThemeSet, Value, Variant,
};

/// Rows the host fetched itself: every query gets the same small series.
pub struct Prefetched;

impl DataProvider for Prefetched {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, _sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        Ok(vec![
            (
                "c0".into(),
                ["a", "b", "c"].map(|s| Value::Str(s.into())).to_vec(),
            ),
            ("c1".into(), [1.0, 3.0, 2.0].map(Value::Float).to_vec()),
        ])
    }
}

/// One chart from JSON rows and a role spec.
pub fn chart() -> Result<String, String> {
    let spec = serde_json::json!({
        "rows": [{"c0": "a", "c1": 1}, {"c0": "b", "c1": 3}],
        "roles": [[0, "XAXIS", "k"], [1, "BARCHART", "v"]],
        "width": 480,
        "height": 320,
        "locale": "de-DE",
    });
    render_spec_checked(&spec.to_string(), &RenderLimits::default())
}

/// A compiled dashboard, rows from the host.
pub fn dashboard(script: &str) -> Result<String, String> {
    render_dashboard_svg(script, &mut Prefetched, &DashboardOptions::default())
}

/// Author time: the statements, and the lint without a database.
pub fn author(script: &str) -> (Vec<Panel>, Vec<Diagnostic>) {
    let panels = plan(script);
    let diags = check_opts(script, |_q| Ok(Vec::new()), LintOptions::default());
    (panels, diags)
}

pub fn role(token: &str) -> Option<&'static RoleSpec> {
    lookup(token)
}

pub fn directive(roles: &[(usize, Role)]) -> bool {
    is_directive_panel(roles)
}

pub fn theme(def: &serde_json::Value, v: Variant) -> Result<ResolvedTheme, String> {
    let set: ThemeSet = resolve_json(def)?;
    Ok(set.pick(v).clone())
}

pub fn locales() -> Vec<Locale> {
    Locale::TAGS
        .iter()
        .filter_map(|t| Locale::from_tag(t))
        .collect()
}

pub fn errors(d: &[Diagnostic]) -> usize {
    d.iter().filter(|d| d.severity == Severity::Error).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "SELECT 'Sales'::DASHBOARD_TITLE;\nSELECT 12::SPAN;\nSELECT k::XAXIS, v::BARCHART FROM t;\n";

    #[test]
    fn the_embedding_surface_works_without_features() {
        assert!(chart().unwrap().starts_with("<svg"));
        assert!(dashboard(SCRIPT).unwrap().contains("<svg"));
        let (panels, diags) = author(SCRIPT);
        assert_eq!(panels.len(), 3);
        assert!(directive(&panels[1].roles));
        assert!(!directive(&panels[2].roles));
        assert_eq!(errors(&diags), 0, "{diags:?}");
        assert!(role("BARCHART").is_some() && role("NOPE").is_none());
        assert!(theme(&serde_json::json!({"extends": "dark"}), Variant::Dark).is_ok());
        assert_eq!(locales().len(), Locale::TAGS.len());
    }
}
