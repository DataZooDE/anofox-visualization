//! #423: `--check` warns when `::CATEGORY`/`::FACET` are dropped on
//! `::HISTOGRAM`/`::QQ`-adjacent combinations it still doesn't support, and
//! stays quiet once they are supported. Mirrors `tests/lint_step.rs`'s own
//! `duckdb()` helper (a fresh `:memory:` session per query); skips without a
//! `duckdb` CLI on PATH (CI always has one).
use anofox_visualization::lint::{self, LintOptions};
use anofox_visualization::sql;
use std::collections::HashMap;
use std::process::Command;

type Rows = Vec<serde_json::Map<String, serde_json::Value>>;

fn duckdb(sql: &str) -> Result<Rows, String> {
    let out = Command::new("duckdb")
        .args([":memory:", "-json", "-c", sql])
        .output()
        .map_err(|e| format!("run duckdb: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    sql::parse_rows_json(&String::from_utf8_lossy(&out.stdout))
}

fn codes(script: &str) -> Vec<String> {
    if Command::new("duckdb").arg("-version").output().is_err() {
        return Vec::new();
    }
    let mut cache: HashMap<String, Result<Rows, String>> = HashMap::new();
    lint::check_opts(
        script,
        |q| {
            cache
                .entry(q.to_string())
                .or_insert_with(|| duckdb(q))
                .clone()
        },
        LintOptions {
            design: false,
            ..Default::default()
        },
    )
    .into_iter()
    .map(|d| d.code.to_string())
    .collect()
}

#[test]
fn histogram_category_no_longer_warns() {
    // #446: ::CATEGORY on ::HISTOGRAM is supported now (ggplot-rs 0.28).
    let script = "SELECT v ::HISTOGRAM, g ::CATEGORY FROM \
                  (VALUES (1.0, 'a'), (2.0, 'b')) t(v, g);";
    let cs = codes(script);
    if cs.is_empty() {
        return; // no duckdb CLI on PATH
    }
    assert!(
        !cs.contains(&"histogram/category-dropped".to_string()),
        "{cs:?}"
    );
}

#[test]
fn histogram_facet_no_longer_warns() {
    let script = "SELECT v ::HISTOGRAM, g ::FACET FROM \
                  (VALUES (1.0, 'a'), (2.0, 'b')) t(v, g);";
    let cs = codes(script);
    if cs.is_empty() {
        return;
    }
    assert!(
        !cs.contains(&"facet/unsupported-kind".to_string()),
        "{cs:?}"
    );
}

#[test]
fn qq_category_and_facet_no_longer_warn() {
    let script = "SELECT v ::QQ, g ::CATEGORY FROM \
                  (VALUES (1.0, 'a'), (2.0, 'b'), (3.0, 'a'), (4.0, 'b')) t(v, g);";
    let cs = codes(script);
    if !cs.is_empty() {
        assert!(
            !cs.contains(&"facet/unsupported-kind".to_string()),
            "{cs:?}"
        );
    }
    let script = "SELECT v ::QQ, g ::FACET FROM \
                  (VALUES (1.0, 'a'), (2.0, 'b'), (3.0, 'a'), (4.0, 'b')) t(v, g);";
    let cs = codes(script);
    if !cs.is_empty() {
        assert!(
            !cs.contains(&"facet/unsupported-kind".to_string()),
            "{cs:?}"
        );
    }
}

#[test]
fn facet_is_still_dropped_on_a_kind_that_does_not_support_it() {
    let script = "SELECT v ::PIE, g ::CATEGORY, g2 ::FACET FROM \
                  (VALUES (1.0, 'a', 'x'), (2.0, 'b', 'y')) t(v, g, g2);";
    let cs = codes(script);
    if cs.is_empty() {
        return;
    }
    assert!(cs.contains(&"facet/unsupported-kind".to_string()), "{cs:?}");
}

#[test]
fn hist_bins_and_hist_norm_warn_off_a_histogram() {
    let script = "SELECT v ::LINECHART, x ::XAXIS, b ::HIST_BINS, n ::HIST_NORM FROM \
                  (VALUES (1.0, 1.0, '5', 'share')) t(v, x, b, n);";
    let cs = codes(script);
    if cs.is_empty() {
        return;
    }
    assert!(cs.contains(&"histogram/wrong-kind".to_string()), "{cs:?}");
    assert_eq!(
        cs.iter().filter(|c| *c == "histogram/wrong-kind").count(),
        2,
        "one per role: {cs:?}"
    );
}

#[test]
fn hist_bins_and_hist_norm_are_quiet_on_a_histogram() {
    let script = "SELECT v ::HISTOGRAM, b ::HIST_BINS, n ::HIST_NORM FROM \
                  (VALUES (1.0, '5', 'share')) t(v, b, n);";
    let cs = codes(script);
    if !cs.is_empty() {
        assert!(!cs.contains(&"histogram/wrong-kind".to_string()), "{cs:?}");
    }
}
