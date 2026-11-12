//! #1133: `dashboard --check` opens each dashboard the way `serve
//! --dashboards` does: its `-- @param` defaults are bound (and its `-- @load`
//! extensions loaded) before the panels run. Before this, a panel filtered by
//! a param read NULL and was reported empty (`empty-panel`) although the
//! server shows it filled.
use std::process::Command;

fn check(script: &str, tag: &str) -> String {
    let f = std::env::temp_dir().join(format!("anofox_1133_{tag}_{}.sql", std::process::id()));
    std::fs::write(&f, script).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .arg("--check")
        .arg(&f)
        .arg("--json")
        .output()
        .expect("run dashboard --check");
    let _ = std::fs::remove_file(&f);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_param_default_is_bound_before_the_panels_run() {
    let out = check(
        "-- @title Sessions\n-- @param region [EU, US] = US\n\
         SELECT channel::XAXIS, n::BARCHART, 'Sessions'::TITLE\n\
         FROM (VALUES ('US', 'app', 3), ('US', 'web', 5)) t(region, channel, n)\n\
         WHERE region = getvariable('region') ORDER BY n DESC;\n",
        "param",
    );
    assert!(out.contains("\"diagnostics\""), "{out}");
    assert!(!out.contains("empty-panel"), "{out}");
}

#[test]
fn without_a_param_the_same_panel_is_empty() {
    // the control: the same query with nothing binding `region`
    let out = check(
        "SELECT channel::XAXIS, n::BARCHART, 'Sessions'::TITLE\n\
         FROM (VALUES ('US', 'app', 3), ('US', 'web', 5)) t(region, channel, n)\n\
         WHERE region = getvariable('region') ORDER BY n DESC;\n",
        "none",
    );
    assert!(out.contains("empty-panel"), "{out}");
}
