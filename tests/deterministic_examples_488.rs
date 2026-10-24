//! #488: six `web/examples/*.sql` dashboards rendered different SVG bytes
//! from run to run with the same binary — `arg_min`/`ORDER BY ... DESC` ties
//! resolved arbitrarily (parallel scan order), a flow map's end-point dots
//! drawn in `HashMap` iteration order, a quantity-weighted mean built from
//! `sum(...)/sum(...)` picking up a different last bit or two each time a
//! parallel hash aggregate combined its partial sums in a different order,
//! and (tabular-feed, found by a separate pass over #488) a window-function
//! total tied across categories with no tie-break beyond a non-unique column.
//!
//! This renders each listed example twice, with `SET threads=8;` forced (so
//! the duckdb CLI subprocess parallelizes even on a small CI runner — the
//! same condition that exposed the bug), and asserts byte-identical SVG.
//! Needs the `duckdb` CLI (skips without it locally, fails on CI — see
//! `common::skip_without`).
mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

fn have_duckdb(test: &str) -> bool {
    let ok = Command::new("duckdb").arg("-version").output().is_ok();
    if !ok {
        common::skip_without("duckdb", test);
    }
    ok
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        let d = std::env::temp_dir().join(format!("anofox_488_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Scratch(d)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const EXAMPLES: &[&str] = &[
    "decide-claims",
    "decide-showcase",
    "delivery-coverage",
    "optimize-delivery",
    "supplier-lead-times",
    "tabular-feed",
];

/// Renders `web/examples/<name>.sql` to SVG, with `SET threads=8;` forced at
/// the top (cwd is `web/`, as the examples' `data/...` paths are relative to
/// it, so the forced-threads copy lives elsewhere and is passed by its
/// absolute path).
fn render(scratch: &Path, web: &Path, name: &str, tag: &str) -> String {
    let script = std::fs::read_to_string(web.join("examples").join(format!("{name}.sql")))
        .unwrap_or_else(|e| panic!("read {name}.sql: {e}"));
    let forced = format!("SET threads=8;\n{script}");
    let sql_path = scratch.join(format!("{name}-{tag}.sql"));
    std::fs::write(&sql_path, forced).unwrap();
    let out_path = scratch.join(format!("{name}-{tag}.svg"));
    let run = Command::new(env!("CARGO_BIN_EXE_dashboard"))
        .current_dir(web)
        .arg("--svg")
        .arg(&sql_path)
        .arg(&out_path)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{name}: stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    std::fs::read_to_string(&out_path).unwrap_or_else(|e| panic!("read {name}'s svg: {e}"))
}

#[test]
fn examples_render_byte_identical_across_runs() {
    if !have_duckdb("examples_render_byte_identical_across_runs") {
        return;
    }
    let web = Path::new(env!("CARGO_MANIFEST_DIR")).join("web");
    let scratch = Scratch::new("deterministic");
    let mut failures = Vec::new();
    for name in EXAMPLES {
        let a = render(&scratch.0, &web, name, "a");
        let b = render(&scratch.0, &web, name, "b");
        if a != b {
            let first_diff = a
                .bytes()
                .zip(b.bytes())
                .position(|(x, y)| x != y)
                .unwrap_or_else(|| a.len().min(b.len()));
            let ctx = |s: &str| {
                let lo = first_diff.saturating_sub(60);
                let hi = (first_diff + 60).min(s.len());
                s.get(lo..hi).unwrap_or("").to_string()
            };
            failures.push(format!(
                "{name}: differs at byte {first_diff} ({} vs {} bytes)\n  a: …{}…\n  b: …{}…",
                a.len(),
                b.len(),
                ctx(&a),
                ctx(&b)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "non-deterministic SVG output (#488):\n{}",
        failures.join("\n")
    );
}
