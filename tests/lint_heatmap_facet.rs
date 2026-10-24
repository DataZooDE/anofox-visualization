//! #424: `--check` warns when `::IDENTITY`/`::REFLINE` are dropped on
//! `::HEATMAP` (reference/identity lines aren't drawn over tiles yet), and
//! stays quiet once `::FACET` is no longer dropped there. Mirrors
//! `tests/lint_step.rs`'s own `duckdb()` helper (a fresh `:memory:` session
//! per query); skips without a `duckdb` CLI on PATH (CI always has one).
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
        LintOptions { design: false },
    )
    .into_iter()
    .map(|d| d.code.to_string())
    .collect()
}

#[test]
fn heatmap_identity_and_refline_are_dropped_with_a_warning() {
    let script = "SELECT x ::XAXIS, y ::YAXIS, n ::HEATMAP, 1 ::IDENTITY, 5 ::REFLINE FROM \
                  (VALUES ('a', 'p', 1.0), ('b', 'q', 2.0)) t(x, y, n);";
    let cs = codes(script);
    if cs.is_empty() {
        return; // no duckdb CLI on PATH
    }
    assert_eq!(
        cs.iter().filter(|c| *c == "heatmap/line-dropped").count(),
        2,
        "one per role: {cs:?}"
    );
}

#[test]
fn heatmap_facet_no_longer_warns_and_the_confusion_matrix_repro_is_clean() {
    // The issue's own repro (#424).
    let script = "SELECT 'Confusion matrix'::TITLE, pred::XAXIS, actual::YAXIS, n::HEATMAP, \
                  p::FACET, ''::DATALABELS FROM \
                  (VALUES ('ref','0','0',50),('ref','0','1',5),('ref','1','0',7),('ref','1','1',40), \
                  ('cur','0','0',45),('cur','0','1',12),('cur','1','0',15),('cur','1','1',30)) \
                  t(p,pred,actual,n);";
    let cs = codes(script);
    if cs.is_empty() {
        return;
    }
    assert!(cs.is_empty(), "expected a clean check: {cs:?}");
}

#[test]
fn identity_and_refline_are_still_quiet_on_a_chart_kind_that_supports_them() {
    let script = "SELECT x ::XAXIS, y ::LINECHART, 1 ::IDENTITY, 5 ::REFLINE FROM \
                  (VALUES (1.0, 1.0), (2.0, 2.0)) t(x, y);";
    let cs = codes(script);
    if !cs.is_empty() {
        assert!(!cs.contains(&"heatmap/line-dropped".to_string()), "{cs:?}");
    }
}

// #471: two ::REFLINE columns (an alert band's lower and upper edge) on the
// same statement is a legitimate, fully-drawn shape now — `--check --strict`
// stays quiet on the issue's own repro.
#[test]
fn two_reflines_with_a_shared_label_check_clean() {
    let script = "SELECT i::XAXIS, (800 + 10 * i)::LINECHART, 930::REFLINE, 760::REFLINE, \
                  'Alert band'::REFLINE_LABEL FROM range(10) t(i);";
    let cs = codes(script);
    assert!(cs.is_empty(), "expected a clean check: {cs:?}");
}
