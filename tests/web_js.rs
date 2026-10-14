//! Runs the browser regression tests under Node: `tests/web/sanitize_test.mjs`
//! (escapeHtml / safeUrl / renderMarkdown from `web/app.js`) and
//! `tests/web/svg_attrs_test.mjs` (the readers of ggplot-rs SVG metadata).
//! Skipped (with a note) when `node` is not installed.

fn run_node(file: &str) {
    let script = format!("{}/tests/web/{file}", env!("CARGO_MANIFEST_DIR"));
    let out = match std::process::Command::new("node").arg(&script).output() {
        Ok(o) => o,
        Err(_) => {
            eprintln!("node not found — skipping {file}");
            return;
        }
    };
    assert!(
        out.status.success(),
        "{file} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn web_html_sinks_are_inert() {
    run_node("sanitize_test.mjs");
}

#[test]
fn web_reads_svg_metadata() {
    run_node("svg_attrs_test.mjs");
}

#[test]
fn web_contract_plot_glue() {
    run_node("contract_test.mjs");
}

/// Loads the fetched anofox wasm extensions in DuckDB-Wasm (Node); skips
/// itself unless `scripts/fetch-wasm-extensions.sh` and
/// `npm install --prefix tests/web` have been run.
#[test]
fn web_wasm_extensions_load() {
    run_node("wasm_extensions_test.mjs");
}
