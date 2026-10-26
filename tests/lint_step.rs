//! The browser's lint (`lint_step`, issue #145) gives the same diagnostics as
//! `dashboard --check`: every `web/examples/*.sql` and the lint fixtures
//! (`tests/fixtures/lint/`) are linted twice through the `duckdb` CLI — once
//! by `lint::check_opts` with a synchronous runner, as the CLI does, and once
//! by the step protocol (`lint::step_json`, the function the wasm export
//! wraps) the way the browser drives it: run the needed queries, add the
//! answers, call again until done. Skips without a `duckdb` CLI (fails
//! without one on CI, see `common::skip_without`).
mod common;

use anofox_visualization::lint::{self, LintOptions};
use anofox_visualization::sql;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

type Rows = Vec<serde_json::Map<String, serde_json::Value>>;

/// `prelude + statement` in a fresh in-memory session from `web/` (the
/// examples read `data/…`), as `dashboard --check` runs it.
fn duckdb(sql: &str) -> Result<Rows, String> {
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("web");
    let out = Command::new("duckdb")
        .current_dir(web)
        .args([":memory:", "-json", "-c", sql])
        .output()
        .map_err(|e| format!("run duckdb: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    sql::parse_rows_json(&String::from_utf8_lossy(&out.stdout))
}

fn scripts() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    for dir in ["web/examples", "tests/fixtures/lint"] {
        for e in std::fs::read_dir(root.join(dir)).unwrap().flatten() {
            if e.path().extension().is_some_and(|x| x == "sql") {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out
}

#[test]
fn lint_step_matches_the_native_lint() {
    if Command::new("duckdb").arg("-version").output().is_err() {
        return common::skip_without("duckdb", "lint_step_matches_the_native_lint");
    }
    let mut seen_input_steps = false;
    for path in scripts() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let script = std::fs::read_to_string(&path).unwrap();
        // Every answer, so each query runs once (the queries are the same
        // with and without the design pass).
        let mut cache: HashMap<String, Result<Rows, String>> = HashMap::new();
        for (design, strict) in [(true, false), (true, true), (false, false)] {
            let native = lint::check_opts(
                &script,
                |q| {
                    cache
                        .entry(q.to_string())
                        .or_insert_with(|| duckdb(q))
                        .clone()
                },
                LintOptions {
                    design,
                    // `duckdb(q)` above is a fresh in-memory session per
                    // call — the same shape `lint_step`'s `scratchRunner`
                    // drives in the browser — so both sides must replay
                    // the same prelude text for the query strings (and so
                    // the `cache`) to line up.
                    stateful_session: false,
                    image_root: None,
                },
            );
            let want: Vec<serde_json::Value> = native.iter().map(|d| d.to_json()).collect();

            let opts = serde_json::json!({ "design": design, "strict": strict }).to_string();
            let mut known = serde_json::Map::new();
            let mut steps = 0;
            let got = loop {
                steps += 1;
                assert!(steps <= 20, "{name}: lint_step did not settle");
                let r: serde_json::Value = serde_json::from_str(&lint::step_json(
                    &script,
                    &serde_json::Value::Object(known.clone()).to_string(),
                    &opts,
                ))
                .unwrap();
                assert!(r.get("error").is_none(), "{name}: {r}");
                if r["done"].as_bool().unwrap() {
                    break r;
                }
                let need = r["need"].as_array().unwrap();
                assert!(!need.is_empty(), "{name}: not done but needs nothing");
                for q in need {
                    let q = q.as_str().unwrap();
                    assert!(!known.contains_key(q), "{name}: asked again for {q}");
                    let answer = cache.get(q).cloned().unwrap_or_else(|| duckdb(q));
                    let v = match answer {
                        Ok(rows) => {
                            serde_json::json!({ "rows": serde_json::to_string(&rows).unwrap() })
                        }
                        Err(e) => serde_json::json!({ "error": e }),
                    };
                    known.insert(q.to_string(), v);
                }
            };
            assert_eq!(
                got["diagnostics"].as_array().unwrap(),
                &want,
                "{name} (design {design}, strict {strict}): lint_step ≠ check_opts"
            );
            assert_eq!(
                got["ok"].as_bool().unwrap(),
                lint::passes(&native, strict),
                "{name}: ok"
            );
            assert_eq!(got["summary"], lint::summary(&native), "{name}: summary");
            // spans[n - 1] is statement n (UTF-16 offsets; these scripts are
            // ASCII outside a few literals, so compare on chars).
            let raws: Vec<String> = sql::split_statements(&sql::strip_line_comments(&script))
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let spans = got["spans"].as_array().unwrap();
            assert_eq!(spans.len(), sql::plan(&script).len(), "{name}: spans");
            let units: Vec<u16> = script.encode_utf16().collect();
            for (i, sp) in spans.iter().enumerate() {
                let (a, b) = (
                    sp[0].as_u64().unwrap() as usize,
                    sp[1].as_u64().unwrap() as usize,
                );
                let text = String::from_utf16(&units[a..b]).unwrap();
                assert_eq!(
                    sql::strip_line_comments(&text).trim(),
                    raws[i],
                    "{name}: span of statement {}",
                    i + 1
                );
            }
            if name == "inputs-chain.sql" {
                // Its second input and its panels read the first input's
                // default: one step to learn each default, one to finish.
                assert!(steps >= 3, "{name}: {steps} steps");
                seen_input_steps = true;
            }
        }
    }
    assert!(seen_input_steps, "the inputs-chain fixture was not linted");
}

#[test]
fn lint_step_rejects_bad_arguments() {
    let r = lint::step_json("SELECT 1 ::METRIC;", "[1]", "{}");
    assert!(r.contains("\"error\""), "{r}");
    let r = lint::step_json("SELECT 1 ::METRIC;", "{}", "nope");
    assert!(r.contains("\"error\""), "{r}");
    // An answer with neither rows nor an error is a DuckDB-style failure.
    let first: serde_json::Value =
        serde_json::from_str(&lint::step_json("SELECT 1 ::METRIC;", "", "")).unwrap();
    let q = first["need"][0].as_str().unwrap().to_string();
    let known = serde_json::json!({ q: {} }).to_string();
    let r: serde_json::Value =
        serde_json::from_str(&lint::step_json("SELECT 1 ::METRIC;", &known, "")).unwrap();
    assert_eq!(r["done"], true);
    assert_eq!(r["diagnostics"][0]["code"], "sql-error", "{r}");
    assert_eq!(r["ok"], false);
}
