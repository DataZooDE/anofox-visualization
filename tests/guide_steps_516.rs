//! `::GUIDE_STEP` (#516): the headless SVG renderer flattens the stepper —
//! each step draws in order as a "Step n of N · Label" heading, and the
//! step variable is bound (`SET VARIABLE`) before that step's own panels
//! run — see `src/dashboard.rs::execute`'s `step_n` / `StaticSteps`.

use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use ggplot_rs::prelude::Value;
use std::collections::HashMap;

/// A stateful provider: `SET VARIABLE x = n` updates a variable table that
/// `getvariable('x')` reads back from; a `-- @requires` check and `::GATE`
/// are driven by magic literals in the SQL text (`ERRORS`/`FALSE`/`TRUE`),
/// as `tests/show_if_515.rs`'s `Mock` does; a lone string literal panel is
/// named by its text; anything numeric falls back to a recognisable value.
#[derive(Default)]
struct Mock {
    vars: HashMap<String, Value>,
}

fn s(v: &[&str]) -> Vec<Value> {
    v.iter().map(|x| Value::Str((*x).into())).collect()
}

impl DataProvider for Mock {
    fn execute(&mut self, sql: &str) -> Result<(), String> {
        let t = sql.trim();
        if let Some(rest) = t
            .strip_prefix("SET VARIABLE ")
            .or_else(|| t.strip_prefix("SET VARIABLE"))
        {
            if let Some((name, val)) = rest.split_once('=') {
                let name = name.trim().to_string();
                let val = val.trim().trim_end_matches(';').trim();
                let v = if let Ok(n) = val.parse::<f64>() {
                    Value::Float(n)
                } else {
                    Value::Str(val.trim_matches('\'').to_string())
                };
                self.vars.insert(name, v);
            }
        }
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        if sql.contains("MISSING_FILE") {
            // `-- @requires` probe: pretend the file never exists.
            return Ok(vec![("n".into(), vec![Value::Str("0".into())])]);
        }
        if sql.contains("getvariable('step')") {
            let v = self
                .vars
                .get("step")
                .cloned()
                .unwrap_or(Value::Str("".into()));
            return Ok(vec![("c0".into(), vec![v])]);
        }
        // A lone string-literal panel (a ::GUIDE_STEP marker or a
        // ::MARKDOWN body) is named by its text.
        if let Some(rest) = sql.trim_start().strip_prefix("SELECT '") {
            if let Some((text, _)) = rest.split_once('\'') {
                return Ok(vec![("c0".into(), s(&[text]))]);
            }
        }
        Ok(vec![("c0".into(), s(&["?"]))])
    }
}

#[test]
fn three_steps_flatten_in_order_with_headings() {
    let script = "\
        SELECT 'Doors'::GUIDE_STEP;\n\
        SELECT 'DoorsBody'::MARKDOWN;\n\
        SELECT 'Sandbox'::GUIDE_STEP;\n\
        SELECT 'SandboxBody'::MARKDOWN;\n\
        SELECT 'Wrap'::GUIDE_STEP;\n\
        SELECT 'WrapBody'::MARKDOWN;";
    let svg =
        render_dashboard_svg(script, &mut Mock::default(), &DashboardOptions::default()).unwrap();
    for (n, label) in [(1, "Doors"), (2, "Sandbox"), (3, "Wrap")] {
        assert!(
            svg.contains(&format!("Step {n} of 3 · {label}")),
            "missing heading for step {n}: {svg}"
        );
    }
    let doors_pos = svg.find("Doors").unwrap();
    let sandbox_pos = svg.find("Sandbox").unwrap();
    let wrap_pos = svg.find("Wrap").unwrap();
    assert!(
        doors_pos < sandbox_pos && sandbox_pos < wrap_pos,
        "steps draw in order"
    );
}

#[test]
fn a_panel_reading_the_step_variable_sees_its_own_step_number() {
    let script = "\
        SELECT 'S1'::GUIDE_STEP;\n\
        SELECT getvariable('step')::MARKDOWN;\n\
        SELECT 'S2'::GUIDE_STEP;\n\
        SELECT getvariable('step')::MARKDOWN;";
    let svg =
        render_dashboard_svg(script, &mut Mock::default(), &DashboardOptions::default()).unwrap();
    assert!(svg.contains(">1<") || svg.contains("1.0"), "{svg}");
    assert!(svg.contains(">2<") || svg.contains("2.0"), "{svg}");
}

#[test]
fn a_named_group_binds_its_own_variable_name() {
    let script = "\
        SELECT 'S1' AS lesson ::GUIDE_STEP;\n\
        SELECT 'S2' ::GUIDE_STEP;";
    let mut mock = Mock::default();
    render_dashboard_svg(script, &mut mock, &DashboardOptions::default()).unwrap();
    assert_eq!(mock.vars.get("lesson"), Some(&Value::Float(2.0)));
}

#[test]
fn a_step_hidden_by_requires_is_left_out_of_the_count() {
    let script = "\
        SELECT 'S1'::GUIDE_STEP;\n\
        SELECT 'S1Body'::MARKDOWN;\n\
        -- @requires MISSING_FILE\n\
        SELECT 'S2'::GUIDE_STEP;\n\
        SELECT 'S2Body'::MARKDOWN;\n\
        SELECT 'S3'::GUIDE_STEP;\n\
        SELECT 'S3Body'::MARKDOWN;";
    let svg =
        render_dashboard_svg(script, &mut Mock::default(), &DashboardOptions::default()).unwrap();
    assert!(svg.contains("Step 1 of 2 · S1"), "{svg}");
    assert!(svg.contains("Step 2 of 2 · S3"), "{svg}");
    assert!(!svg.contains("S2Body"), "{svg}");
}
