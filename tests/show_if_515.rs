//! `::SHOW_IF` (#515): the headless SVG renderer skips a panel (or `::GROUP`
//! box) whose condition doesn't hold, exactly like a hidden `-- @requires`
//! section — see `src/dashboard.rs::execute`'s `show_if_holds`.

use anofox_visualization::dashboard::{render_dashboard_svg, DashboardOptions, DataProvider};
use ggplot_rs::prelude::Value;

/// A tiny provider: the `::SHOW_IF` condition's own query returns the
/// BOOLEAN literal written in its SQL text (`TRUE`/`FALSE`); every other
/// query returns a one-row bar-shaped (or heading-shaped) result so each
/// panel renders something recognisable.
struct Mock;

fn s(v: &[&str]) -> Vec<Value> {
    v.iter().map(|x| Value::Str((*x).into())).collect()
}
fn f(v: &[f64]) -> Vec<Value> {
    v.iter().map(|x| Value::Float(*x)).collect()
}

impl DataProvider for Mock {
    fn execute(&mut self, _sql: &str) -> Result<(), String> {
        Ok(())
    }
    fn query(&mut self, sql: &str) -> Result<Vec<(String, Vec<Value>)>, String> {
        if sql.contains("TRUE AS c0") {
            return Ok(vec![("c0".into(), vec![Value::Bool(true)])]);
        }
        if sql.contains("FALSE AS c0") {
            return Ok(vec![("c0".into(), vec![Value::Bool(false)])]);
        }
        if sql.contains("ERRORS AS c0") {
            return Err("Conversion Error: boom".into());
        }
        // a ::MARKDOWN / ::LABEL heading: one string literal, named by it
        // (checked before the generic numeric fallback below, since a short
        // quoted literal would otherwise also match that shape).
        if let Some(rest) = sql.trim_start().strip_prefix("SELECT '") {
            if let Some((text, _)) = rest.split_once('\'') {
                return Ok(vec![("c0".into(), s(&[text]))]);
            }
        }
        // a lone ::COL / ::HEIGHT / other directive's single number
        if sql.contains("AS c0") && !sql.contains("AS c1") && sql.len() < 40 {
            return Ok(vec![("c0".into(), f(&[2.0]))]);
        }
        Ok(vec![("c0".into(), s(&["?"]))])
    }
}

#[test]
fn true_target_shown_false_target_hidden() {
    let script = "\
        SELECT TRUE::SHOW_IF;\n\
        SELECT 'ShownText'::MARKDOWN;\n\
        SELECT FALSE::SHOW_IF;\n\
        SELECT 'HiddenText'::MARKDOWN;";
    let svg = render_dashboard_svg(script, &mut Mock, &DashboardOptions::default()).unwrap();
    assert!(svg.contains("ShownText"), "{svg}");
    assert!(!svg.contains("HiddenText"), "{svg}");
}

#[test]
fn a_false_condition_before_a_group_drops_every_tile_in_the_box() {
    let script = "\
        SELECT FALSE::SHOW_IF;\n\
        SELECT 1::GROUP;\n\
        SELECT 'InsideBox'::MARKDOWN;\n\
        SELECT 1::ENDGROUP;\n\
        SELECT 'AfterBox'::MARKDOWN;";
    let svg = render_dashboard_svg(script, &mut Mock, &DashboardOptions::default()).unwrap();
    assert!(!svg.contains("InsideBox"), "{svg}");
    assert!(svg.contains("AfterBox"), "{svg}");
}

/// A query error is one of `::SHOW_IF`'s own fail-closed cases (decision 2:
/// FALSE / NULL / no row / non-boolean / an error all hide the target) —
/// the whole render must still succeed, same as `false`, not bubble up as a
/// render-wide `Err` (see also `tests/serve_page_errors.rs`'s equivalent
/// check for the served script-free page).
#[test]
fn a_condition_query_error_hides_the_target_and_does_not_fail_the_render() {
    let script = "\
        SELECT ERRORS::SHOW_IF;\n\
        SELECT 'HiddenText'::MARKDOWN;\n\
        SELECT 'AfterText'::MARKDOWN;";
    let svg = render_dashboard_svg(script, &mut Mock, &DashboardOptions::default())
        .expect("a failing condition must not fail the whole render");
    assert!(!svg.contains("HiddenText"), "{svg}");
    assert!(svg.contains("AfterText"), "{svg}");
}

#[test]
fn a_col_before_a_hidden_target_does_not_widen_the_next_panel() {
    // The issue's own ordering: SELECT 12::COL; SELECT …::SHOW_IF; SELECT …
    // A ::COL in the hidden pair's range must not leak onto the panel after.
    let with_hidden_pair = "\
        SELECT 12::COL;\n\
        SELECT FALSE::SHOW_IF;\n\
        SELECT 'HiddenText'::MARKDOWN;\n\
        SELECT 'NextText'::MARKDOWN;";
    let without_hidden_pair = "SELECT 'NextText'::MARKDOWN;";
    let a =
        render_dashboard_svg(with_hidden_pair, &mut Mock, &DashboardOptions::default()).unwrap();
    let b =
        render_dashboard_svg(without_hidden_pair, &mut Mock, &DashboardOptions::default()).unwrap();
    assert!(!a.contains("HiddenText"), "{a}");
    assert_eq!(a, b, "the leftover ::COL must not widen NextText's panel");
}
